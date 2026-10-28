//! 内置 HTTP 服务（阶段 3）。
//!
//! 默认监听 `0.0.0.0:12300`，除 `/health` 外所有端点都要 token。
//! 这里只负责传输与解析（方法/路径/查询/表单/JSON 里的 token、来源是否 loopback），
//! 具体路由与执行交给 `super::handle_remote_request`，两边共用同一套核心逻辑。

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine as _;
use sha1::{Digest, Sha1};
use tauri::AppHandle;
use tiny_http::{Header, Response, Server};

use super::{
    handle_remote_request, websocket_disconnect_all, websocket_hello, websocket_subscribe,
    websocket_unsubscribe, RemoteReply, RemoteRequest, MAX_REQUEST_BODY,
};

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

/// WebSocket 推送通道的路径（网页端 / 手机端用它替掉 2s 一次的 /api/ui-state 轮询）。
const WS_PATH: &str = "/api/ws";
/// RFC 6455 握手写死的魔数。
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// 推送线程等新快照的时长：只是让它周期性地回来看看要不要发心跳，不是延迟上限
/// （一有新快照立刻返回）。
const WS_POLL: Duration = Duration::from_millis(1000);
/// 空闲多久发一个 ping。
///
/// 两个作用：① 保活 —— 手机切网/路由器踢空闲连接时，一条 ping 就能把链路重新
/// 激活；② 它是我们**唯一**的「对端还在吗」探测器（见 `serve_websocket` 的说明，
/// 这条连接只写不读，对端消失只能从写失败上看出来）。
const WS_KEEPALIVE: Duration = Duration::from_secs(25);
/// 网页端把 token 存在这个 cookie 里（前端 `REMOTE_TOKEN_COOKIE` 必须同名）。
/// WebSocket 握手喝不进自定义请求头，这是浏览器唯一会自动带上的凭据。
const WS_TOKEN_COOKIE: &str = "anyboot_token";

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
        // 推送连接的线程是 detached 的（不归下面这批 worker），收服务时得单独请它们走：
        // 清掉订阅表 → 那边的 recv 立刻拿到 Disconnected → 线程自己收尾退出。
        // 不清的话，换 token / 关服务之后这些线程会抱着旧 socket 一直挂着。
        websocket_disconnect_all();
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

    // WebSocket 升级请求要先分流：它没有 body，而且一旦升级成功，连接的所有权
    // 就交给推送线程了（`upgrade` 吃掉 request），不能再走常规的「解析 → 路由 → 回包」。
    let upgrade_key = if path == WS_PATH {
        websocket_key(&headers)
    } else {
        None
    };

    let mut body = String::new();
    if upgrade_key.is_none() {
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
    }

    let body_pairs = parse_pairs(&body);
    // token 的取值顺序：请求头 X-Auth-Token → query → cookie → body（见 plan §4.4）。
    // cookie 排在 query 之后：手机 App 用 `?token=` 最省事，网页端则两条路都有
    // （WebSocket 握手没法带自定义头，cookie 是那条路上的兜底）。
    let token_value = header_value(&headers, "X-Auth-Token")
        .or_else(|| find_pair(&query, "token"))
        // 远程接入网关配对串用 ab_token 命名（设计稿⑦补8）：跨源 iframe 带
        // 自定义 Header，给它一条独立 query 键，取值后与本机 token 同链处理。
        .or_else(|| find_pair(&query, "ab_token"))
        .or_else(|| cookie_token(&headers))
        .or_else(|| find_pair(&body_pairs, "token"))
        .filter(|value| !value.trim().is_empty());

    if let Some(key) = upgrade_key {
        serve_websocket(app, request, &key, token_value, &source);
        return;
    }

    let remote_request = RemoteRequest {
        method,
        path,
        query,
        body: body_pairs,
        raw_body: body,
        token: token_value,
        source: source.clone(),
        source_is_loopback: is_loopback(&source),
    };
    let reply = handle_remote_request(app, &remote_request);
    send(request, &reply);
}

