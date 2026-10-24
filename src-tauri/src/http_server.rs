//! HTTP 服务：token 校验 / 健康检查 / 端口诊断 / HTTP_RUNTIME 生命周期。

use super::*;

/// 校验用户自定义的 token（纯函数，便于单测）。
///
/// - 空字符串是**合法**输入：等于「清空 token」，内置 HTTP 服务会因此不再启动（沿用现有语义）。
/// - 非空：长度 4–128，且只能是可见 ASCII —— 粘贴时混进换行/空格/全角字符会让请求一直 401，
///   在远程端极难自己查出来，所以这里直接拒掉并说清是哪个字符。
pub(crate) fn validate_http_token(token: &str) -> Result<String, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        return Ok(token);
    }
    let length = token.chars().count();
    if length < MIN_TOKEN_LEN || length > MAX_TOKEN_LEN {
        return Err(format!(
            "token 长度要在 {MIN_TOKEN_LEN}–{MAX_TOKEN_LEN} 之间（现在是 {length}）"
        ));
    }
    if let Some(bad) = token.chars().find(|c| !c.is_ascii_graphic()) {
        return Err(format!(
            "token 只能用可见 ASCII 字符（字母/数字/符号），这个字符不行：{bad:?}"
        ));
    }
    Ok(token)
}

/// 生成 32 位十六进制 token（首次运行自动生成）。
pub(crate) fn generate_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // 取不到系统随机数（几乎不会发生）：退回时间 + pid，仍保证唯一性
        let seed = format!("{}:{}", now_epoch_seconds(), std::process::id());
        let digest = Sha256::digest(seed.as_bytes());
        bytes.copy_from_slice(&digest[..16]);
    }
    hex::encode(bytes)
}

/// 一次请求的短 id（8 位十六进制），用于 pending / 日志关联。
pub(crate) fn short_request_id() -> String {
    generate_token()[..8].to_string()
}

/// 本机名称（给客户端一眼认出「这是哪台电脑」）。取不到返回空串。
///
/// 只读环境变量与几个约定路径，不开子进程 —— 它在 `/health` 里每次请求都会走，
/// 不能有进程开销。
pub(crate) fn machine_name() -> String {
    // Windows 是 COMPUTERNAME；Linux / macOS 的 shell 里是 HOSTNAME
    let from_env = ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .map(|key| std::env::var(key).unwrap_or_default());
    // Linux 兜底；其它系统这两个文件不存在，read_to_string 会失败并被跳过
    let from_file = ["/etc/hostname", "/proc/sys/kernel/hostname"]
        .iter()
        .map(|path| std::fs::read_to_string(path).unwrap_or_default());
    first_nonempty(from_env.chain(from_file))
}

/// 从一组候选里挑第一个「去掉首尾空白后非空」的值（纯函数，便于单测）。
pub(crate) fn first_nonempty<T: IntoIterator<Item = String>>(values: T) -> String {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

/// `GET /health` 的响应体（纯函数，便于单测）。
///
/// `ok`/`service` 是既有的探活字段；`machineId`/`machineName` 是给客户端认机器用的
/// —— 同一台电脑多块网卡会有多个 IP，客户端靠 `machineId` 把它们归并成一份页面。
/// `os`/`arch` 只放**编译期常量**（windows / macos / linux + x86_64 / aarch64），
/// 零开销且足以让客户端画出系统徽标；更细的版本 / CPU / 内存走鉴权后的 `/api/system`。
/// 这里**不含任何机密**，所以和探活一样免鉴权。
pub(crate) fn health_json(machine_id: &str, machine_name: &str) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "service": "anyboot",
        "machineId": machine_id.trim(),
        "machineName": machine_name.trim(),
        "os": crate::sysinfo_lite::os_family(),
        "arch": crate::sysinfo_lite::arch(),
    })
}

/// 界面上的打码 token：只露前 4 后 4，明文要用 reveal 命令专门取。
pub(crate) fn token_hint(token: &str) -> String {
    let token = token.trim();
    if token.is_empty() {
        return "未设置（服务不会启动）".to_string();
    }
    if token.len() <= 8 {
        return "*".repeat(token.len());
    }
    format!("{}…{}", &token[..4], &token[token.len() - 4..])
}

