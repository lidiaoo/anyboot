//! root 助手的安装 / 卸载 / 调用（macOS）。
//!
//! 目的只有一个：让「设为默认」不再每次都弹密码框。装的时候弹一次（写 `/Library`
//! 必须 root），之后 App 把请求丢进文件信箱，由 launchd 唤起 root 助手去写 NVRAM。
//!
//! 为什么不用 SMAppService / SMJobBless（macOS 的「正规」做法）：
//! 它们要求 App 与助手都用**带 Team ID 的证书**签名；本项目目前是 ad-hoc 签名
//! （`Signature=adhoc`、无 TeamIdentifier），注册必然失败。别把这段改成 SMAppService
//! 除非先解决签名身份 —— 否则会出现「装不上但看不出原因」。
//!
//! 代价写在 UI 里也写在文档里：助手装好后，任何以当前用户身份运行的进程都能让它
//! 改 OpenCore 的默认启动项。它不接受任意变量名、不执行 shell，所以拿不到 root；
//! 但「默认启动项被改」这个后果是真的。不想承担就别开。

use std::path::{Path, PathBuf};

use crate::helper_proto::{self, Op, Reply, Request};

pub(crate) const LABEL: &str = "com.example.anyboot.helper";
pub(crate) const HELPER_PATH: &str = "/Library/PrivilegedHelperTools/com.example.anyboot.helper";
pub(crate) const PLIST_PATH: &str = "/Library/LaunchDaemons/com.example.anyboot.helper.plist";

/// 等助手回结果的时长。launchd 唤起一次通常 <300ms；4 秒是「慢机器 + 冷启动」的
/// 上限，超过就不是慢了，是没装上或被拦了 —— 那时候要退回密码框，不能让用户干等。
const REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(4000);

/// 读 Linux 分区的超时。真机十块分区、每块读 fstab/cfg/os-release，实测 2.3 秒；
/// 但那是暖缓存。冷启动 + 十块盘的 inode 寻道可以到十几秒，25 秒是「还没回就是坏了」。
/// **不能用 [`REPLY_TIMEOUT`]**：那样一块大分区就能让整个扫描被判成「助手没响应」。
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(25_000);

const REPLY_POLL: std::time::Duration = std::time::Duration::from_millis(40);

/// 调用助手时的两类失败。区别很重要：
/// - **Unavailable**：压根没通路（没装、被删了、超时）→ 退回密码框，功能仍可用；
/// - **Rejected**：助手明确拒绝或 `nvram` 报错 → 再弹一次密码只会再失败一次，
///   直接如实报错（用户要看到真正的失败原因，而不是一个密码框）。
#[derive(Debug)]
pub(crate) enum HelperError {
    Unavailable(String),
    Rejected(String),
}

impl HelperError {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Unavailable(text) | Self::Rejected(text) => text,
        }
    }
}

/// 前端那条「免密模式」要显示的全部信息。
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HelperView {
    /// 这个平台有没有这套东西（现在只有 macOS 有）
    pub(crate) supported: bool,
    /// 助手已安装（文件在位）
    pub(crate) installed: bool,
    /// 装的那份和当前 App 不是同一个构建（App 更新过）。
    /// 按**内容**比（sha256），不看大小 —— 见 [`helper_is_stale`]。
    ///
    /// **只用来提示「建议重装」，不再当门禁**：真正决定能不能用的是协议版本
    /// （请求里的 `v`，助手侧 `helper_proto::validate` 对不上会直接拒绝）。
    /// 以前拿它当门禁的后果是「App 重新构建一次，所有免密读取都退回弹密码」。
    pub(crate) stale: bool,
    /// 一句话现状：为什么没开 / 开了以后少什么
    pub(crate) text: String,
}

