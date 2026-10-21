//! 远程电源（重启 / 关机 / 睡眠）与网络唤醒（Wake-on-LAN）。
//!
//! 两条路，按有没有配「局域网电源路由」自动选：
//!
//! 1. **配了路由**（`HubServer.power_routes`，在「编辑服务器」里逐条加）：
//!    往「广播地址 + 端口」发 UDP 包。包体是**标准的 102 字节魔术包**
//!    （6×0xFF + MAC×16），不带任何自定义尾巴 —— 端口就是指令：目标机上装的
//!    **sol** 按端口区分收到的是唤醒 / 重启 / 关机 / 睡眠，同一台机器的四个动作
//!    各自绑不同端口。这条路的优点是机器睡着、SSH 不通也能操作；
//!    缺点是没有 sol 就没人收包。
//! 2. **没配路由**（老行为，完全保留）：重启 / 关机 / 睡眠登录目标机跑 systemctl
//!    （要求该账号 `sudo -n` 免密）；唤醒自己拼魔术包，配了跳板机就由跳板机广播。
//!
//! 所有外部命令都走 `crate::command`（Windows 上带 CREATE_NO_WINDOW，不闪黑框），
//! 认证一律 `hub_tunnel::apply_auth`（密钥 / 存档密码 / 跳板代理 / ControlMaster 复用），
//! 不自己拼 BatchMode —— 见 MEMORY「远端管理认证层铁律」。

use std::net::{IpAddr, UdpSocket};

use serde_json::json;
use tauri::AppHandle;

use crate::hub::{
    normalize_power_action, split_address, HubServer, PowerRoute, POWER_REBOOT, POWER_SHUTDOWN,
    POWER_SLEEP, POWER_WAKE,
};

/// 没有可用凭据时给用户的话（与 sftp 那句同一风格）
const NO_CRED: &str = "这台服务器没有可用的自动认证凭据：请在「编辑」里改用密钥或保存密码（交互模式没法在后台跑命令）";

/// 跳板机 shell 握手标记（OK / NOTOOL）：只出现在命令输出里，**不进包体**。
/// 包体永远是最标准的 102 字节魔术包，端口即指令，不多一个字节。
const POWER_TAG: &str = "ANYBOOT-POWER";

/// 读一台服务器（找不到就给一句能看懂的错）
fn find_server(app: &AppHandle, id: &str) -> Result<HubServer, String> {
    crate::hub::load_servers_pub(app)?
        .into_iter()
        .find(|server| server.id == id)
        .ok_or_else(|| format!("服务器不存在（可能已被删除）：{id}"))
}

/// PC 端电源操作（异步包装，真正的活走 spawn_blocking —— 起 ssh 是阻塞的）
#[tauri::command]
pub async fn hub_server_power(app: AppHandle, id: String, action: String) -> Result<String, String> {
    run_power_command(app, "hub_server_power", id, action).await
}

/// PC 端网络唤醒（同上）
#[tauri::command]
pub async fn hub_wake_server(app: AppHandle, id: String) -> Result<String, String> {
    run_power_command(app, "hub_wake_server", id, POWER_WAKE.to_string()).await
}

// ==================== 埋点日志 ====================

/// 电源动作埋点：**无条件**追加到 trace.log（不受 debug_trace 开关限制）——
/// 「按了唤醒没反应」这类问题必须能查到按没按、发没发、发去哪。所有写入
/// 失败全部吞掉：日志写不进去也绝不影响电源动作本身。
pub(crate) fn power_log(app: &AppHandle, message: &str) {
    let line = format!("{}ms [power] {message}\n", crate::unix_millis());
    eprint!("trace {line}");
    let Some(path) = crate::trace_file_path(app) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, line.as_bytes()));
}

/// 按钮接口的统一埋点包装：进命令记一条入口（哪个按钮 / 哪台机器 / 什么动作），
/// 出来再记一条结果原文（成功信息或失败原因）。`entry` 就是前端 invoke 的命令名，
/// 借此能对上「界面上哪个按钮」。
async fn run_power_command(
    app: AppHandle,
    entry: &'static str,
    id: String,
    action: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = crate::hub::load_servers_pub(&app)
            .ok()
            .and_then(|servers| {
                servers
                    .iter()
                    .find(|server| server.id == id)
                    .map(|server| server.name.clone())
            })
            .unwrap_or_else(|| "?".to_string());
        power_log(
            &app,
            &format!("{entry} 入口：服务器「{name}」({id}) 动作 {action}"),
        );
        let result = dispatch(&app, &id, &action);
        match &result {
            Ok(message) => power_log(&app, &format!("{entry} 完成：{message}")),
            Err(message) => power_log(&app, &format!("{entry} 失败：{message}")),
        }
        result
    })
    .await
    .map_err(|error| format!("电源操作任务失败：{error}"))?
}