/// 常量时间比较 token（长度不同直接 false）。
pub(crate) fn token_matches(expected: &str, provided: &str) -> bool {
    let expected = expected.trim();
    let provided = provided.trim();
    if expected.is_empty() || expected.len() != provided.len() {
        return false;
    }
    expected
        .bytes()
        .zip(provided.bytes())
        .fold(0u8, |acc, (left, right)| acc | (left ^ right))
        == 0
}

/// 本机 IPv4（UDP 的 connect 只是让内核选出口地址，不发包）。
pub(crate) fn local_ipv4() -> String {
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return addr.ip().to_string();
            }
        }
    }
    "127.0.0.1".to_string()
}

/// HTTP 运行态：进程级单例。
///
/// 刻意不放进 Tauri 的 State——`app.state::<T>()` 返回的句柄是临时值，
/// 从它派生出的 MutexGuard 没法跨函数返回；这里用 OnceLock 拿到 'static 引用。
pub(crate) static HTTP_RUNTIME: std::sync::OnceLock<Mutex<HttpRuntime>> = std::sync::OnceLock::new();

pub(crate) fn http_lock() -> std::sync::MutexGuard<'static, HttpRuntime> {
    HTTP_RUNTIME
        .get_or_init(|| Mutex::new(HttpRuntime::default()))
        .lock()
        .expect("http mutex poisoned")
}

/// 进快照的端口占用视图（文案在后端算好，前端直接显示）
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PortOccupantView {
    pub(crate) pid: u32,
    pub(crate) name: String,
    /// 占用者就是本程序（当前进程，理论上不会出现）
    pub(crate) is_self: bool,
    /// 占用者是本应用的另一个实例（按进程名判断；升级后旧实例还占着端口的场景）
    pub(crate) same_app: bool,
    /// 给人看的一句话
    pub(crate) message: String,
    /// 可直接复制的结束命令
    pub(crate) kill_hint: Option<String>,
}

/// 端口占用诊断结果
#[derive(Clone, Debug)]
pub(crate) struct PortOccupant {
    pub(crate) pid: u32,
    /// 进程名（如 explorer.exe、python.exe），查不到则为空
    pub(crate) name: String,
    /// 占用者就是当前进程（理论上不会出现：我们自己没绑上才报的错）
    pub(crate) is_self: bool,
    /// 占用者是本程序的另一个实例（按进程名匹配，例如升级后旧版还驻留着）
    pub(crate) same_app: bool,
}

impl PortOccupant {
    pub(crate) fn to_view(&self) -> PortOccupantView {
        PortOccupantView {
            pid: self.pid,
            name: self.name.clone(),
            is_self: self.is_self,
            same_app: self.same_app,
            message: self.display(),
            kill_hint: self.kill_hint(),
        }
    }

    /// 界面文案：自己占着 / 同程序的另一个实例 / 完全外部，三种说法不能混。
    ///
    /// 以前「自己占着」也被算进 same_app，于是显示成「本应用的另一个实例」——
    /// 实际上是**我们自己**上一轮监听的 socket 没放开，用户会去找一个根本不存在的进程。
    pub(crate) fn display(&self) -> String {
        if self.is_self {
            format!(
                "端口还被本程序自己占着（PID {}）：上一轮监听的 socket 没放开。点「重试监听」，仍不行就重启本程序",
                self.pid
            )
        } else if self.same_app {
            format!(
                "端口被本应用的另一个实例占用：PID {}（{}）——可以点「结束占用进程」把它退掉（旧版本驻留时常见），或先手动退出那个实例",
                self.pid,
                if self.name.is_empty() { "同程序" } else { &self.name }
            )
        } else if self.name.is_empty() {
            format!(
                "端口被 PID {} 占用（拿不到进程名），请释放它或改用其他端口",
                self.pid
            )
        } else {
            format!(
                "端口被 PID {}（{}）占用，请释放它或改用其他端口",
                self.pid, self.name
            )
        }
    }

