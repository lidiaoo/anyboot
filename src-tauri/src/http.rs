//! 内置 HTTP 服务（阶段 3）。
//!
//! 默认监听 `0.0.0.0:12300`，除 `/health` 外所有端点都要 token。
//! 这里只负责传输与解析（方法/路径/查询/表单/JSON 里的 token、来源是否 loopback），
//! 具体路由与执行交给 `super::handle_remote_request`，两边共用同一套核心逻辑。

use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use tauri::AppHandle;
use tiny_http::{Header, Response, Server};

use super::{handle_remote_request, RemoteReply, RemoteRequest, MAX_REQUEST_BODY};

/// 工作线程数：请求都很短（真正耗时的动作在 lib 侧另起线程），4 个足够。
const WORKER_THREADS: usize = 4;
/// 等待请求的超时：只为周期性检查停止标志，不增加延迟。
const RECV_TIMEOUT: Duration = Duration::from_millis(200);

#[derive(Clone, Debug)]
pub(crate) struct ServerConfig {
    pub(crate) bind: String,
    pub(crate) port: u16,
}

pub(crate) struct RunningServer {
    stop: Arc<AtomicBool>,
    joins: Vec<JoinHandle<()>>,
    pub(crate) addr: String,
}

impl RunningServer {
    /// 确定性收掉服务：置停止标志 + 自连接唤醒阻塞的 accept，然后 join。
    pub(crate) fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        let port = self
            .addr
            .rsplit(':')
            .next()
            .and_then(|port| port.parse::<u16>().ok());
        if let Some(port) = port {
            let _ = TcpStream::connect_timeout(
                &SocketAddr::from(([127, 0, 0, 1], port)),
                Duration::from_millis(500),
            );
        }
        for join in self.joins {
            let _ = join.join();
        }
    }
}

pub(crate) fn start(app: AppHandle, config: ServerConfig) -> Result<RunningServer, String> {
    let server = Server::http((config.bind.as_str(), config.port))
        .map_err(|error| format!("无法监听 {}:{}: {error}", config.bind, config.port))?;
    let server = Arc::new(server);
    let stop = Arc::new(AtomicBool::new(false));
    let addr = format!("{}:{}", config.bind, config.port);

    let mut joins = Vec::with_capacity(WORKER_THREADS);
    for _ in 0..WORKER_THREADS {
        let server = Arc::clone(&server);
        let stop = Arc::clone(&stop);
        let app = app.clone();
        joins.push(std::thread::spawn(move || {
            worker(app, server, stop);
        }));
    }

    Ok(RunningServer { stop, joins, addr })
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

fn percent_decode(value: &str) -> String {
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

    /// `stop()` 是「置停止标志 + 自连接唤醒 accept + join」，
    /// 这里用真 socket 验一遍：收掉之后端口必须能再被绑定。
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
        };
        server.stop();

        // 收掉之后同一个端口应该还能绑上（time_wait 由 SO_REUSEADDR 兜住）
        let rebound = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(rebound.is_ok(), "端口没释放: {:?}", rebound.err());
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
