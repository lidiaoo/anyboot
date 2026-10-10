//! 内置 HTTP 服务（阶段 3）。
//!
//! 默认监听 `0.0.0.0:12300`，除 `/health` 外所有端点都要 token。
//! 这里只负责传输与解析（方法/路径/查询/表单/JSON 里的 token、来源是否 loopback），
//! 具体路由与执行交给 `super::handle_remote_request`，两边共用同一套核心逻辑。

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tiny_http::{Header, Response, Server};

use super::{handle_remote_request, RemoteReply, RemoteRequest, MAX_REQUEST_BODY};

/// 工作线程数：请求都很短（真正耗时的动作在 lib 侧另起线程），4 个足够。
const WORKER_THREADS: usize = 4;
/// 等待请求的超时：只为周期性检查停止标志，不增加延迟。
const RECV_TIMEOUT: Duration = Duration::from_millis(200);
/// 自连接唤醒的超时：连的是本机正在监听的端口，正常是微秒级，这个值只是防呆。
const WAKE_TIMEOUT: Duration = Duration::from_millis(500);
/// 等旧监听 socket 真正放手的上限（收服务时最多阻塞这么久）。
const RELEASE_TIMEOUT: Duration = Duration::from_millis(2000);
/// 等待期间的重试间隔。
const RELEASE_POLL: Duration = Duration::from_millis(5);

#[derive(Clone, Debug)]
pub(crate) struct ServerConfig {
    pub(crate) bind: String,
    pub(crate) port: u16,
}

pub(crate) struct RunningServer {
    stop: Arc<AtomicBool>,
    joins: Vec<JoinHandle<()>>,
    pub(crate) addr: String,
    /// 唤醒 tiny_http 的 accept 线程用的**可达**地址（不能是通配地址，见 `stop`）
    wake: Vec<SocketAddr>,
    /// 等旧监听 socket 放手时拿来试绑的地址（就是当初 bind 的那个）
    probe: (String, u16),
}

impl RunningServer {
    /// 确定性收掉服务：**返回时端口一定已经放开**。
    ///
    /// 三步缺一不可：
    /// 1. 置停止标志 + 敲一次自己的端口，把 worker 从 `recv_timeout(200ms)` 里提前叫醒；
    /// 2. join：最后一个 worker 退出时 `Arc<Server>` 归零，tiny_http 的 `Drop` 在这里跑
    ///    （它置 close，并自己连一次监听地址试图唤醒自己的 accept 线程）；
    /// 3. 补一次**可达的**唤醒，再等端口真的能重新绑定。
    ///
    /// 第 3 步不是兜底，是必需的：tiny_http 的 accept 线程是**异步**退出的，退出之前它
    /// 一直握着监听 socket，所以「join 完 worker」≠「端口已释放」。而它 `Drop` 里的那次
    /// 自连接用的是 `listening_addr`——绑 0.0.0.0 时那是个**通配地址**，Windows 上
    /// `connect(0.0.0.0:port)` 直接报 10049（Linux 会把它当 127.0.0.1，所以只在 Windows 上炸），
    /// accept 线程等不到唤醒就永远卡住，端口再也放不开。这正是「改了 token 重新监听后
    /// 一直报 10048、还得退出程序才能恢复」的根因。
    pub(crate) fn stop(self) -> Result<(), String> {
        self.stop.store(true, Ordering::SeqCst);
        wake(&self.wake, WAKE_TIMEOUT);
        for join in self.joins {
            let _ = join.join();
        }
        // Drop 跑完了，close 已经是 true；现在敲醒 accept 线程让它走到循环条件去
        wake(&self.wake, WAKE_TIMEOUT);
        wait_until_released(&self.probe, &self.wake)
    }
}

pub(crate) fn start(app: AppHandle, config: ServerConfig) -> Result<RunningServer, String> {
    let server = Server::http((config.bind.as_str(), config.port))
        .map_err(|error| format!("无法监听 {}:{}: {error}", config.bind, config.port))?;
    // 用**实际绑上**的地址算唤醒目标：绑 127.0.0.1 和绑 0.0.0.0 要连的地址不一样
    let wake = wake_addrs(server.server_addr().to_ip());
    let server = Arc::new(server);
    let stop = Arc::new(AtomicBool::new(false));
    let addr = format!("{}:{}", config.bind, config.port);

    let mut workers = Vec::with_capacity(WORKER_THREADS);
    for _ in 0..WORKER_THREADS {
        let server = Arc::clone(&server);
        let stop = Arc::clone(&stop);
        let app = app.clone();
        workers.push(std::thread::spawn(move || {
            worker(app, server, stop);
        }));
    }

    Ok(RunningServer {
        stop,
        joins: workers,
        addr,
        wake,
        probe: (config.bind, config.port),
    })
}