fn send(request: tiny_http::Request, reply: &RemoteReply) {
    let mut response = match &reply.raw {
        // 静态资源（网页控制台页面）：原样回字节，带 no-store 防止改版后浏览器吃缓存
        Some((content_type, bytes)) => {
            let content_type =
                Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes())
                    .expect("静态 content-type 一定合法");
            let no_store = Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..])
                .expect("静态 header 一定合法");
            Response::from_data(bytes.clone())
                .with_status_code(reply.status)
                .with_header(content_type)
                .with_header(no_store)
        }
        None => {
            let body =
                serde_json::to_string(&reply.json).unwrap_or_else(|_| "{\"ok\":false}".to_string());
            let content_type = Header::from_bytes(
                &b"Content-Type"[..],
                &b"application/json; charset=utf-8"[..],
            )
            .expect("静态 header 一定合法");
            Response::from_string(body)
                .with_status_code(reply.status)
                .with_header(content_type)
        }
    };
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
    let url = strip_absolute_uri(url);
    match url.split_once('?') {
        Some((path, query)) => (path.to_string(), parse_pairs(query)),
        None => (url.to_string(), Vec::new()),
    }
}

/// 把「绝对 URI 形式的请求目标」剥成纯路径（纯函数）。
///
/// 客户端配了系统代理时，HTTP 请求行发的是**绝对 URI**：
/// `GET http://192.168.1.5:12300/app.js HTTP/1.1`（普通直连才是 `GET /app.js`）。
/// 这时 `path` 里带着 scheme 与 host，凡是按路径匹配的路由（`static_ui_reply`
/// 的静态页、`/health`、`/api/*`）**全部落空** → 落到 token 闸门 → 401。
///
/// 实测（Xvfb + WebKit 浏览器，环境里带 `http_proxy=http://127.0.0.1:45611`）：
/// 页面**整片空白**，宿主日志是
/// `[http-access] 127.0.0.1 http http://127.0.0.1:12400/styles.css -> rejected token 无效`
/// —— 连自己的样式表都取不到；把代理摘掉后同一次加载立刻变成
/// `/api/ui-command -> ok`。这不是测试环境特有的：任何走了系统代理的浏览器
/// （国内挂 Clash/Surge 之类很常见）打开网页端都会撞上，症状就是「网页一片空白」。
///
/// 所以在解析的最前头统一剥掉，代理与非代理行为一致。本来就是相对路径的原样返回。
fn strip_absolute_uri(target: &str) -> &str {
    let bytes = target.as_bytes();
    for scheme in [&b"http://"[..], &b"https://"[..]] {
        if bytes.len() >= scheme.len() && bytes[..scheme.len()].eq_ignore_ascii_case(scheme) {
            // 前缀已确认是 ASCII，按字节长度切片不会切坏 UTF-8
            let rest = &target[scheme.len()..];
            // 取 host 之后的第一个 '/'；`http://host` 这种退化形式当根路径
            return match rest.find('/') {
                Some(index) => &rest[index..],
                None => "/",
            };
        }
    }
    target
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

// ==================== WebSocket 推送通道（/api/ws） ====================
//
// 网页端与手机端原先靠每 2s 拉一次 `/api/ui-state` 兜底（没有推送通道）。这里给
// 它们一条真正的推送：连上即为一条长连接，之后每次 `publish_ui` 都把新快照推过去。
// 传输层只做「发」，业务（鉴权、快照内容）仍旧全部在 lib 侧，和 HTTP 那条路同一个来源。

/// `Sec-WebSocket-Accept` = base64(SHA1(key + GUID))（RFC 6455 §4.2.2，纯函数）。
fn websocket_accept_key(secret: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(secret.trim().as_bytes());
    hasher.update(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

/// 认出「这是一个 WebSocket 升级请求」，返回它的 `Sec-WebSocket-Key`。
///
/// 三个条件都要满足：`Upgrade` 里有 websocket、`Connection` 里有 upgrade
/// （可能是逗号列表，比如 `keep-alive, Upgrade`）、以及带了 key。
fn websocket_key(headers: &[Header]) -> Option<String> {
    let upgrade = header_value(headers, "Upgrade")?;
    if !upgrade
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("websocket"))
    {
        return None;
    }
    let connection = header_value(headers, "Connection").unwrap_or_default();
    if !connection
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
    {
        return None;
    }
    let key = header_value(headers, "Sec-WebSocket-Key")?;
    let key = key.trim().to_string();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

/// 从 `Cookie` 头里取 AnyBoot 的访问令牌（找不到返回 None）。
///
/// 为什么要有这条路：浏览器建立 WebSocket 时**不能**自定义请求头，`X-Auth-Token`
/// 根本没机会带上；能自动带上的只有 query 与 cookie。网页端本来就把 token 写在
/// `anyboot_token` 这个 cookie 里（同源、SameSite=Strict），正好复用。
fn cookie_token(headers: &[Header]) -> Option<String> {
    let raw = header_value(headers, "Cookie")?;
    for pair in raw.split(';') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name.trim() == WS_TOKEN_COOKIE {
            // 前端写入时走的是 encodeURIComponent
            let value = percent_decode(value.trim()).trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// 编一个 WebSocket 帧（**只做服务端 → 客户端这一个方向**）。
///
/// 所以有两件事刻意不做：① 不掩码 —— RFC 6455 §5.1 规定服务端发往客户端的帧
/// 必须不带掩码（客户端发来的帧才必须掩码）；② 不分片 —— 一个 payload 一帧。
/// 长度按 §5.2 的三种形式选：<126 一字节、≤65535 用 126 + u16、再大用 127 + u64。
fn websocket_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x80 | opcode); // FIN = 1，不分片
    match payload.len() {
        length if length < 126 => frame.push(length as u8),
        length if length <= u16::MAX as usize => {
            frame.push(126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    frame
}

/// 快照帧（opcode 0x1 = 文本）。快照是 JSON，用文本帧省得两端再做一次编解码。
fn websocket_text_frame(payload: &str) -> Vec<u8> {
    websocket_frame(0x1, payload.as_bytes())
}

/// 心跳帧（opcode 0x9 = ping），载荷为空。
fn websocket_ping_frame() -> Vec<u8> {
    websocket_frame(0x9, &[])
}

/// 关闭帧（opcode 0x8），载荷为空（不带状态码 = 1005/无码）。
fn websocket_close_frame() -> Vec<u8> {
    websocket_frame(0x8, &[])
}

/// 把一帧写完并冲出去。返回 Err 表示这条连接已经不可用了。
fn websocket_send<W: Write + ?Sized>(stream: &mut W, frame: &[u8]) -> std::io::Result<()> {
    stream.write_all(frame)?;
    stream.flush()
}

/// 接管一条已升级的连接，直到对端离开或服务被收掉。
///
/// ## 为什么只写不读
///
/// `Request::upgrade` 交回来的是一条**不可拆分**的流（`Box<dyn ReadWrite + Send>`，
/// 底层 reader/writer 被 box 进同一个对象，拿不到 `TcpStream` 也就没法设读超时）。
/// 而「读对端帧」与「发快照」必须并发 —— 阻塞读会一直占着这条流。所以这里放弃读：
/// 这条通道本来就是单向推送，客户端不需要往上发任何东西。代价是**对端消失只能从
/// 写失败上发现**，于是有了 `WS_KEEPALIVE` 定期 ping：它既是保活，也是那个探测器。
fn serve_websocket(
    app: &AppHandle,
    request: tiny_http::Request,
    key: &str,
    token: Option<String>,
    source: &str,
) {
    // 先鉴权再升级：token 不对就别把连接开成 WebSocket，直接回一个普通 HTTP 401
    // （浏览器会把它报成「握手失败」，语义上正是如此）。
    let hello = match websocket_hello(app, token.as_deref()) {
        Ok(payload) => payload,
        Err(reply) => {
            send(request, &reply);
            return;
        }
    };

    let accept = Header::from_bytes(
        &b"Sec-WebSocket-Accept"[..],
        websocket_accept_key(key).as_bytes(),
    )
    .expect("base64 的结果一定是合法的 header 值");
    let mut stream = request.upgrade("websocket", Response::empty(101).with_header(accept));

    super::record_websocket_event(app, source, true);
    let (client, receiver) = websocket_subscribe();
    // 首帧就是完整快照：客户端连上即可渲染，不必再单独 GET 一次 /api/ui-state。
    // 这一步失败（极端情况：对端刚连上就断）也照常走下面的收尾。
    let mut last_write = Instant::now();
    if websocket_send(&mut *stream, &websocket_text_frame(&hello)).is_err() {
        websocket_unsubscribe(client);
        super::record_websocket_event(app, source, false);
        return;
    }
    // 客户端刚连上就补一轮刷新（后台跑，不拖住这条连接）：首帧的快照可能是
    // 主窗页面冻结期间留下的旧状态，等保活节拍最长要 20 秒；手机刚打开
    // 「电脑页」就该看到当下的事实，而不是 20 秒前的。
    {
        let app = app.clone();
        std::thread::spawn(move || {
            if !super::busy_snapshot(&app).any() {
                super::refresh_and_update(&app);
            }
        });
    }

    loop {
        match receiver.recv_timeout(WS_POLL) {
            Ok(payload) => {
                if websocket_send(&mut *stream, &websocket_text_frame(&payload)).is_err() {
                    break;
                }
                last_write = Instant::now();
            }
            // 这一拍没有新快照：到点就敲一个 ping 探一下对端还在不在
            Err(RecvTimeoutError::Timeout) => {
                if last_write.elapsed() < WS_KEEPALIVE {
                    continue;
                }
                if websocket_send(&mut *stream, &websocket_ping_frame()).is_err() {
                    break;
                }
                last_write = Instant::now();
            }
            // 订阅表里我们这条被清掉了（收服务 / 换 token）：主动退出
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = websocket_send(&mut *stream, &websocket_close_frame());
    websocket_unsubscribe(client);
    super::record_websocket_event(app, source, false);
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

    /// 走代理的客户端发绝对 URI（`GET http://host:port/path HTTP/1.1`），
    /// 不剥掉 scheme+host 的话静态页与 /api 全部匹配不上 → 网页整片空白。
    #[test]
    fn split_url_strips_proxy_absolute_uri() {
        // 静态资源：浏览器配了系统代理时就是这个形状
        let (path, query) = split_url("http://127.0.0.1:12400/styles.css");
        assert_eq!(path, "/styles.css");
        assert!(query.is_empty());

        // 命令通道 + query（token 也走 query，不能丢）
        let (path, query) = split_url("http://192.168.1.5:12300/api/power?action=sleep&token=abc");
        assert_eq!(path, "/api/power");
        assert_eq!(find_pair(&query, "action").as_deref(), Some("sleep"));
        assert_eq!(find_pair(&query, "token").as_deref(), Some("abc"));

        // 根路径与 /health（后者是免鉴权的探活，剥错就会被 401 挡掉）
        assert_eq!(split_url("http://127.0.0.1:12400/").0, "/");
        assert_eq!(split_url("http://127.0.0.1:12400/health").0, "/health");
        assert_eq!(split_url("http://127.0.0.1:12400?x=1").0, "/");

        // scheme 大小写不敏感；裸 host 也算根路径
        assert_eq!(split_url("HTTPS://Host:443/health").0, "/health");
        assert_eq!(split_url("http://host").0, "/");

        // 直连（相对路径）必须原样不动
        assert_eq!(split_url("/app.js").0, "/app.js");
        assert_eq!(split_url("/api/status?token=t").0, "/api/status");
        // 只有前缀像 http:// 才动它，路径里出现 "http://" 不影响
        assert_eq!(split_url("/redir/http://x").0, "/redir/http://x");
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

    fn header(pairs: &[(&str, &str)]) -> Vec<Header> {
        pairs
            .iter()
            .map(|(name, value)| Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap())
            .collect()
    }

    /// RFC 6455 §1.3 的示例：这条对了才说明握手摘要算对了（写错一个字节，
    /// 浏览器和 WKWebView 都会直接把握手判失败，而且不会告诉你哪儿错了）。
    #[test]
    fn websocket_accept_key_matches_the_rfc_example() {
        assert_eq!(
            websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        // 头里带来的值可能带首尾空白，不影响结果
        assert_eq!(
            websocket_accept_key("  dGhlIHNhbXBsZSBub25jZQ==  "),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        // 换个 key 就得换个结果（不是固定串）
        assert_ne!(
            websocket_accept_key("AAAAAAAAAAAAAAAAAAAAAA=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    /// 只有三个条件都满足才算升级请求；普通的 `GET /api/ws` 不能被当成升级。
    #[test]
    fn websocket_key_requires_a_real_upgrade_request() {
        let full = header(&[
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade"),
            ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ]);
        assert_eq!(
            websocket_key(&full).as_deref(),
            Some("dGhlIHNhbXBsZSBub25jZQ==")
        );

        // Chrome / Safari 发的是逗号列表，大小写也不固定
        let list = header(&[
            ("Upgrade", "WebSocket"),
            ("Connection", "keep-alive, Upgrade"),
            ("Sec-WebSocket-Key", "abc"),
        ]);
        assert_eq!(websocket_key(&list).as_deref(), Some("abc"));

        // 少了 Connection / 少了 key / Upgrade 是别的协议：都不认
        assert_eq!(websocket_key(&header(&[("Upgrade", "websocket")])), None);
        assert_eq!(
            websocket_key(&header(&[("Upgrade", "websocket"), ("Connection", "Upgrade")])),
            None
        );
        assert_eq!(
            websocket_key(&header(&[
                ("Upgrade", "h2c"),
                ("Connection", "Upgrade"),
                ("Sec-WebSocket-Key", "abc"),
            ])),
            None
        );
        // 空 key 也不算
        assert_eq!(
            websocket_key(&header(&[
                ("Upgrade", "websocket"),
                ("Connection", "Upgrade"),
                ("Sec-WebSocket-Key", "   "),
            ])),
            None
        );
    }

    /// 帧编码的三种长度形式 + 控制帧。服务端发出去的帧**不掩码**，第二个字节
    /// 的最高位必须是 0（掩码位），否则客户端会直接断连。
    #[test]
    fn websocket_frame_covers_all_three_length_forms() {
        // 短：<126 直接放一个字节
        assert_eq!(websocket_frame(0x1, b"hi"), vec![0x81, 0x02, b'h', b'i']);

        // 中：126 + u16 大端（真实快照就有这么大）
        let frame = websocket_frame(0x1, &vec![b'x'; 200]);
        assert_eq!(&frame[..4], &[0x81, 126, 0x00, 0xC8]);
        assert_eq!(frame.len(), 4 + 200);

        // 长：127 + u64 大端
        let frame = websocket_frame(0x1, &vec![b'x'; 70000]);
        assert_eq!(&frame[..2], &[0x81, 127]);
        assert_eq!(&frame[2..10], &70_000u64.to_be_bytes());
        assert_eq!(frame.len(), 10 + 70000);

        // 边界：125 还是短形式，65535 还是中形式，65536 才换长形式
        assert_eq!(websocket_frame(0x1, &[0u8; 125])[1], 125);
        assert_eq!(websocket_frame(0x1, &[0u8; 65535])[1], 126);
        assert_eq!(websocket_frame(0x1, &[0u8; 65536])[1], 127);

        // 掩码位（第二个字节的 bit7）必须是 0
        for payload in [vec![0u8; 1], vec![0u8; 300]] {
            assert_eq!(websocket_frame(0x1, &payload)[1] & 0x80, 0);
        }

        // 控制帧：FIN=1 固定，opcode 0x9 = ping / 0x8 = close，空载荷
        assert_eq!(websocket_ping_frame(), vec![0x89, 0x00]);
        assert_eq!(websocket_close_frame(), vec![0x88, 0x00]);

        // 文本帧就是给 JSON 用的
        assert_eq!(websocket_text_frame("{}"), vec![0x81, 0x02, b'{', b'}']);
    }

    /// WebSocket 握手带不了自定义请求头，token 只能从 cookie / query 走。
    #[test]
    fn cookie_token_reads_the_anyboot_cookie() {
        assert_eq!(
            cookie_token(&header(&[(
                "Cookie",
                "anyboot_token=abc123; other=1"
            )]))
            .as_deref(),
            Some("abc123")
        );
        // 排在后面也能取到；cookie 是 encodeURIComponent 写进去的
        assert_eq!(
            cookie_token(&header(&[(
                "Cookie",
                "a=1; anyboot_token=a%2Bb%20c; z=9"
            )]))
            .as_deref(),
            Some("a+b c")
        );
        // 只有名字没有值 = 没带
        assert_eq!(cookie_token(&header(&[("Cookie", "anyboot_token=")])), None);
        // 别的 cookie / 压根没有 Cookie 头
        assert_eq!(cookie_token(&header(&[("Cookie", "other=1")])), None);
        assert_eq!(cookie_token(&[]), None);
    }
}