// ==================== 分发 ====================

/// 一个动作的总入口：先看有没有配路由，配了按路由发；没配才走老路。
fn dispatch(app: &AppHandle, id: &str, action: &str) -> Result<String, String> {
    let server = find_server(app, id)?;
    let action = normalize_power_action(action)
        .ok_or_else(|| format!("未知的电源动作：{action}（可用 wake / reboot / shutdown / sleep）"))?;
    let routes = server.routes_for(action);
    if !routes.is_empty() {
        return send_routes(app, &server, action, &routes);
    }
    if action == POWER_WAKE {
        wake_legacy(app, &server)
    } else {
        power_ssh(app, &server, action)
    }
}

pub(crate) fn action_label(action: &str) -> &'static str {
    match action {
        POWER_REBOOT => "重启",
        POWER_SHUTDOWN => "关机",
        POWER_SLEEP => "睡眠",
        _ => "唤醒",
    }
}

// ==================== 按路由发 UDP ====================

/// 一个待发的 UDP 包（目标 + 端口 + 包体 + 发几个 / 重试几次）
pub(crate) struct Delivery {
    pub target: String,
    pub port: u16,
    pub packet: Vec<u8>,
    /// 这条要发几个包（唤醒可配，默认 10；其余动作固定 1）
    pub times: u32,
    /// 一轮没全发出去时的重试次数（默认 10；其余动作固定 0）
    pub retry: u32,
}

/// 重试之间的间隔：WOL 收不到任何应答，只能「等一会儿再来一轮」
const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

fn send_routes(
    app: &AppHandle,
    server: &HubServer,
    action: &str,
    routes: &[PowerRoute],
) -> Result<String, String> {
    // 服务器通道没有它的 AnyBoot HTTP 凭据（token 在「电脑」档案里），
    // 「仅 HTTP」在这里永远发不出去，直接说清楚别让人等
    let mode = crate::hub::routes_mode(routes);
    if mode == "http_only" {
        return Err(
            "「仅 HTTP」模式走的是这台机器自带的 AnyBoot 接口，但服务器档案里没有它的 HTTP 凭据 \
             —— 要用 HTTP 模式请改在「远程管理 -> 电脑」里配"
                .to_string(),
        );
    }
    let all_macs = server.all_macs();
    let jump = if server.jump_id.is_empty() {
        None
    } else {
        Some(find_jump(app, &server.jump_id)?)
    };
    let plan = plan_routes(action, &server.addresses, &all_macs, routes)?;
    let (_sent, where_from) = send_plan(app, jump.as_ref(), &plan)?;

    let label = action_label(action);
    let mut message = format!("已从{where_from}按路由发送{}：{}", label, plan.described.join(" · "));
    if mode == "http_first" {
        // HTTP 兜底只在「电脑」档案里有：服务器通道只能发广播包
        message.insert_str(0, "（服务器档案没有 HTTP 凭据，「优先 HTTP」按广播包发送）");
    }
    if mode == "hybrid" {
        message.insert_str(0, "（服务器档案没有 HTTP 凭据，混合模式只发了广播包）");
    }
    if action != POWER_WAKE {
        message.push_str("。这个动作靠目标机上的 sol 监听该端口执行 —— 没装 sol 不会有反应");
    }
    if !plan.problems.is_empty() {
        message.push_str(&format!(
            "（另有 {} 条路由被跳过：{}）",
            plan.problems.len(),
            plan.problems.join("；")
        ));
    }
    Ok(message)
}

/// 找一台跳板机（找不到就给一句能看懂的错）
fn find_jump(app: &AppHandle, jump_id: &str) -> Result<HubServer, String> {
    crate::hub::load_servers_pub(app)?
        .into_iter()
        .find(|candidate| candidate.id == jump_id)
        .ok_or_else(|| "跳板机不存在（可能已被删除），请到服务器列表检查".to_string())
}

/// 一批待发的电源包 + 给用户看的描述。PC（hub 服务器 / 电脑档案）共用。
pub(crate) struct RoutePlan {
    pub deliveries: Vec<Delivery>,
    pub described: Vec<String>,
    pub problems: Vec<String>,
}