/// 朝一组地址各敲一下（只为自连接唤醒 accept 线程，成败都无所谓）。
fn wake(targets: &[SocketAddr], timeout: Duration) {
    for addr in targets {
        let _ = TcpStream::connect_timeout(addr, timeout);
    }
}

/// 自连接该往哪儿敲。
///
/// 不能直接连监听到的那个地址：绑 0.0.0.0 / :: 时它是**通配**地址，Windows 上
/// `connect` 它必然失败（10049），而 tiny_http 的 accept 线程就是靠自连接唤醒的。
/// 通配换成同族回环地址；另配一个 127.0.0.1 兜双栈（绑 `::` 且允许 v4 映射时它才连得上）。
fn wake_addrs(bound: Option<SocketAddr>) -> Vec<SocketAddr> {
    let Some(bound) = bound else {
        return Vec::new();
    };
    let ip = match bound.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::from(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::from(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let primary = SocketAddr::new(ip, bound.port());
    let loopback = SocketAddr::new(IpAddr::from(Ipv4Addr::LOCALHOST), bound.port());
    if primary == loopback {
        vec![primary]
    } else {
        vec![primary, loopback]
    }
}

/// 轮询到 `bind:port` 真的能重新绑定为止（有界）。
///
/// 「我们自己能绑上」是「旧 socket 已经放手」唯一可信的信号 —— tiny_http 的 accept
/// 线程异步退出，join 完我们的 worker 并不等于端口已经释放。
/// 试绑用的 listener 用完立刻丢掉：它从没 listen/accept 过，不会留下 TIME_WAIT。
fn wait_until_released(probe: &(String, u16), targets: &[SocketAddr]) -> Result<(), String> {
    let deadline = Instant::now() + RELEASE_TIMEOUT;
    loop {
        match std::net::TcpListener::bind((probe.0.as_str(), probe.1)) {
            Ok(listener) => {
                drop(listener);
                return Ok(());
            }
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "旧监听 socket 没在 {}ms 内放手（{}:{} 仍被本进程占着：{error}）",
                        RELEASE_TIMEOUT.as_millis(),
                        probe.0,
                        probe.1
                    ));
                }
                // accept 线程可能刚被唤醒、还没轮到执行，补一下再等
                wake(targets, WAKE_TIMEOUT);
                std::thread::sleep(RELEASE_POLL);
            }
        }
    }
}

fn worker(app: AppHandle, server: Arc<Server>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match server.recv_timeout(RECV_TIMEOUT) {
            Ok(Some(request)) => handle_one(&app, request),
            // 超时：回头再看停止标志（stop() 的自连接也会立刻把它唤醒）
            Ok(None) => continue,
            Err(_) => break,
        }
    }
}

fn handle_one(app: &AppHandle, mut request: tiny_http::Request) {
    let method = format!("{}", request.method());
    let url = request.url().to_string();
    let (path, query) = split_url(&url);
    let source = request
        .remote_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| "-".to_string());
    let headers = request.headers().to_vec();

    let mut body = String::new();
    if let Some(length) = request.body_length() {
        if length > MAX_REQUEST_BODY {
            // 明确回 413，别静默当空 body（那样会变成看不懂的 401）
            let reply = RemoteReply::error(413, "body_too_large", "请求体过大");
            send(request, &reply);
            return;
        }
        let _ = request
            .as_reader()
            .take(length as u64)
            .read_to_string(&mut body);
    }

    let body_pairs = parse_pairs(&body);
    // token 的取值顺序：请求头 X-Auth-Token → query → body（见 plan §4.4）
    let token_value = header_value(&headers, "X-Auth-Token")
        .or_else(|| find_pair(&query, "token"))
        .or_else(|| find_pair(&body_pairs, "token"))
        .filter(|value| !value.trim().is_empty());

    let remote_request = RemoteRequest {
        method,
        path,
        query,
        body: body_pairs,
        token: token_value,
        source: source.clone(),
        source_is_loopback: is_loopback(&source),
    };
    let reply = handle_remote_request(app, &remote_request);
    send(request, &reply);
}