/// `/Library` 里那份助手是不是**当前这个**构建。
///
/// 装过去的是 App 可执行文件本身，所以「App 升级了而助手没换」是必然会发生的事：
/// 助手仍按旧代码校验、旧协议工作。这条判断因此**不能只比文件大小**：
/// macOS 的可执行文件有页对齐与代码签名，改几十字节的代码，文件长度很可能一字节不差
/// （`LC_CODE_SIGNATURE` 只在跨页时才变）。而「助手是旧的」这次的后果恰恰是
/// 「写进去了、回执 ok、其实写到了另一个变量」—— 判断不出来就等于没修。
///
/// 所以比内容：sha256。25 MB 读一次约几十毫秒，`view()` 每次快照都会调，
/// 于是按 (长度, mtime) 缓存 —— 文件没变时只做一次 metadata。
#[cfg(target_os = "macos")]
fn helper_is_stale() -> bool {
    let helper = Path::new(HELPER_PATH);
    match (installed_digest(helper), current_digest()) {
        (Some(installed), Some(current)) => installed != current,
        // 查不到就别说过期（宁可少提示，也不要误报让用户去重装）
        _ => false,
    }
}

/// 安装副本的内容指纹，按 (长度, mtime) 缓存。
#[cfg(target_os = "macos")]
fn installed_digest(path: &Path) -> Option<String> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<Option<(u64, Option<std::time::SystemTime>, String)>>> =
        OnceLock::new();
    let meta = path.metadata().ok()?;
    let key = (meta.len(), meta.modified().ok());
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((len, modified, digest)) = guard.as_ref() {
            if (*len, *modified) == key {
                return Some(digest.clone());
            }
        }
    }
    let digest = file_digest(path)?;
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((key.0, key.1, digest.clone()));
    }
    Some(digest)
}

/// 本进程自己的指纹：进程生命周期内不会变，算一次就够。
#[cfg(target_os = "macos")]
fn current_digest() -> Option<String> {
    use std::sync::OnceLock;
    static DIGEST: OnceLock<Option<String>> = OnceLock::new();
    DIGEST
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            file_digest(&exe)
        })
        .clone()
}

#[cfg(target_os = "macos")]
fn file_digest(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(hex::encode(hasher.finalize()))
}