/// 按动作把启用的路由编译成待发包列表。
///
/// `addresses` = 这台机器的候选访问地址（推自动广播用）；`all_macs` = 配过的全部 MAC
/// （路由没挑 MAC 时全发）。hub 服务器与电脑档案（remote_servers）共用这套编译逻辑。
pub(crate) fn plan_routes(
    action: &str,
    addresses: &[String],
    all_macs: &[String],
    routes: &[PowerRoute],
) -> Result<RoutePlan, String> {
    let label = action_label(action);
    let mut deliveries: Vec<Delivery> = Vec::new();
    let mut described: Vec<String> = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    for route in routes {
        let macs = if route.macs.is_empty() {
            all_macs.to_vec()
        } else {
            route.macs.clone()
        };
        if macs.is_empty() {
            // 包体是标准魔术包，没有 MAC 就没有包体 —— 所有动作都必须配 MAC
            problems.push(format!(
                "「{}」这条路由没配 MAC，标准魔术包发不出去（在「编辑」里给地址行加上 MAC）",
                if route.target.is_empty() { "自动广播".to_string() } else { route.target.clone() }
            ));
            continue;
        }
        let mut targets = if route.target.is_empty() {
            auto_targets_from(addresses)
        } else {
            vec![route.target.clone()]
        };
        // 非唤醒动作只发一个目标：sol 按端口执行，同一个包到两次就会执行两次
        if action != POWER_WAKE && targets.len() > 1 {
            targets.truncate(1);
        }
        let port = route.effective_port();
        // 每个动作都能单独设「发几个包 / 重试几次」：
        //   唤醒：每块网卡各发一份魔术包（不知道现在插的是哪根网线），每份 burst 个；
        //   重启/关机/睡眠：只发一个包，发 burst 次 —— 同一个包到两次 sol 就执行两次，
        //                  默认是 1/1，用户在路由里改大了就是他要的效果。
        let waking = action == POWER_WAKE;
        let times = route.effective_burst();
        let retry = route.effective_retry();
        let packets: Vec<Vec<u8>> = if waking {
            macs.iter()
                .map(|mac| magic_packet(mac))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![magic_packet(macs.first().map(|mac| mac.as_str()).unwrap_or(""))?]
        };
        for target in &targets {
            for packet in &packets {
                deliveries.push(Delivery {
                    target: target.clone(),
                    port,
                    packet: packet.clone(),
                    times,
                    retry,
                });
            }
        }
        described.push(format!(
            "{label} → {}:{}（{}）",
            targets.join(" / "),
            port,
            if waking {
                format!("{} 个 MAC × {} 包 / 重试 {}", macs.len(), times, retry)
            } else {
                format!("{} 包 / 重试 {}", times, retry)
            }
        ));
    }

    Ok(RoutePlan { deliveries, described, problems })
}

/// 把编译好的计划发出去。`jump` = Some 时经跳板机发，None 时本机直发。
/// 返回 (发出条数, 从哪发的)；一个包都没发出去算失败。
pub(crate) fn send_plan(
    app: &AppHandle,
    jump: Option<&HubServer>,
    plan: &RoutePlan,
) -> Result<(usize, String), String> {
    if plan.deliveries.is_empty() {
        let detail = if plan.problems.is_empty() {
            "这些路由都被关掉了（启用勾选框）".to_string()
        } else {
            plan.problems.join("；")
        };
        return Err(format!("没有可发的电源包：{detail}"));
    }
    let (sent, where_from) = match jump {
        None => (send_local(&plan.deliveries)?, "本机".to_string()),
        Some(jump) => send_via_jump(app, jump, &plan.deliveries)?,
    };
    if sent == 0 {
        return Err(format!(
            "{where_from}一个包都没发出去：可能在隔离网络、防火墙拦了 UDP 广播，或者广播地址填错了"
        ));
    }
    Ok((sent, where_from))
}

/// 路由没写目标地址时的默认目标：**先各访问地址所在 /24 的广播，再全局广播**。
///
/// 顺序有意义：非唤醒动作只发第一个目标（同一动作发两遍会被 sol 执行两遍），
/// 而子网广播比 255.255.255.255 更容易真的送到（很多路由器不转发全局广播）。
pub(crate) fn auto_targets_from(addresses: &[String]) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    for address in addresses {
        if let Some((host, _)) = split_address(address) {
            if let Some(broadcast) = subnet_broadcast(&host) {
                if !targets.contains(&broadcast) {
                    targets.push(broadcast);
                }
            }
        }
    }
    targets.push("255.255.255.255".to_string());
    targets
}