fn send(request: tiny_http::Request, reply: &RemoteReply) {
    let body = serde_json::to_string(&reply.json).unwrap_or_else(|_| "{\"ok\":false}".to_string());
    let content_type = Header::from_bytes(
        &b"Content-Type"[..],
        &b"application/json; charset=utf-8"[..],
    )
    .expect("静态 header 一定合法");
    let mut response = Response::from_string(body)
        .with_status_code(reply.status)
        .with_header(content_type);
    if reply.status == 401 {
        // 让暴力试探慢一点；正常请求不受影响
        response = response.with_header(
            Header::from_bytes(&b"WWW-Authenticate"[..], &b"Token"[..]).expect("静态 header"),
        );
    }
    // 响应体永不回显 token
    let _ = request.respond(response);
}

fn split_url(url: &str) -> (String, Vec<(String, String)>) {
    match url.split_once('?') {
        Some((path, query)) => (path.to_string(), parse_pairs(query)),
        None => (url.to_string(), Vec::new()),
    }
}

/// 解析 `k=v&k2=v2`（query 或表单 body 都走这里）。
fn parse_pairs(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((key, value)) => (percent_decode(key), percent_decode(value)),
            None => (percent_decode(part), String::new()),
        })
        .collect()
}

pub(crate) fn find_pair(pairs: &[(String, String)], key: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.clone())
}

/// `pub(crate)`：CLI 侧的 `percent_encode` 有单测要对着它做往返校验
/// （两个模块必须严格互逆，否则含空格的 GRUB selector 到服务端就散了）。
pub(crate) fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                match hex {
                    Some(byte) => {
                        decoded.push(byte);
                        index += 3;
                    }
                    None => {
                        decoded.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).to_string()
}

fn header_value(headers: &[Header], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|header| header.field.to_string().eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str().to_string())
}