    /// 给用户复制即用的结束命令（Windows 优先 taskkill，非 Windows 给通用提示）。
    ///
    /// 占端口的是**当前进程**时不给：那是「结束自己」，后端也会直接拒绝，摆出来只会误导。
    pub(crate) fn kill_hint(&self) -> Option<String> {
        if self.pid == 0 || self.is_self {
            return None;
        }
        if cfg!(target_os = "windows") {
            Some(format!("taskkill /PID {} /F", self.pid))
        } else {
            Some(format!("kill {}", self.pid))
        }
    }
}

/// 纯函数：从 netstat -ano 的输出一行行文本里找指定端口的 LISTENING 条目，
/// 返回占用者的 PID（用于后续查进程名）。
/// 抽成纯函数是为了可单测（不真的起 netstat）。
pub(crate) fn parse_netstat_listening_port<'a>(
    lines: impl Iterator<Item = &'a str>,
    port: u16,
) -> Option<u32> {
    let marker = format!(":{port} ");
    for line in lines {
        let trimmed = line.trim();
        // netstat 输出形如: TCP 0.0.0.0:12300 0.0.0.0:0 LISTENING 29220
        let is_tcp_listen = trimmed.starts_with("TCP")
            && trimmed.contains("LISTENING")
            && trimmed.contains(&marker);
        if is_tcp_listen {
            // PID 是行末的最后一个数字 token
            let pid = trimmed
                .split_whitespace()
                .last()
                .and_then(|value| value.parse::<u32>().ok());
            if let Some(pid) = pid {
                return Some(pid);
            }
        }
    }
    None
}