fn auto_targets(server: &HubServer) -> Vec<String> {
    auto_targets_from(&server.addresses)
}

/// 本机直接发（PC 和目标机同网段时最省事）
///
/// <p>每条按 `times` 发、没全发出去就再试，最多 `retry` 次（WOL 没有回执，
/// 「重试」就是隔 200ms 再打一轮）。
fn send_local(deliveries: &[Delivery]) -> Result<usize, String> {
    let socket =
        UdpSocket::bind("0.0.0.0:0").map_err(|error| format!("创建 UDP 套接字失败：{error}"))?;
    socket
        .set_broadcast(true)
        .map_err(|error| format!("开启广播失败：{error}"))?;
    let mut sent = 0usize;
    for delivery in deliveries {
        let mut attempts = 0u32;
        loop {
            let mut round = 0u32;
            for _ in 0..delivery.times.max(1) {
                if socket
                    .send_to(&delivery.packet, (delivery.target.as_str(), delivery.port))
                    .is_ok()
                {
                    round += 1;
                }
            }
            sent += round as usize;
            if round == delivery.times.max(1) || attempts >= delivery.retry {
                break;
            }
            attempts += 1;
            std::thread::sleep(RETRY_INTERVAL);
        }
    }
    Ok(sent)
}

/// 经跳板机发：目标机可能已经睡了 / 关了，SSH 进不去，所以命令发给跳板机
fn send_via_jump(
    app: &AppHandle,
    jump: &HubServer,
    deliveries: &[Delivery],
) -> Result<(usize, String), String> {
    if jump.protocol != "ssh" {
        return Err("跳板机是 Telnet，发不了局域网电源包 —— 换一台 SSH 服务器".to_string());
    }

    // 计划表用 base64(JSON) 传：包体是二进制，命令行里只能放十六进制
    let plan: Vec<serde_json::Value> = deliveries
        .iter()
        .map(|delivery| {
            json!({
                "targets": [delivery.target],
                "port": delivery.port,
                "packet": hex_encode(&delivery.packet),
                "times": delivery.times.max(1),
                "retry": delivery.retry,
            })
        })
        .collect();
    let encoded_plan = base64_encode(serde_json::to_string(&plan).unwrap_or_default().as_bytes());
    let command = format!(
        "if command -v python3 >/dev/null 2>&1; then \
         python3 -c \"import base64,sys;exec(base64.b64decode('{}').decode())\" {}; \
         else echo {POWER_TAG}-NOTOOL; fi",
        base64_encode(PY_POWER.trim().as_bytes()),
        encoded_plan
    );

    let target = if jump.username.is_empty() {
        jump.host.clone()
    } else {
        format!("{}@{}", jump.username, jump.host)
    };
    let mut ssh = crate::command("ssh");
    crate::hub_tunnel::apply_auth(app, &jump, &mut ssh, NO_CRED)?;
    ssh.args(["-p", &jump.effective_port().to_string()])
        .args(["-o", "ConnectTimeout=8"])
        .arg(&target)
        .arg(&command)
        .env("LC_ALL", "C.UTF-8");
    let output = ssh
        .output()
        .map_err(|error| format!("无法启动 ssh：{error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let notool_tag = format!("{POWER_TAG}-NOTOOL");
    let ok_tag = format!("{POWER_TAG}-OK");
    if stdout.contains(&notool_tag) {
        return Err(format!(
            "跳板机 {} 上没有 python3 —— 装一个（apt install python3）或换台跳板机",
            jump.host
        ));
    }
    let sent = stdout
        .split_whitespace()
        .skip_while(|piece| *piece != ok_tag.as_str())
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok());
    let Some(sent) = sent else {
        return Err(format!(
            "跳板机没能发出电源包（exit {}）：{}",
            output.status.code().unwrap_or(-1),
            tail(&format!("{stdout}\n{stderr}"), 200)
        ));
    };
    let where_from = if jump.name.is_empty() {
        format!("跳板机 {}", jump.host)
    } else {
        format!("跳板机「{}」", jump.name)
    };
    Ok((sent, where_from))
}

/// 跳板机侧的 python：按计划表逐条发（不依赖装了 wakeonlan / etherwake）
///
/// `times` = 这条发几个包，`retry` = 没全发出去时再试几次（0.2s 一轮）。
const PY_POWER: &str = r#"
import base64, json, socket, sys, time
plan = json.loads(base64.b64decode(sys.argv[1]).decode('utf-8'))
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
sent = 0
for item in plan:
    packet = bytes.fromhex(item['packet'])
    times = max(1, int(item.get('times', 1)))
    retry = int(item.get('retry', 0))
    for target in item['targets']:
        for attempt in range(retry + 1):
            ok = 0
            for _ in range(times):
                try:
                    s.sendto(packet, (target, int(item['port'])))
                    ok += 1
                except Exception:
                    pass
            sent += ok
            if ok == times:
                break
            time.sleep(0.2)
print('ANYBOOT-POWER-OK %d' % sent)
s.close()
"#;

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

// ==================== 老路：SSH 电源 ====================

fn power_ssh(app: &AppHandle, server: &HubServer, action: &'static str) -> Result<String, String> {
    let (label, remote) = match action {
        POWER_REBOOT => ("重启", reboot_script()),
        POWER_SHUTDOWN => ("关机", shutdown_script()),
        POWER_SLEEP => ("睡眠", sleep_script()),
        other => return Err(format!("未知的电源动作：{other}（可用 reboot / shutdown / sleep）")),
    };
    let output = run_remote(app, server, remote)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    if combined.contains("ANYBOOT-NO-SUDO") {
        return Err(format!(
            "{label}需要目标机的 sudo 免密权限：请先在 {} 上给该账号配 NOPASSWD（或手动执行一次）",
            server.host
        ));
    }
    if combined.contains("ANYBOOT-OK") {
        return Ok(format!(
            "已提交{label}（{}）。{}",
            server.host,
            if action == POWER_SLEEP {
                "机器睡下后连接会断，要叫醒它请点「唤醒」（需要配好 MAC 并开启网络唤醒）"
            } else {
                "连接会断开，稍等片刻再刷新/重连"
            }
        ));
    }
    if !output.status.success() {
        return Err(format!(
            "{label}失败（exit {}）：{}",
            output.status.code().unwrap_or(-1),
            tail(&combined, 240)
        ));
    }
    Ok(format!("已提交{label}（{}）", server.host))
}

/// 三种动作的远端脚本。
///
/// 为什么要 `nohup ... &`：`systemctl reboot` / `poweroff` 会把当前 ssh 会话
/// 一起干掉，ssh 侧看到一个非零退出码就会误报失败；后台执行 + 立即回 OK，
/// 才是「命令确实发出去了」的准确表达。
fn reboot_script() -> &'static str {
    "if ! sudo -n true 2>/dev/null; then echo ANYBOOT-NO-SUDO; exit 3; fi\n\
     nohup sh -c 'sleep 1; sudo -n systemctl reboot || sudo -n /sbin/shutdown -r now || sudo -n /sbin/reboot' >/dev/null 2>&1 &\n\
     echo ANYBOOT-OK"
}