/// 是否来自本机（只有 loopback 才允许用 `channel=cli` 冒充 CLI 渠道，见 plan §4.7）。
pub(crate) fn is_loopback(source: &str) -> bool {
    match source.parse::<std::net::IpAddr>() {
        Ok(addr) => addr.is_loopback(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `stop()` 是「置停止标志 + 自连接唤醒 accept + join + 等端口放手」，
    /// 这里用真 socket（监听 socket 由另一个线程持有，和 tiny_http 的形状一致）验一遍：
    /// 返回之后端口必须能再被绑定。
    #[test]
    fn stop_releases_the_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("先占一个空闲端口");
        let port = listener.local_addr().expect("local_addr").port();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_flag = Arc::clone(&stop);
        let join = std::thread::spawn(move || {
            listener
                .set_nonblocking(false)
                .expect("blocking 模式才用得上唤醒连接");
            while !thread_flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
        });

        let server = RunningServer {
            stop,
            joins: vec![join],
            addr: format!("127.0.0.1:{port}"),
            wake: vec![SocketAddr::from(([127, 0, 0, 1], port))],
            probe: ("127.0.0.1".to_string(), port),
        };
        server.stop().expect("stop() 返回时端口就该已经放开");

        let rebound = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(rebound.is_ok(), "端口没释放: {:?}", rebound.err());
    }

    /// 通配地址（0.0.0.0 / ::）不能拿去自连接，必须换成回环地址。
    #[test]
    fn wake_addrs_replace_wildcard_with_loopback() {
        let wake = wake_addrs(Some(SocketAddr::from(([0, 0, 0, 0], 12300))));
        assert_eq!(wake, vec![SocketAddr::from(([127, 0, 0, 1], 12300))]);

        // :: → ::1，再兜一个 127.0.0.1（双栈时才有用）
        let wake = wake_addrs(Some(SocketAddr::from(([0u16; 8], 12300))));
        assert_eq!(
            wake,
            vec![
                SocketAddr::new(IpAddr::from(Ipv6Addr::LOCALHOST), 12300),
                SocketAddr::from(([127, 0, 0, 1], 12300)),
            ]
        );

        // 具体地址原样连
        let wake = wake_addrs(Some(SocketAddr::from(([192, 168, 1, 5], 12300))));
        assert_eq!(wake[0], SocketAddr::from(([192, 168, 1, 5], 12300)));

        // 非 TCP 监听（unix socket）没有自连接一说
        assert!(wake_addrs(None).is_empty());
    }

    /// 真起一个 tiny_http 服务绑在 0.0.0.0 上，按 `stop()` 的收尾顺序来一遍，
    /// 验证「补一次可达的唤醒 + 轮询」确实能把端口收回来。
    ///
    /// 这条直接钉住根因：tiny_http 的 accept 线程退出前一直握着监听 socket，
    /// 而它 `Drop` 里的自连接在 Windows 上连不通通配地址（见下面那条测试）。
    #[test]
    fn wait_until_released_reclaims_a_wildcard_port() {
        let server = Server::http(("0.0.0.0", 0)).expect("起一个真实服务");
        let bound = server
            .server_addr()
            .to_ip()
            .expect("TCP 服务一定有 IP 地址");
        assert!(bound.ip().is_unspecified(), "要的就是通配地址这一路");
        let targets = wake_addrs(Some(bound));

        // 确保 accept 线程已经阻塞在 accept() 上：没阻塞的话 close 标志会被它自己在
        // 下一轮循环开头读到，测不出「卡死在 accept」这条路径
        std::thread::sleep(Duration::from_millis(100));
        drop(server);

        wait_until_released(&("0.0.0.0".to_string(), bound.port()), &targets)
            .expect("端口应该被收回来");
        let rebound = std::net::TcpListener::bind(("0.0.0.0", bound.port()));
        assert!(rebound.is_ok(), "端口没释放: {:?}", rebound.err());
    }

    /// 根因钉子（只在 Windows 上有意义）：
    /// `tiny_http::Server::drop` 用 `listening_addr` 自连接唤醒 accept 线程，绑 0.0.0.0 时
    /// 那就是 `0.0.0.0:port` —— Windows 上这个 connect 直接失败（10049），accept 线程等不到
    /// 唤醒就永远不放监听 socket。Linux 把它当 127.0.0.1，所以这个坑只在 Windows 上现形。
    #[cfg(target_os = "windows")]
    #[test]
    fn connecting_to_the_wildcard_address_fails_on_windows() {
        let listener = std::net::TcpListener::bind("0.0.0.0:0").expect("先占一个端口");
        let port = listener.local_addr().expect("local_addr").port();

        let wildcard = SocketAddr::from(([0u8, 0, 0, 0], port));
        assert!(
            TcpStream::connect_timeout(&wildcard, Duration::from_millis(500)).is_err(),
            "Windows 上 connect(0.0.0.0) 居然连上了？这个结论得重新查"
        );

        // 换成回环地址就能连上 —— 我们补的唤醒用的就是它
        let loopback = SocketAddr::from(([127, 0, 0, 1], port));
        assert!(TcpStream::connect_timeout(&loopback, Duration::from_millis(500)).is_ok());
    }

    #[test]
    fn split_url_separates_path_and_query() {
        let (path, query) = split_url("/api/power?action=sleep&token=abc");
        assert_eq!(path, "/api/power");
        assert_eq!(find_pair(&query, "action").as_deref(), Some("sleep"));
        assert_eq!(find_pair(&query, "token").as_deref(), Some("abc"));

        let (path, query) = split_url("/health");
        assert_eq!(path, "/health");
        assert!(query.is_empty());
    }

    #[test]
    fn parse_pairs_decodes_percent_and_plus_and_keeps_case_insensitive_keys() {
        let pairs = parse_pairs("Token=a%2Bb%20c&channel=CLI&flag");
        assert_eq!(find_pair(&pairs, "token").as_deref(), Some("a+b c"));
        assert_eq!(find_pair(&pairs, "TOKEN").as_deref(), Some("a+b c"));
        assert_eq!(find_pair(&pairs, "token").is_some(), true);
        assert_eq!(find_pair(&pairs, "channel").as_deref(), Some("CLI"));
        // 只有键没有值
        assert_eq!(find_pair(&pairs, "flag").as_deref(), Some(""));
        // 不在就是 None
        assert_eq!(find_pair(&pairs, "missing"), None);
    }

    #[test]
    fn percent_decode_is_lenient_with_broken_input() {
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%E4%B8%AD%E6%96%87"), "中文");
        // 坏转义原样保留，不 panic
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%4"), "%4");
    }

    #[test]
    fn loopback_detection_covers_ipv4_ipv6_and_junk() {
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("::1"));
        assert!(!is_loopback("192.168.1.10"));
        assert!(!is_loopback("::ffff:192.168.1.10"));
        assert!(!is_loopback("-"));
    }
}