/// 跨平台：按 PID 查进程名（Windows 用 wmic，非 Windows 用 ps）。查不到返回空串。
pub(crate) fn process_name_for_pid(pid: u32) -> String {
    let output = if cfg!(target_os = "windows") {
        command("wmic")
            .args([
                "process",
                "where",
                &format!("ProcessId={pid}"),
                "get",
                "Name",
                "/value",
            ])
            .output()
            .ok()
    } else {
        command("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()
    };
    let Some(output) = output else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    if cfg!(target_os = "windows") {
        // wmic /value 输出形如（每行一个 key=value）：
        //   Name=explorer.exe
        text.lines()
            .find_map(|line| line.strip_prefix("Name="))
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    } else {
        // ps comm= 直接输出进程名
        text.trim().to_string()
    }
}

/// 诊断端口占用者（只读）：起 netstat -ano 拿 PID，再查进程名，判断是否本程序。
pub(crate) fn diagnose_port_occupant(port: u16) -> Option<PortOccupant> {
    let output = command("netstat").args(["-ano"]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let pid = parse_netstat_listening_port(text.lines(), port)?;
    let name = process_name_for_pid(pid);
    let is_self = pid == std::process::id();
    // 同一个程序的另一个实例：进程名和当前 exe 名一致（升级后旧版还驻留的场景）
    let current_name = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_default();
    let same_app = is_self
        || (!name.is_empty()
            && !current_name.is_empty()
            && name.eq_ignore_ascii_case(&current_name));
    Some(PortOccupant {
        pid,
        name,
        is_self,
        same_app,
    })
}

/// 结束占用进程（仅当用户显式点「结束占用进程」按钮时调用）。
/// 跨平台：Windows 用 taskkill /F，非 Windows 用 kill。
pub(crate) fn kill_process(pid: u32) -> Result<(), String> {
    let status = if cfg!(target_os = "windows") {
        command("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()
            .map_err(|error| format!("无法执行 taskkill：{error}"))?
    } else {
        command("kill")
            .args([&pid.to_string()])
            .status()
            .map_err(|error| format!("无法执行 kill：{error}"))?
    };
    if status.success() {
        Ok(())
    } else {
        Err(format!("结束 PID {pid} 失败（exit code {status}）"))
    }
}

/// 首次运行/升级后把完整设置写回磁盘：默认值一眼可见，token 一定存在。
///
/// 环境变量覆盖**不**写进文件（它们只是调试开关）。
pub(crate) fn ensure_settings_complete(app: &AppHandle) {
    let mut settings = stored_settings(app);
    let mut changed = false;
    if settings.http_token.trim().is_empty() {
        settings.http_token = generate_token();
        changed = true;
        trace(app, "http", "首次运行：已生成 32 位十六进制 token");
    }
    if settings.machine_id.trim().is_empty() {
        // 本机标识：跨重启稳定（存在 settings.json 里），客户端靠它归并同一台
        // 电脑的多个网卡 IP。跟 token 用同一套随机来源，但没有鉴权含义。
        settings.machine_id = generate_token();
        changed = true;
        trace(app, "http", "首次运行：已生成机器标识 machineId");
    }
    if settings.http_bind.trim().is_empty() {
        settings.http_bind = DEFAULT_HTTP_BIND.to_string();
        changed = true;
    }
    if settings.http_port == 0 {
        settings.http_port = DEFAULT_HTTP_PORT;
        changed = true;
    }
    if changed {
        if let Err(error) = save_settings(app, &settings) {
            trace(app, "http", &format!("写回设置失败: {error}"));
        }
    }
}

/// 确定性收掉当前 HTTP 监听：`stop()` 返回时端口一定已经放开（tiny_http 的 accept
/// 线程是异步退场的，等待逻辑在 http::RunningServer::stop 里），所以调用方紧接着
/// 重绑端口是安全的。
///
/// 两个调用方：改配置时的「停旧的再起新的」（apply_http_config），以及「重启程序」
/// 之前先把端口让出来（restart_app）。
pub(crate) fn stop_http_server(app: &AppHandle) {
    let previous = http_lock().server.take();
    if let Some(server) = previous {
        let addr = server.addr.clone();
        match server.stop() {
            Ok(()) => trace(app, "http", &format!("已停止旧服务 {addr}，端口已放开")),
            Err(error) => trace(app, "http", &format!("已停止旧服务 {addr}，但{error}")),
        }
    }
}

/// 按当前设置（+ 环境变量覆盖）启动 / 停止 / 重启 HTTP 服务。
pub(crate) fn apply_http_config(app: &AppHandle) {
    let settings = stored_settings(app);
    let enabled = env_flag_from(std::env::var("ANYBOOT_HTTP_ENABLED").ok().as_deref())
        .unwrap_or(settings.http_enabled);
    let bind = std::env::var("ANYBOOT_HTTP_BIND")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| settings.http_bind.trim().to_string());
    let port = std::env::var("ANYBOOT_HTTP_PORT")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(settings.http_port);
    let token = settings.http_token.trim().to_string();

    // 先确定性收掉旧服务：stop() 返回时端口一定已经放开（tiny_http 的 accept 线程
    // 是异步退场的，等待逻辑在 http::RunningServer::stop 里），所以下面立刻重绑是安全的。
    stop_http_server(app);

    let mut last_error = None;
    let mut occupant = None;
    if !enabled {
        trace(app, "http", "服务已关闭（设置或环境变量）");
    } else if token.is_empty() {
        last_error = Some("token 为空，服务未启动（不允许无鉴权监听）".to_string());
        trace(app, "http", "token 为空，拒绝启动");
    } else {
        let config = http::ServerConfig {
            bind: bind.clone(),
            port,
        };
        match http::start(app.clone(), config) {
            Ok(server) => {
                trace(app, "http", &format!("已监听 {}", server.addr));
                http_lock().server = Some(server);
            }
            Err(error) => {
                trace(app, "http", &format!("启动失败: {error}"));
                // 只读诊断：告诉用户端口被谁占了（不自动杀，杀要用户点按钮）
                occupant = diagnose_port_occupant(port);
                if let Some(found) = occupant.as_ref() {
                    trace(
                        app,
                        "http",
                        &format!(
                            "端口占用者 pid={} name={} is_self={} same_app={}",
                            found.pid,
                            if found.name.is_empty() {
                                "(未知)"
                            } else {
                                &found.name
                            },
                            found.is_self,
                            found.same_app
                        ),
                    );
                }
                last_error = Some(error);
            }
        }
    }
    {
        let mut runtime = http_lock();
        runtime.last_error = last_error;
        runtime.occupant = occupant;
    }
    publish_ui(app);
}

/// 自定义 token 的长度范围（太短不安全，太长粘贴起来也难受）
pub(crate) const MIN_TOKEN_LEN: usize = 4;