fn shutdown_script() -> &'static str {
    "if ! sudo -n true 2>/dev/null; then echo ANYBOOT-NO-SUDO; exit 3; fi\n\
     nohup sh -c 'sleep 1; sudo -n systemctl poweroff || sudo -n /sbin/shutdown -h now || sudo -n /sbin/halt' >/dev/null 2>&1 &\n\
     echo ANYBOOT-OK"
}

fn sleep_script() -> &'static str {
    "if ! sudo -n true 2>/dev/null; then echo ANYBOOT-NO-SUDO; exit 3; fi\n\
     nohup sh -c 'sleep 1; sudo -n systemctl suspend || sudo -n /usr/sbin/pm-suspend || sudo -n /usr/sbin/pm-suspend-hybrid' >/dev/null 2>&1 &\n\
     echo ANYBOOT-OK"
}

/// 在目标机上跑一条命令（走 apply_auth，密码/密钥/跳板全部自动）
fn run_remote(app: &AppHandle, server: &HubServer, script: &str) -> Result<std::process::Output, String> {
    if server.id.is_empty() {
        return Err("该服务器还没保存档案，无法自动认证".to_string());
    }
    let target = if server.username.is_empty() {
        server.host.clone()
    } else {
        format!("{}@{}", server.username, server.host)
    };
    let mut command = crate::command("ssh");
    crate::hub_tunnel::apply_auth(app, server, &mut command, NO_CRED)?;
    command
        .args(["-p", &server.effective_port().to_string()])
        .args(["-o", "ConnectTimeout=8"])
        .arg(&target)
        .arg(script)
        .env("LC_ALL", "C.UTF-8");
    command
        .output()
        .map_err(|error| format!("无法启动 ssh：{error}"))
}