/// 读状态：只做文件检查。
///
/// 每次快照都会调一次，所以**不能**在这里试探性写一次 NVRAM 去「探活」——
/// 那等于每次刷新都往固件里写一遍。真正的可用性由 [`request`] 的超时来判定。
pub(crate) fn view() -> HelperView {
    #[cfg(target_os = "macos")]
    {
        let installed = Path::new(HELPER_PATH).is_file() && Path::new(PLIST_PATH).is_file();
        let stale = installed && helper_is_stale();
        HelperView {
            supported: true,
            installed,
            stale,
            text: if stale {
                // 不用 markdown 加粗：前端只把反引号渲染成 <code>，`**` 会原样露出来
                // 注意：**过期不等于不能用**（见 linux_grub::helper_ready 的说明）。
                // 同协议、不同构建照常免密，这里只是提示「想保持一致可以重装」。
                "已开启：`/Library` 里那份是上一个构建装的，协议一致，照常免密。\
                 想消掉这条提示就点「重装」"
                    .to_string()
            } else if installed {
                "已开启：由 root 助手写 `OCBt*`，不再询问密码。卸载后恢复成每次输入密码"
                    .to_string()
            } else {
                "未开启：每次「设为默认」都要输一次管理员密码。开启会在 `/Library` 装一个只在有请求时才运行的 root 助手（安装时需要一次密码，之后免密）"
                    .to_string()
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        HelperView {
            supported: false,
            installed: false,
            stale: false,
            text: "这个平台不需要额外助手".to_string(),
        }
    }
}

pub(crate) fn installed() -> bool {
    view().installed
}

/// 把一批变量写入交给 root 助手。
///
/// 成功 = 助手回了 `ok`；**不做「写完再回读」的自证**（回读由上层按需做，
/// 一次性写完两条变量后读一次 `OCBtOrder` 更有意义）。
#[cfg(target_os = "macos")]
pub(crate) fn request(entries: &[(&str, &str, &[u8])]) -> Result<(), HelperError> {
    // 「装没装」由调用方先判（见 channels::nvram）：这里不再重复判一次，
    // 否则这条客户端逻辑就没法在临时信箱里跑测试了（测试没有 /Library 的写入权）。
    let ops: Vec<Op> = entries
        .iter()
        .map(|(name, guid, value)| Op {
            guid: guid.to_string(),
            name: name.to_string(),
            hex: hex::encode(value),
        })
        .collect();
    send(&Request::new(ops), REPLY_TIMEOUT).map(|_| ())
}

/// 让 root 助手枚举 ext4 分区并读 fstab / grub.cfg / grubenv。返回整包 JSON 字节。
///
/// 返回的是**原始字节**而不是解析好的结构：协议层不该知道 `ProbePayload` 长什么样，
/// 解析留在 `channels::linux_grub`。
#[cfg(target_os = "macos")]
pub(crate) fn linux_probe(esp: &str) -> Result<Vec<u8>, HelperError> {
    let (_, blob) = send(&Request::linux_probe(esp), PROBE_TIMEOUT)?;
    blob.ok_or_else(|| {
        HelperError::Unavailable("免密助手回了成功，但没有附探测结果".to_string())
    })
}

/// 让 root 助手把 1024 字节环境块覆盖回 `dev` 的 `rel`。
#[cfg(target_os = "macos")]
pub(crate) fn linux_write_env(
    dev: &str,
    rel: &str,
    block: &[u8],
) -> Result<String, HelperError> {
    let (reply, _) = send(&Request::linux_write_env(dev, rel, block), REPLY_TIMEOUT)?;
    Ok(reply.detail.unwrap_or_else(|| "已写入".to_string()))
}

/// 投递一个请求并等回执。返回 `(回执, 载荷)`。
///
/// 载荷（`blob`）由助手先落盘、再落回执，所以**看到回执就等于载荷完整** ——
/// 这里的读文件不需要任何「等它写完」的重试。
#[cfg(target_os = "macos")]
fn send(
    request: &Request,
    timeout: std::time::Duration,
) -> Result<(Reply, Option<Vec<u8>>), HelperError> {
    // 校验放在本地也走一遍：请求格式错了就别去打扰 root 进程，
    // 而且能把错误说成「AnyBoot 要写的变量名不在允许范围」这种人话。
    if let Err(error) = helper_proto::validate(request) {
        return Err(HelperError::Rejected(error));
    }
    let json = request
        .to_json()
        .map_err(HelperError::Unavailable)?;
    let name = request.file_name();
    let stem = name.trim_end_matches(".json").to_string();

    deliver(&name, &json)?;

    let deadline = std::time::Instant::now() + timeout;
    let reply_path = Request::outbox_dir().join(format!("{stem}.json"));
    loop {
        if let Ok(bytes) = std::fs::read(&reply_path) {
            let _ = std::fs::remove_file(&reply_path);
            let reply: Reply = serde_json::from_slice(&bytes).map_err(|error| {
                HelperError::Rejected(format!("助手的回复看不懂（{error}）"))
            })?;
            if !reply.ok {
                return Err(HelperError::Rejected(
                    reply
                        .error
                        .unwrap_or_else(|| "助手拒绝了这次写入，但没给原因".to_string()),
                ));
            }
            let blob = match reply.blob.as_deref() {
                Some(name) => {
                    // 载荷名由助手给，但它只会是 `<stem>.blob`；仍然过一遍形状，
                    // 免得一个被篡改的回执把读取引到别处
                    if name.contains('/') || name.contains("..") {
                        return Err(HelperError::Rejected(format!(
                            "助手给的结果文件名不合法：{name}"
                        )));
                    }
                    let path = Request::outbox_dir().join(name);
                    let bytes = std::fs::read(&path).map_err(|error| {
                        HelperError::Rejected(format!(
                            "回执说有结果载荷，但读不到（{}）：{error}",
                            path.display()
                        ))
                    })?;
                    let _ = std::fs::remove_file(&path);
                    Some(bytes)
                }
                None => None,
            };
            return Ok((reply, blob));
        }
        if std::time::Instant::now() >= deadline {
            // 还没被处理就把请求撤掉：否则助手十分钟后才醒来，会在用户完全不知情的
            // 情况下把启动项改掉 —— 那种「鬼写」比失败更难排查。
            let _ = std::fs::remove_file(Request::inbox_dir().join(&name));
            return Err(HelperError::Unavailable(format!(
                "免密助手在 {} 秒内没有响应（launchd 作业可能没加载，或助手被系统拦下了）",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(REPLY_POLL);
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn request(_entries: &[(&str, &str, &[u8])]) -> Result<(), HelperError> {
    Err(HelperError::Unavailable("这个平台没有免密助手".to_string()))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn linux_probe(_esp: &str) -> Result<Vec<u8>, HelperError> {
    Err(HelperError::Unavailable("这个平台没有免密助手".to_string()))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn linux_write_env(
    _dev: &str,
    _rel: &str,
    _block: &[u8],
) -> Result<String, HelperError> {
    Err(HelperError::Unavailable("这个平台没有免密助手".to_string()))
}

/// 落盘请求：先写 `<spool>/tmp`，再 rename 进 inbox。
///
/// rename 是原子的：助手绝不会读到「写了一半的 JSON」。直接写进 inbox 就会出现
/// 这种时序 —— spawn 的助手比 `write()` 快，读到半个文件，回一条「不是合法 JSON」。
#[cfg(target_os = "macos")]
fn deliver(name: &str, json: &str) -> Result<(), HelperError> {
    let inbox = Request::inbox_dir();
    let staging = Request::staging_dir();
    let temp = staging.join(name);
    if std::fs::write(&temp, json).is_ok() {
        if std::fs::rename(&temp, inbox.join(name)).is_ok() {
            return Ok(());
        }
        let _ = std::fs::remove_file(&temp);
    }
    // 暂存目录不可用（老版本装的助手没有 tmp/）：退回直写 inbox
    std::fs::write(inbox.join(name), json).map_err(|error| {
        HelperError::Unavailable(format!(
            "写不进免密助手的信箱（{}）：{error}。重新开启一次免密模式可修复",
            inbox.display()
        ))
    })
}

// ─────────────────────────── 安装 / 卸载 ───────────────────────────

/// 安装来源 = **App 自己的可执行文件**。
///
/// 为什么不做成独立的小二进制再打包进来：Tauri 的 `externalBin` 在**编译期**就校验
/// 文件存在，而那个文件又得由同一个 crate 编出来 —— 首次构建必然死锁（实测报
/// `resource path binaries/anyboot-helper-… doesn't exist`）。而把产物提交进仓库，
/// 只会换来「源码改了、/Library 里还是旧助手」这种更难发现的错。
///
/// 同一份二进制两种模式（`main.rs` 第一行看 `--run-inbox` 就知道该走哪条）：
/// 好处是永远不版本错配；代价是 `/Library` 里多一份 ~25 MB 的副本，
/// 且这份副本以 root 跑时执行的是同一个可执行文件（但只走信箱处理那一小段）。
#[cfg(target_os = "macos")]
fn helper_source() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| format!("取不到自身路径：{error}"))?;
    if !exe.is_file() {
        return Err(format!("自身可执行文件不存在：{}", exe.display()));
    }
    Ok(exe)
}

/// launchd 作业：**只在 inbox 有变化时**被唤起，不常驻。
///
/// `WatchPaths` 指向目录：向目录里 rename 一个新文件会让 kqueue 报 NOTE_WRITE，
/// 正好触发。`RunAtLoad=false` 保证开机不会白跑一次。
#[cfg(target_os = "macos")]
fn launchd_plist(inbox: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{HELPER_PATH}</string>
        <string>--run-inbox</string>
    </array>
    <key>WatchPaths</key>
    <array>
        <string>{inbox}</string>
    </array>
    <key>ThrottleInterval</key>
    <integer>1</integer>
    <key>RunAtLoad</key>
    <false/>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
"#
    )
}

#[cfg(target_os = "macos")]
fn run_as_admin(script_path: &Path) -> Result<(), String> {
    // `do shell script` 是把密码框和 root 权限绑在一起的唯一可用通道（见模块注释）。
    // 脚本路径由我们自己生成（ASCII、无空格），不需要再转义。
    let applescript = format!(
        "do shell script \"/bin/sh {}\" with administrator privileges",
        script_path.display()
    );
    let output = crate::command("osascript")
        .args(["-e", applescript.as_str()])
        .output()
        .map_err(|error| format!("无法调用 osascript：{error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.contains("-128") || stderr.contains("User canceled") || stderr.contains("用户已取消") {
        return Err("已取消：没有做任何改动".to_string());
    }
    Err(if stderr.is_empty() {
        "管理员授权失败".to_string()
    } else {
        format!("管理员授权失败：{stderr}")
    })
}

#[cfg(target_os = "macos")]
fn staging_dir() -> Result<PathBuf, String> {
    let mut nonce = [0u8; 6];
    let _ = getrandom::fill(&mut nonce);
    let path = std::env::temp_dir().join(format!("anyboot-helper-{}", hex::encode(nonce)));
    std::fs::create_dir_all(&path).map_err(|error| format!("建暂存目录失败：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
    }
    Ok(path)
}

/// 安装脚本正文（纯函数，便于用 `sh -n` 与断言验证）。
///
/// 四条容易写错的细节，都在这里固定下来：
/// 1. `in/` 必须是 **0733**：用户要能在里面新建请求文件，但不能列举别人的；
/// 2. `out/` 也是 0733：回执由 root 写（0644）、用户读；目录若给 0755，
///    用户就读得到却**删不掉**自己的回执，`out/` 会一直堆着历史文件
///    （排查「写了却没生效」时这些残留会误导判断）；
/// 3. 先 `bootout` 再 `bootstrap`：重复安装（换版本重开）时作业已经加载，
///    直接 bootstrap 会报 "service already loaded" 然后整条 `set -e` 挂掉；
/// 4. `bootstrap` 必须用**绝对路径**指向 plist，抄相对路径会去错目录找。
#[cfg(target_os = "macos")]
fn install_script(stage: &Path, plist: &Path) -> String {
    format!(
        r#"#!/bin/sh
set -e
umask 022
/usr/bin/install -d -m 0755 -o root -g wheel /Library/PrivilegedHelperTools
/usr/bin/install -m 0755 -o root -g wheel "{stage}/anyboot-helper" "{helper}"
/usr/bin/xattr -c "{helper}" 2>/dev/null || true
/usr/bin/install -d -m 0755 -o root -g wheel "{spool}"
/usr/bin/install -d -m 0733 -o root -g wheel "{spool}/in"
/usr/bin/install -d -m 0733 -o root -g wheel "{spool}/out"
/usr/bin/install -d -m 0733 -o root -g wheel "{spool}/tmp"
/usr/bin/install -m 0644 -o root -g wheel "{plist}" "{target_plist}"
/bin/launchctl bootout system/{label} 2>/dev/null || true
/bin/launchctl bootstrap system "{target_plist}"
"#,
        stage = stage.display(),
        helper = HELPER_PATH,
        spool = helper_proto::spool_root().display(),
        plist = plist.display(),
        target_plist = PLIST_PATH,
        label = LABEL,
    )
}

/// 卸载脚本正文（纯函数）：先卸作业再删文件，否则 launchd 会拿着已删除的程序保留
/// 一个 "spawn failed" 的作业，状态一直脏着。
#[cfg(target_os = "macos")]
fn uninstall_script() -> String {
    format!(
        r#"#!/bin/sh
/bin/launchctl bootout system/{label} 2>/dev/null || true
/bin/rm -f "{plist}"
/bin/rm -f "{helper}"
/bin/rm -rf "{spool}"
exit 0
"#,
        label = LABEL,
        plist = PLIST_PATH,
        helper = HELPER_PATH,
        spool = helper_proto::spool_root().display(),
    )
}

/// 安装（一次密码）。写进 `/Library` 的三样东西：助手本体、launchd 作业、信箱目录。
#[cfg(target_os = "macos")]
pub(crate) fn install() -> Result<String, String> {
    let source = helper_source()?;
    let stage = staging_dir()?;
    let result = install_in(&stage, &source);
    let _ = std::fs::remove_dir_all(&stage);
    result
}

#[cfg(target_os = "macos")]
fn install_in(stage: &Path, source: &Path) -> Result<String, String> {
    let staged_helper = stage.join("anyboot-helper");
    std::fs::copy(source, &staged_helper).map_err(|error| format!("复制助手失败：{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged_helper, std::fs::Permissions::from_mode(0o755));
    }

    let inbox = Request::inbox_dir();
    let plist = stage.join(format!("{LABEL}.plist"));
    std::fs::write(&plist, launchd_plist(&inbox.to_string_lossy()))
        .map_err(|error| format!("写 launchd 描述失败：{error}"))?;

    // 安装脚本：路径全部写死，不接受任何外部输入 —— 它是以 root 身份跑的。
    // `install` 不会带 xattr，这里仍显式 `xattr -c`：dmg 下载来的包可能带 quarantine。
    let script = install_script(stage, &plist);
    let script_path = stage.join("install.sh");
    std::fs::write(&script_path, script).map_err(|error| format!("写安装脚本失败：{error}"))?;

    run_as_admin(&script_path)?;

    // 提权子进程的退出码不可靠（`do shell script` 只报「脚本是否非零退出」，
    // 而 bootout 的失败被 `|| true` 吃掉了），所以按结果核对：文件都在才算成。
    if !Path::new(HELPER_PATH).is_file() {
        return Err(format!("安装没有落到位：{HELPER_PATH} 不存在"));
    }
    if !Path::new(PLIST_PATH).is_file() {
        return Err(format!("安装没有落到位：{PLIST_PATH} 不存在"));
    }
    Ok("免密模式已开启：之后「设为默认」不再询问密码".to_string())
}

/// 卸载（一次密码）：删作业、删助手、删信箱。
#[cfg(target_os = "macos")]
pub(crate) fn uninstall() -> Result<String, String> {
    let stage = staging_dir()?;
    let result = uninstall_in(&stage);
    let _ = std::fs::remove_dir_all(&stage);
    result
}

#[cfg(target_os = "macos")]
fn uninstall_in(stage: &Path) -> Result<String, String> {
    let script = uninstall_script();
    let script_path = stage.join("uninstall.sh");
    std::fs::write(&script_path, script).map_err(|error| format!("写卸载脚本失败：{error}"))?;
    run_as_admin(&script_path)?;
    if Path::new(PLIST_PATH).exists() || Path::new(HELPER_PATH).exists() {
        return Err("卸载没有完成：/Library 里仍有文件（可能被别的进程占用了）".to_string());
    }
    Ok("免密模式已关闭：之后会恢复成每次输入密码".to_string())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn install() -> Result<String, String> {
    Err("这个平台没有免密助手".to_string())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn uninstall() -> Result<String, String> {
    Err("这个平台没有免密助手".to_string())
}

/// 前端「开启 / 关闭免密」的唯一入口。
#[tauri::command]
pub(crate) fn boot_helper(action: String) -> Result<String, String> {
    match action.as_str() {
        "install" => install(),
        "uninstall" => uninstall(),
        other => Err(format!("未知的助手动作: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_matches_files_on_this_machine() {
        let view = view();
        // 这台开发机上没装过：断言「未装」时文案必须点明代价与收益，
        // 因为用户就是靠这句话决定要不要开。
        //
        // 只在 macOS 断言文案：那句话（`每次…root 助手`）是 macOS 分支写的，
        // Windows / Linux 的 `view()` 是「不支持」的桩，文案完全不同 ——
        // 原来这条断言没加平台门，在 Windows 上一直是红的，红久了就没人看测试了。
        if !view.installed {
            #[cfg(target_os = "macos")]
            {
                assert!(view.text.contains("每次"));
                assert!(view.text.contains("root"));
            }
        }
        assert_eq!(view.installed, Path::new(HELPER_PATH).is_file() && Path::new(PLIST_PATH).is_file());
    }

    #[test]
    fn launchd_job_only_wakes_on_inbox_changes() {
        #[cfg(target_os = "macos")]
        {
            let text = launchd_plist("/var/spool/anyboot/in");
            // 常驻是浪费：只要 WatchPaths 触发
            assert!(text.contains("<key>WatchPaths</key>"));
            assert!(text.contains("<key>RunAtLoad</key>\n    <false/>"));
            assert!(text.contains(HELPER_PATH));
            assert!(text.contains("--run-inbox"));
            assert!(text.contains(LABEL));

            // 结构对不对只能靠 plutil：launchd 读到坏 plist 不会报错给你，
            // 只会在系统日志里失败，用户在界面上看到的只会是「助手没响应」。
            let path = std::env::temp_dir().join(format!(
                "anyboot-plist-{}.plist",
                std::process::id()
            ));
            std::fs::write(&path, &text).unwrap();
            let output = std::process::Command::new("/usr/bin/plutil")
                .arg("-lint")
                .arg(&path)
                .output()
                .expect("plutil 应该存在");
            let _ = std::fs::remove_file(&path);
            assert!(
                output.status.success(),
                "plist 结构不合法：{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// 安装 / 卸载脚本只能在 root 下真跑（要密码），所以在这里固定两件能测的事：
    /// 语法（`sh -n`）与关键顺序（先 bootout 再 bootstrap）。
    #[test]
    fn scripts_are_valid_and_ordered() {
        #[cfg(target_os = "macos")]
        {
            let stage = Path::new("/tmp/anyboot-helper-unit");
            let install = install_script(stage, &stage.join("x.plist"));
            let uninstall = uninstall_script();
            sh_syntax_ok("install", &install);
            sh_syntax_ok("uninstall", &uninstall);

            // in/ 是用户写请求的地方：可写但不可列举（0733）
            assert!(install.contains("-d -m 0733 -o root -g wheel \"/var/spool/anyboot/in\""));
            // out/ 同样 0733：给 0755 用户就删不掉自己的回执，目录会越积越多
            assert!(install.contains("-d -m 0733 -o root -g wheel \"/var/spool/anyboot/out\""));
            // 重复开启免密时作业已经加载：不先 bootout，bootstrap 会报 already loaded
            let bootout = install.find("bootout").expect("install 要先 bootout");
            let bootstrap = install.find("bootstrap system").expect("install 要 bootstrap");
            assert!(bootout < bootstrap);
            // 卸载先卸作业再删文件，否则 launchd 会留一个 spawn failed 的脏作业
            assert!(uninstall.find("bootout").unwrap() < uninstall.find("rm -f").unwrap());
            assert!(uninstall.contains(PLIST_PATH) && uninstall.contains(HELPER_PATH));
        }
    }

    #[cfg(target_os = "macos")]
    fn sh_syntax_ok(name: &str, script: &str) {
        let path = std::env::temp_dir().join(format!(
            "anyboot-syntax-{name}-{}.sh",
            std::process::id()
        ));
        std::fs::write(&path, script).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .arg("-n")
            .arg(&path)
            .output()
            .expect("/bin/sh 应该存在");
        let _ = std::fs::remove_file(&path);
        assert!(
            output.status.success(),
            "{name} 脚本语法错误：{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn rejected_requests_never_become_a_password_prompt() {
        // 分类错了就会「失败一次 → 又弹密码 → 又失败」，用户完全看不出真正原因
        let rejected = HelperError::Rejected("只允许写 OC 私有命名空间".to_string());
        match rejected {
            HelperError::Rejected(message) => assert!(message.contains("私有命名空间")),
            HelperError::Unavailable(_) => panic!("明确拒绝不能当成「没装」"),
        }
    }

    /// 真跑一遍「客户端 → 信箱 → 助手 → 回结果」，用的是**本程序自身**的
    /// `--run-inbox` 模式（与装到 /Library 的那份走的是同一条代码路径）。
    ///
    /// 默认 ignore（会 set_var 污染进程环境，跟并行测试互相打架）：
    /// `cargo test --lib -- --ignored helper_roundtrip_with_real_binary`
    ///
    /// 这条测试覆盖的正是单元测试碰不到的那段：rename 投递、轮询回执、超时撤回。
    /// 它仍然**不**覆盖 root 下真正写 NVRAM 那一步（dry-run 跳过了），
    /// 那一步只能在装好助手的机器上验。
    #[test]
    #[ignore]
    fn helper_roundtrip_with_real_binary() {
        #[cfg(not(target_os = "macos"))]
        {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::Arc;

            let root = std::env::temp_dir().join(format!("anyboot-rt-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for dir in ["in", "out", "tmp"] {
                std::fs::create_dir_all(root.join(dir)).unwrap();
            }
            std::env::set_var(helper_proto::SPOOL_ENV, &root);
            std::env::set_var("ANYBOOT_HELPER_DRY_RUN", "1");

            // 测试二进制在 target/<triple>/debug/deps/ 下，主程序在它的上一级
            let exe = std::env::current_exe().unwrap();
            let helper = exe
                .parent()
                .and_then(|deps| deps.parent())
                .map(|dir| dir.join("anyboot"))
                .expect("测试二进制路径异常");
            assert!(
                helper.is_file(),
                "先 cargo build --bins（缺 {}）",
                helper.display()
            );

            let stop = Arc::new(AtomicBool::new(false));
            let inbox = helper_proto::Request::inbox_dir();
            let worker = {
                let stop = stop.clone();
                let helper = helper.clone();
                let inbox = inbox.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::SeqCst) {
                        let pending = std::fs::read_dir(&inbox)
                            .map(|mut entries| entries.next().is_some())
                            .unwrap_or(false);
                        if pending {
                            let _ = std::process::Command::new(&helper)
                                .arg("--run-inbox")
                                .output();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(40));
                    }
                })
            };

            let result = request(&[(
                "OCBtOrder",
                helper_proto::OC_GUID,
                &[0x82, 0x00, 0x81, 0x00],
            )]);
            stop.store(true, Ordering::SeqCst);
            let _ = worker.join();

            assert!(result.is_ok(), "{}", result.unwrap_err().message());
            // 请求必须被助手清掉（否则下次开机还会再写一遍）
            let left: Vec<_> = std::fs::read_dir(&inbox)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .collect();
            assert!(left.is_empty(), "inbox 里还剩 {} 个文件", left.len());
            assert!(root.join("helper.log").is_file(), "助手应留下处理日志");

            std::env::remove_var("ANYBOOT_HELPER_DRY_RUN");
            std::env::remove_var(helper_proto::SPOOL_ENV);
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