// ==================== 老路：标准 WOL ====================

fn wake_legacy(app: &AppHandle, server: &HubServer) -> Result<String, String> {
    let macs = server.all_macs();
    if macs.is_empty() {
        return Err(format!(
            "这台服务器还没配 MAC：请在「编辑」里把 MAC 跟在地址后面（如 {} AA:BB:CC:DD:EE:FF）",
            server.host
        ));
    }
    if !server.jump_id.is_empty() {
        return wake_via_jump(app, server, &macs);
    }
    wake_local(server, &macs)
}

/// 本机直接往局域网广播（PC 和这台机器同网段时最省事）
fn wake_local(server: &HubServer, macs: &[String]) -> Result<String, String> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .map_err(|error| format!("创建 UDP 套接字失败：{error}"))?;
    socket
        .set_broadcast(true)
        .map_err(|error| format!("开启广播失败：{error}"))?;

    let targets = auto_targets(server);
    let mut sent = 0usize;
    for mac in macs {
        let packet = magic_packet(mac)?;
        for target in &targets {
            // 9 端口是标准 WOL：按默认数量多发几个（丢一两个也能醒）；
            // 7 是老设备用的，发一个兜底就够 —— 别把局域网刷满。
            for _ in 0..crate::hub::DEFAULT_WAKE_BURST {
                if socket.send_to(&packet, (target.as_str(), 9u16)).is_ok() {
                    sent += 1;
                }
            }
            if socket.send_to(&packet, (target.as_str(), 7u16)).is_ok() {
                sent += 1;
            }
        }
    }
    if sent == 0 {
        return Err("魔术包一个都没发出去：本机可能在隔离网络 / 防火墙拦了 UDP 广播".to_string());
    }
    Ok(format!(
        "已在本机往局域网广播唤醒包（{} 个 MAC，广播地址 {}）。\
         等 10~30 秒再重连；如果 PC 和这台机器不在同一网段，请给它配一台跳板机",
        macs.len(),
        targets.join(" / ")
    ))
}

/// 经跳板机广播：目标机已经关机/睡眠，SSH 进不去，所以命令发给**跳板机**
fn wake_via_jump(app: &AppHandle, server: &HubServer, macs: &[String]) -> Result<String, String> {
    let jump = crate::hub::load_servers_pub(app)?
        .into_iter()
        .find(|candidate| candidate.id == server.jump_id)
        .ok_or_else(|| "跳板机不存在（可能已被删除），请到服务器列表检查".to_string())?;
    if jump.protocol != "ssh" {
        return Err("跳板机是 Telnet，发不了唤醒命令 —— 换一台 SSH 服务器".to_string());
    }
    let broadcast = server
        .addresses
        .first()
        .and_then(|address| split_address(address))
        .and_then(|(host, _)| subnet_broadcast(&host))
        .unwrap_or_else(|| "255.255.255.255".to_string());

    let command = wol_script(macs, &broadcast);
    let target = if jump.username.is_empty() {
        jump.host.clone()
    } else {
        format!("{}@{}", jump.username, jump.host)
    };
    let mut ssh = crate::command("ssh");
    crate::hub_tunnel::apply_auth(app, &jump, &mut ssh, NO_CRED)?;
    ssh.args(["-p", &jump.effective_port().to_string()])
        .args(["-o", "ConnectTimeout=8"])
        .arg(&target)
        .arg(command)
        .env("LC_ALL", "C.UTF-8");
    let output = ssh
        .output()
        .map_err(|error| format!("无法启动 ssh：{error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if stdout.contains("ANYBOOT-WOL-NOTOOL") {
        return Err(format!(
            "跳板机 {} 上没有 python3 / wakeonlan / etherwake —— 装一个（apt install wakeonlan）或换台跳板机",
            jump.host
        ));
    }
    if !stdout.contains("ANYBOOT-WOL-OK") {
        return Err(format!(
            "跳板机没能发出唤醒包（exit {}）：{}",
            output.status.code().unwrap_or(-1),
            tail(&format!("{stdout}\n{stderr}"), 200)
        ));
    }
    Ok(format!(
        "已通过跳板机「{}」广播唤醒包（{} 个 MAC，广播地址 {broadcast}）。等 10~30 秒再重连",
        if jump.name.is_empty() { &jump.host } else { &jump.name },
        macs.len()
    ))
}

/// 跳板机上跑的 WOL 命令：优先 python3 现拼包，退而用 wakeonlan / etherwake
fn wol_script(macs: &[String], broadcast: &str) -> String {
    let joined = macs.join(" ");
    let python = format!(
        "python3 -c \"import base64;exec(base64.b64decode('{}').decode('utf-8'))\" {} {}",
        base64_encode(PY_WOL.trim().as_bytes()),
        broadcast,
        joined
    );
    format!(
        "if command -v python3 >/dev/null 2>&1; then {python} && echo ANYBOOT-WOL-OK; \
         elif command -v wakeonlan >/dev/null 2>&1; then for m in {joined}; do wakeonlan \"$m\"; done && echo ANYBOOT-WOL-OK; \
         elif command -v etherwake >/dev/null 2>&1; then for m in {joined}; do etherwake \"$m\"; done && echo ANYBOOT-WOL-OK; \
         else echo ANYBOOT-WOL-NOTOOL; fi"
    )
}

/// 跳板机侧的 python 脚本（自己拼魔术包，不依赖装了什么）
const PY_WOL: &str = r#"
import socket, sys
targets = [sys.argv[1], '255.255.255.255']
macs = sys.argv[2:]
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
for m in macs:
    h = m.replace(':', '').replace('-', '')
    p = b'\xff' * 6 + bytes.fromhex(h) * 16
    for t in targets:
        for _ in range(10):
            try:
                s.sendto(p, (t, 9))
            except Exception:
                pass
        try:
            s.sendto(p, (t, 7))
        except Exception:
            pass
s.close()
"#;

/// 极简 base64（不引依赖）：标准字母表 + 补 `=`
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 标准魔术包：6 个 0xFF + MAC 重复 16 次
fn magic_packet(mac: &str) -> Result<Vec<u8>, String> {
    let hex: String = mac.chars().filter(|c| *c != ':' && *c != '-').collect();
    if hex.len() != 12 {
        return Err(format!("MAC 格式不对：{mac}"));
    }
    let mut packet = vec![0xFFu8; 6];
    for _ in 0..16 {
        for index in (0..12).step_by(2) {
            let byte = u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| format!("MAC 格式不对：{mac}"))?;
            packet.push(byte);
        }
    }
    Ok(packet)
}

/// IPv4 的 /24 广播地址（IPv6 / 域名给 None）
fn subnet_broadcast(host: &str) -> Option<String> {
    let ip: IpAddr = host.parse().ok()?;
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            Some(format!("{}.{}.{}.255", octets[0], octets[1], octets[2]))
        }
        IpAddr::V6(_) => None,
    }
}

fn tail(text: &str, limit: usize) -> String {
    let cleaned = text.replace(['\r', '\n'], " ");
    let trimmed = cleaned.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let skip = trimmed.chars().count() - limit;
    format!("…{}", trimmed.chars().skip(skip).collect::<String>())
}

// ==================== 单测 ====================

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> HubServer {
        let mut macs = std::collections::BTreeMap::new();
        macs.insert(
            "192.168.0.120:12300".to_string(),
            vec!["aa:bb:cc:dd:ee:ff".to_string()],
        );
        HubServer {
            id: "srv-1".to_string(),
            name: "测试".to_string(),
            protocol: "ssh".to_string(),
            host: "192.168.0.120".to_string(),
            port: 12300,
            addresses: vec!["192.168.0.120:12300".to_string()],
            macs,
            power_routes: Vec::new(),
            username: "root".to_string(),
            notes: String::new(),
            key_id: String::new(),
            password_enc: String::new(),
            jump_id: String::new(),
            term_path: String::new(),
            term_shell: String::new(),
            password_new: None,
            created_at: 0,
        }
    }

    #[test]
    fn packet_is_pure_standard_magic_packet() {
        let packet = magic_packet("aa:bb:cc:dd:ee:ff").unwrap();
        // 标准结构：6×0xFF 同步头 + MAC 连续重复 16 次，总长恰好 102 字节，
        // 一个自定义字节都不多 —— 端口即指令，包体与标准 WOL 完全一致
        assert_eq!(packet.len(), 102);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        let mac = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
        for repeat in 0..16 {
            assert_eq!(&packet[6 + repeat * 6..12 + repeat * 6], &mac);
        }
    }

    #[test]
    fn non_wake_route_without_mac_is_rejected() {
        let route = crate::hub::PowerRoute {
            action: POWER_REBOOT.to_string(),
            target: "192.168.0.255".to_string(),
            port: 11,
            macs: vec![],
            enabled: true,
            burst: 0,
            retry: 0,
            mode: String::new(),
            http_fallback: false,
        };
        let plan = plan_routes(POWER_REBOOT, &[], &[], &[route]).expect("编译路由");
        // 没配 MAC 就没有包体：这条路由进 problems，不产生投递
        assert!(plan.deliveries.is_empty());
        assert!(plan.problems.iter().any(|p| p.contains("没配 MAC")));
    }

    #[test]
    fn auto_targets_include_global_and_subnet_broadcast() {
        let targets = auto_targets(&server());
        assert!(targets.contains(&"255.255.255.255".to_string()));
        assert!(targets.contains(&"192.168.0.255".to_string()));
        // 子网广播在前：非唤醒动作只取第一个（路由器常不转发全局广播）
        assert_eq!(targets.first().map(String::as_str), Some("192.168.0.255"));
    }

    /// 「路由里填的端口」必须原样进投递任务 —— 用户怀疑端口没生效，这是证据链
    /// 的第一环：路由 port=10 → plan_routes → Delivery.port=10（发送那一环由
    /// 下面的回环收包测试覆盖：Delivery.port 是几就发到几）。
    #[test]
    fn route_port_flows_into_the_delivery() {
        let route = crate::hub::PowerRoute {
            action: POWER_SLEEP.to_string(),
            target: String::new(),
            port: 10,
            macs: vec!["aa:bb:cc:dd:ee:ff".to_string()],
            enabled: true,
            burst: 0,
            retry: 0,
            mode: String::new(),
            http_fallback: false,
        };
        let plan = plan_routes(
            POWER_SLEEP,
            &["192.168.0.131:12300".to_string()],
            &["aa:bb:cc:dd:ee:ff".to_string()],
            &[route],
        )
        .expect("编译路由");
        assert_eq!(plan.deliveries.len(), 1);
        assert_eq!(plan.deliveries[0].port, 10);
        assert_eq!(plan.deliveries[0].target, "192.168.0.255");
        // 非唤醒：单包，包体也是纯标准魔术包（端口才是指令）
        assert_eq!(plan.deliveries[0].packet.len(), 102);
        assert_eq!(&plan.deliveries[0].packet[..6], &[0xFF; 6]);
    }

    /// 真发一次 UDP：确认「构造的包」和「发送路径」是通的（在本机回环上收）
    #[test]
    fn local_delivery_reaches_a_listening_socket() {
        let listener = UdpSocket::bind("127.0.0.1:0").expect("监听回环");
        listener
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .expect("设超时");
        let port = listener.local_addr().expect("本地地址").port();
        let packet = magic_packet("aa:bb:cc:dd:ee:ff").expect("造包");
        let deliveries = vec![Delivery {
            target: "127.0.0.1".to_string(),
            port,
            packet: packet.clone(),
            times: 1,
            retry: 0,
        }];
        assert_eq!(send_local(&deliveries).expect("发送"), 1);

        let mut buffer = [0u8; 512];
        let (count, _) = listener.recv_from(&mut buffer).expect("收到包");
        assert_eq!(&buffer[..count], packet.as_slice());
        assert_eq!(count, 102);
    }

    /// 一次唤醒要把 burst 个包都发出去（默认 10）—— 这里用回环收全部
    #[test]
    fn wake_burst_sends_every_packet() {
        let listener = UdpSocket::bind("127.0.0.1:0").expect("监听回环");
        listener
            .set_read_timeout(Some(std::time::Duration::from_millis(600)))
            .expect("设超时");
        let port = listener.local_addr().expect("本地地址").port();
        let packet = magic_packet("aa:bb:cc:dd:ee:ff").expect("造包");
        let deliveries = vec![Delivery {
            target: "127.0.0.1".to_string(),
            port,
            packet,
            times: 10,
            retry: 0,
        }];
        assert_eq!(send_local(&deliveries).expect("发送"), 10);

        let mut buffer = [0u8; 512];
        let mut received = 0;
        while listener.recv_from(&mut buffer).is_ok() {
            received += 1;
        }
        assert_eq!(received, 10);
    }
}
