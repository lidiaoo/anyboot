//! Linux 免密提权（sudoers 方案）：**一次授权，之后不再弹密码框**；卸载时能一键撤掉。
//!
//! 只有三件东西，全都可以一句话删干净（这就是选 sudoers 而不是常驻服务的原因）：
//!
//! | 路径 | 作用 | 谁写 |
//! |---|---|---|
//! | `/usr/lib/anyboot/anyboot-priv` | 本程序的副本，root 拥有 0755。特权动作都由它执行 | 安装时由 root 复制 |
//! | `/etc/sudoers.d/anyboot` | 一行规则：`<装机的人> ALL=(root) NOPASSWD: <上面那个路径>` | 同上 |
//! | `/var/lib/anyboot/priv.json` | 装给谁、什么版本（0644，普通用户也能读，用于界面显示状态） | 同上 |
//!
//! 为什么授权的是**我们自己的助手**、不是 `install` / `mount` / `mount` 这类系统命令：
//! sudoers 只按「命令 + 参数模式」授权。授权 `install` 等于把「任意文件复制到任意位置」
//! 放出去（拼一个 `install -m 4755 x /usr/bin/su` 就是 root 后门）。授权助手时，
//! 能做什么由助手自己的参数白名单决定，边界收在一个我们能 review 的地方。
//!
//! 三条铁律：
//! 1. **用户名先过白名单**再拼进 sudoers（[`is_valid_user_name`]）—— 带换行的用户名
//!    能往 sudoers 里塞任意规则；
//! 2. 写之前必须 `visudo -cf` 校验，写坏 `/etc/sudoers.d/` 会让整机 sudo 瘫痪；
//! 3. 临时文件**不能**放在 `/etc/sudoers.d/` 里：该目录下所有不以 `~` 结尾的文件
//!    **都会被 include**，`.tmp` / `.bak` 一样会被当成规则解析。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 助手（本程序副本）的固定路径。`root` 拥有，普通用户不可写 —— 这是它敢进 sudoers 的前提。
pub(crate) const HELPER_PATH: &str = "/usr/lib/anyboot/anyboot-priv";
/// sudoers 里的授权文件。卸载 = 删掉它。
pub(crate) const SUDOERS_PATH: &str = "/etc/sudoers.d/anyboot";
/// 元数据目录（与 sudoers 同生共死；留空目录无害，但也会尽量删掉）。
pub(crate) const STATE_DIR: &str = "/var/lib/anyboot";
/// 安装记录：给谁装的、哪个版本。
pub(crate) const META_PATH: &str = "/var/lib/anyboot/priv.json";

/// 特权子命令的「结果文件」只允许落在这里。
///
/// 子命令是 root 跑的，`--out` 由调用方给 —— 不限制的话它就是个「以 root 身份往
/// 任意路径写一段固定 JSON」的原语（覆盖 `/etc/cron.d/x` 之类）。固定成 `/tmp`
/// 两边都不用猜对方的 `$TMPDIR`。
pub(crate) const SCRATCH_DIR: &str = "/tmp";

/// 校验 `--out`：必须是绝对路径、直接位于 [`SCRATCH_DIR`] 下、且不含 `..` / 符号链接。
///
/// 三道闸都是有原因的，不是洁癖：
/// - `/tmp/../etc/cron.d/x` 用 `starts_with` 判断是能过的（组件比较时 `..` 只是个普通组件）；
/// - `/tmp/sub/x` 这种子目录可以被别人换成指向 `/etc` 的符号链接，然后我们跟着写；
/// - 最终文件名是我们自己起的、可预测（含 pid），别人能抢先在 `/tmp` 里建同名符号链接。
pub(crate) fn guard_result_path(out: &str) -> Result<(), String> {
    let path = Path::new(out);
    if !path.is_absolute() {
        return Err(format!("结果文件要绝对路径：{out}"));
    }
    if path.components().any(|part| part.as_os_str() == "..") {
        return Err(format!("结果文件路径不许带 ..：{out}"));
    }
    if path.parent() != Some(Path::new(SCRATCH_DIR)) {
        return Err(format!("结果文件要直接放在 {SCRATCH_DIR} 下：{out}"));
    }
    // 已经存在时多半是上一次崩溃残留、或者别人抢注的符号链接 —— 两种都不接
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            return Err(format!("结果文件路径是符号链接：{out}"));
        }
    }
    Ok(())
}

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 界面要用的一份状态（camelCase 直接进前端）。
#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrivStatus {
    /// 只有 Linux 有这套东西；另外两端走 UAC / launchd 助手。
    pub platform: bool,
    /// `ready` | `outdated` | `stale` | `absent` | `unsupported`
    pub state: String,
    /// 一句话说明，直接显示在界面上
    pub detail: String,
    pub helper_path: String,
    pub sudoers_path: String,
    pub helper_exists: bool,
    pub sudoers_exists: bool,
    /// 已装的助手版本（没装就是空）
    pub version: String,
    /// 当前程序版本（用来判 `outdated`）
    pub current_version: String,
    /// sudoers 里授权给了谁
    pub user: String,
    /// 有 pkexec（能弹系统授权框）／有 sudo（能密码兜底）／当前用户在 sudo 组
    pub has_pkexec: bool,
    pub has_sudo: bool,
    pub is_sudoer: bool,
}

/// 安装记录（`priv.json`）。字段少是刻意的：这份文件普通用户可读，别放任何凭据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrivMeta {
    pub user: String,
    pub version: String,
    pub helper: String,
    pub installed_at: String,
}

/// sudoers 文件的完整内容。**用户名必须先过 [`is_valid_user_name`]**。
pub(crate) fn sudoers_text(user: &str) -> String {
    format!(
        "# AnyBoot 免密提权 —— 由 AnyBoot 自动写入，卸载 AnyBoot 时请一并删除本文件。\n\
         # 只授权运行固定路径的助手程序（它能做什么由助手自己的参数白名单决定）。\n\
         {user} ALL=(root) NOPASSWD: {HELPER_PATH}\n"
    )
}

/// sudoers 里能安全写下的用户名：字母数字加 `._-`，不以 `-` 开头，长度合理。
///
/// sudoers 是「一行一条规则」的格式，用户名里只要有空白、逗号、换行、`!`，就能把
/// 我们的授权改写成完全另一条规则 —— 所以宁可不支持怪名字，也不能放过。
pub(crate) fn is_valid_user_name(user: &str) -> bool {
    if user.is_empty() || user.len() > 32 {
        return false;
    }
    if user.starts_with('-') {
        return false;
    }
    user.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
}

/// 状态分类（纯函数，好测）：只看「两个文件在不在 + 版本对不对」。
///
/// `stale` 专指**只剩一半**的情况 —— 最常见的来源是用户手动删了 `/usr/lib/anyboot`
/// 或手工删了 sudoers，此时界面必须能一眼看出「有残留」，而不是显示「已装好」。
pub(crate) fn classify(helper: bool, sudoers: bool, installed: Option<&str>) -> &'static str {
    match (helper, sudoers) {
        (true, true) => match installed {
            Some(version) if version == CURRENT_VERSION => "ready",
            _ => "outdated",
        },
        (false, false) => "absent",
        _ => "stale",
    }
}

/// 把 stat 授权文件的结果折算成「装没装」的三值可见性：
/// `Some(true)` 读到了、`Some(false)` 确定不存在、`None` **不可知**。
///
/// 为什么三值：`/etc/sudoers.d` 在不少发行版是 `0750 root:root`（真机
/// Arch 实测）—— 普通用户连目录都进不去，stat 里面的授权文件只会 EACCES。
/// **读不到 ≠ 没装**：把 EACCES 当「没装」，装好的组件会被判成「残留」，
/// 免密永远进不了 ready，每次都白弹一次 pkexec（2026-10-30 真机真 bug）。
fn sudoers_stat_visibility(result: std::io::Result<std::fs::Metadata>) -> Option<bool> {
    match result {
        Ok(_) => Some(true),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => None,
        Err(_) => Some(false),
    }
}

/// 把 `classify` 的结果翻成界面文案。
pub(crate) fn describe(state: &str, user: &str, installed: &str) -> String {
    match state {
        "ready" => format!("已安装：授权给 {user}，助手 v{installed}。特权操作不再弹密码框。"),
        "outdated" => format!(
            "需要更新：已装助手 v{}，当前程序 v{CURRENT_VERSION}。建议「重新安装」（会换掉助手与授权）。",
            if installed.is_empty() { "?" } else { installed }
        ),
        "stale" => "发现残留：授权文件与助手只剩一半（多半是手动删过其中一样）。点「移除」清干净后重装。"
            .to_string(),
        "absent" => "未安装：每次写 ESP / 固件变量都要输一次密码。装一次之后不再问。".to_string(),
        _ => "这个平台不需要免密组件（Windows 走 UAC，macOS 走系统助手）。".to_string(),
    }
}

// ============================ 下面都只在 Linux 编译 ============================
#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;

    /// 会话内的兜底密码（界面上输的那一次）。只在内存里，进程退出即失。
    static FALLBACK: Mutex<Option<String>> = Mutex::new(None);

    fn euid() -> u32 {
        // SAFETY: geteuid 没有前置条件，不会失败。
        unsafe { libc::geteuid() }
    }

    /// 在 PATH 里找可执行文件（不 shell out `which`：容器里常常没有它）。
    pub(crate) fn find_in_path(name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                // 还要可执行：root 的 PATH 里偶尔有 0644 的同名文件
                if let Ok(meta) = fs::metadata(&candidate) {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate);
                    }
                }
            }
        }
        for fallback in ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            let candidate = Path::new(fallback).join(name);
            if candidate.is_file() {
                if let Ok(meta) = fs::metadata(&candidate) {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    fn username_by_uid(uid: u32) -> Option<String> {
        let passwd = fs::read_to_string("/etc/passwd").ok()?;
        passwd.lines().find_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            // 第 3 个字段是 uid
            let _password = fields.next()?;
            let got = fields.next()?.parse::<u32>().ok()?;
            (got == uid).then(|| name.to_string())
        })
    }

    fn current_user() -> Option<String> {
        std::env::var("USER")
            .ok()
            .filter(|name| !name.is_empty())
            .or_else(|| username_by_uid(euid()))
    }

    /// 当前用户是不是在 `sudo` / `wheel` 组里（只有提示意义：不在的话密码兜底也走不通）。
    fn in_sudo_group() -> bool {
        let Some(user) = current_user() else {
            return false;
        };
        if euid() == 0 {
            return true;
        }
        let Ok(groups) = fs::read_to_string("/etc/group") else {
            return false;
        };
        groups.lines().any(|line| {
            let mut fields = line.split(':');
            let Some(name) = fields.next() else {
                return false;
            };
            if name != "sudo" && name != "wheel" {
                return false;
            }
            let _password = fields.next();
            let _gid = fields.next();
            fields
                .next()
                .map(|members| members.split(',').any(|member| member == user))
                .unwrap_or(false)
        })
    }

    /// 读安装记录；读不到/坏掉都算「没装」。
    pub(crate) fn read_meta() -> Option<PrivMeta> {
        let text = fs::read_to_string(META_PATH).ok()?;
        serde_json::from_str::<PrivMeta>(&text).ok()
    }

    pub(crate) fn status() -> PrivStatus {
        let helper_exists = Path::new(HELPER_PATH).is_file();
        let meta = read_meta();
        // EACCES（目录 0750 看不见）时以 meta 为准：安装流程是
        // 「写助手 → 写 sudoers（visudo 校验过）→ 写 meta」，meta 存在就说明
        // sudoers 那步成功落过盘。真被手动删掉 sudoers 的情况由调用方兜底
        // —— sudo -n 失败会带着真实原因落进提权链的下一段。
        let sudoers_exists = match sudoers_stat_visibility(fs::metadata(SUDOERS_PATH)) {
            Some(visible) => visible,
            None => meta.is_some(),
        };
        let installed = meta.as_ref().map(|m| m.version.clone()).unwrap_or_default();
        let user = meta.as_ref().map(|m| m.user.clone()).unwrap_or_default();
        let state = classify(helper_exists, sudoers_exists, meta.as_ref().map(|m| m.version.as_str()));
        PrivStatus {
            platform: true,
            state: state.to_string(),
            detail: describe(state, &user, &installed),
            helper_path: HELPER_PATH.to_string(),
            sudoers_path: SUDOERS_PATH.to_string(),
            helper_exists,
            sudoers_exists,
            version: installed,
            current_version: CURRENT_VERSION.to_string(),
            user,
            has_pkexec: find_in_path("pkexec").is_some(),
            has_sudo: find_in_path("sudo").is_some(),
            is_sudoer: in_sudo_group(),
        }
    }

    /// 装免密组件时该授权给谁：`sudo` 会给 `SUDO_USER`，`pkexec` 会给 `PKEXEC_UID`。
    ///
    /// 两个都没有 = 有人直接用 root 跑（su / root shell）。这种情况**拒绝**：授权给 root
    /// 没有意义，还会让 sudoers 里多一行看起来生效、实际谁都用不上的规则。
    fn install_user() -> Result<String, String> {
        if let Ok(name) = std::env::var("SUDO_USER") {
            if !name.is_empty() && name != "root" && is_valid_user_name(&name) {
                return Ok(name);
            }
        }
        if let Ok(raw) = std::env::var("PKEXEC_UID") {
            if let Ok(uid) = raw.trim().parse::<u32>() {
                if let Some(name) = username_by_uid(uid) {
                    if is_valid_user_name(&name) {
                        return Ok(name);
                    }
                    return Err(format!("当前用户名的字符不安全，不能写进 sudoers：{name}"));
                }
            }
        }
        Err("拿不到「要授权的用户」：请从 AnyBoot 界面点安装（不要用 root 直接跑这条命令）".to_string())
    }

    fn ensure_dir(path: &str) -> Result<(), String> {
        fs::create_dir_all(path).map_err(|error| format!("创建 {path} 失败：{error}"))?;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
        Ok(())
    }

    /// `visudo -cf <file>`：语法不过就不写。找不到 visudo 也拒绝 —— 宁可装不上，
    /// 也不能往 `/etc/sudoers.d/` 里塞一份没校验过的文件（写坏了整机 sudo 不可用）。
    ///
    /// 这段是 root 在跑，所以不吃 `PATH`：先按各发行版的标准位置找（Debian/RHEL 在
    /// `/usr/sbin`，Arch 也是），找不到才退回 `PATH` 查找，并且**清空环境变量**再执行
    /// —— 否则调用方塞一个 `LD_PRELOAD` / 假 `PATH` 就能让「校验」变成「跑我的程序」。
    fn verify_sudoers(path: &Path) -> Result<(), String> {
        let visudo = ["/usr/sbin/visudo", "/sbin/visudo", "/usr/bin/visudo", "/bin/visudo"]
            .iter()
            .map(PathBuf::from)
            .find(|candidate| candidate.is_file())
            .or_else(|| find_in_path("visudo").map(PathBuf::from))
            .ok_or("没找到 visudo（sudo 的校验工具），出于安全不写入未校验的授权文件")?;
        let output = Command::new(&visudo)
            .arg("-cf")
            .arg(path)
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .output()
            .map_err(|error| format!("跑 {} 失败：{error}", visudo.display()))?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("sudoers 语法校验没通过：{}", stderr.trim()))
        }
    }

    /// **必须 root**：装助手 + 写 sudoers + 落元数据。由 `--linux-priv-install` 调用。
    pub(crate) fn install_as_root() -> Result<(), String> {
        if euid() != 0 {
            return Err("安装免密组件需要 root（应当由 pkexec / sudo 拉起来）".to_string());
        }
        let user = install_user()?;
        ensure_dir(STATE_DIR)?;
        ensure_dir("/usr/lib/anyboot")?;

        // ① 助手 = 本程序的副本。先写 .new 再 rename：正在跑的旧助手是 inode 级的，
        //    rename 换掉目录项不会打断它，也不会留下半个文件。
        let exe = std::env::current_exe().map_err(|error| format!("拿不到自身路径：{error}"))?;
        let staged_helper = format!("{HELPER_PATH}.new");
        fs::copy(&exe, &staged_helper)
            .map_err(|error| format!("复制助手到 {staged_helper} 失败：{error}"))?;
        let _ = fs::set_permissions(&staged_helper, fs::Permissions::from_mode(0o755));
        fs::rename(&staged_helper, HELPER_PATH)
            .map_err(|error| format!("替换助手失败：{error}"))?;

        // ② sudoers：先在 /var/lib 里写临时文件（**绝对不能放 /etc/sudoers.d/**，
        //    那个目录里所有不以 ~ 结尾的文件都会被 include），校验通过再搬过去。
        if !Path::new("/etc/sudoers.d").is_dir() {
            return Err("这个系统没有 /etc/sudoers.d（sudo 配置可能在别处），暂不支持免密安装".to_string());
        }
        let staged_sudoers = Path::new(STATE_DIR).join("anyboot.sudoers.tmp");
        fs::write(&staged_sudoers, sudoers_text(&user))
            .map_err(|error| format!("写临时授权文件失败：{error}"))?;
        let _ = fs::set_permissions(&staged_sudoers, fs::Permissions::from_mode(0o600));
        verify_sudoers(&staged_sudoers)?;

        // 目标文件先落到同目录的 `.incoming`（含 `.` 的会被 sudo 忽略），设好 0440
        // 再 rename —— 避免出现「读得到但还没 chmod」的窗口。
        let incoming = "/etc/sudoers.d/anyboot.incoming";
        fs::copy(&staged_sudoers, incoming)
            .map_err(|error| format!("写 {incoming} 失败：{error}"))?;
        let _ = fs::set_permissions(incoming, fs::Permissions::from_mode(0o440));
        fs::rename(incoming, SUDOERS_PATH)
            .map_err(|error| format!("落位 {SUDOERS_PATH} 失败：{error}"))?;
        let _ = fs::remove_file(&staged_sudoers);

        // ③ 元数据（0644：普通用户要读得到，界面才能显示「装给谁、哪个版本」）
        let meta = PrivMeta {
            user: user.clone(),
            version: CURRENT_VERSION.to_string(),
            helper: HELPER_PATH.to_string(),
            installed_at: now_text(),
        };
        let text = serde_json::to_string_pretty(&meta).unwrap_or_else(|_| "{}".to_string());
        fs::write(META_PATH, text).map_err(|error| format!("写 {META_PATH} 失败：{error}"))?;
        let _ = fs::set_permissions(META_PATH, fs::Permissions::from_mode(0o644));
        Ok(())
    }

    /// **必须 root**：删授权、删助手、删元数据。删完这个系统里就没有 AnyBoot 的特权痕迹。
    ///
    /// 注意顺序无关紧要 —— 这里没有常驻进程，没有 socket 会「把服务重新拉起来」，
    /// 也不存在「先停哪个再停哪个」（那是常驻服务方案才有的麻烦）。
    pub(crate) fn uninstall_as_root() -> Result<(), String> {
        if euid() != 0 {
            return Err("删除免密组件需要 root".to_string());
        }
        let mut errors: Vec<String> = Vec::new();
        for path in [SUDOERS_PATH, HELPER_PATH, META_PATH] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => errors.push(format!("删 {path} 失败：{error}")),
            }
        }
        // 空目录顺手收掉（非空就留着，别删用户的东西）
        let _ = fs::remove_dir("/usr/lib/anyboot");
        let _ = fs::remove_dir(STATE_DIR);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }

    fn now_text() -> String {
        // 不引 chrono：只要一个「看得懂的时间」，用系统时间戳够了
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        format!("unix:{seconds}")
    }

    pub(crate) fn set_password(password: Option<String>) {
        if let Ok(mut slot) = FALLBACK.lock() {
            *slot = password.filter(|text| !text.is_empty());
        }
    }

    /// 看一眼会话密码（**不取走**）：同一次运行里可能连着做几次提权，
    /// 取走的话第二次就要重新输。
    fn peek_password() -> Option<String> {
        FALLBACK.lock().ok().and_then(|slot| slot.clone())
    }

    fn password_present() -> bool {
        peek_password().is_some()
    }

    /// 结果文件「有没有被写过」= 子命令到底跑没跑起来。
    ///
    /// 这是区分「授权没拿到」和「授权拿到了但操作失败」的唯一可靠信号：每条 root
    /// 子命令都会把结果写进 `--out`。没这个判断，「写失败」会误触发一次密码框。
    fn out_ready(path: &Path) -> bool {
        fs::metadata(path).map(|meta| meta.len() > 0).unwrap_or(false)
    }

    fn sudo_script(args: &[String], password: Option<&str>) -> Command {
        let mut cmd = Command::new("sudo");
        if password.is_some() {
            // -S 从 stdin 读密码；-p '' 去掉提示语（提示会混进我们的输出）
            cmd.arg("-S").arg("-p").arg("");
        } else {
            // -n：绝不交互。没授权就立刻失败，不会挂在那里等输入
            cmd.arg("-n");
        }
        cmd.arg(HELPER_PATH).args(args);
        cmd.stdin(if password.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());
        cmd
    }

    fn run_sudo(args: &[String], password: Option<&str>) -> Result<std::process::ExitStatus, String> {
        let mut child = sudo_script(args, password)
            .spawn()
            .map_err(|error| format!("无法执行 sudo（装了吗）：{error}"))?;
        if let Some(secret) = password {
            if let Some(mut stdin) = child.stdin.take() {
                // 密码只走管道：绝不拼进命令行（ps 里会露）
                let _ = stdin.write_all(secret.as_bytes());
                let _ = stdin.write_all(b"\n");
                let _ = stdin.flush();
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("等 sudo 失败：{error}"))?;
        Ok(output.status)
    }

    /// **只走免密助手**：装了就直接跑，不开任何对话框（远程请求也照办）。
    ///
    /// 成功与否一律以「结果文件写没写」为准 —— 操作本身失败也算跑通，错误由调用方
    /// 从结果 JSON 里读。
    pub(crate) fn run_via_helper(args: &[String], out_path: &Path) -> Result<(), String> {
        if !helper_ready() {
            return Err("没装免密助手".to_string());
        }
        let _ = fs::remove_file(out_path);
        let status = run_sudo(args, None).map_err(|error| {
            format!("{error}（sudo -n 没能拉起助手；sudoers 里那行授权可能已被删掉）")
        })?;
        if out_ready(out_path) {
            return Ok(());
        }
        Err(format!(
            "免密助手没写出结果（sudo 退出码 {}）—— 授权多半失效了，重装一次免密组件",
            status.code().unwrap_or(-1)
        ))
    }

    /// 完整链：免密助手 → `pkexec`（系统授权框）→ 内存里的密码。
    ///
    /// `allow_gui=false` 时只走第一条 —— 远程 / 无人值守场景不能让主机桌面上弹框
    /// （弹了也没人点，只会白等超时）。
    pub(crate) fn spawn_privileged(
        args: &[String],
        out_path: &Path,
        allow_gui: bool,
    ) -> Result<(), String> {
        let mut tried: Vec<String> = Vec::new();

        // ① 免密（装了就是这条路，全程无感）
        if helper_ready() {
            match run_via_helper(args, out_path) {
                Ok(()) => return Ok(()),
                Err(error) => tried.push(error),
            }
        } else {
            tried.push("没装免密助手".to_string());
        }

        if !allow_gui {
            return Err(format!(
                "需要本机授权，但当前不允许弹授权框（{}）：在目标机主窗口装一次免密组件即可静默完成",
                tried.join("；")
            ));
        }

        // ② pkexec：系统自己的授权框（有 polkit 的桌面才有）
        if find_in_path("pkexec").is_some() {
            let exe = std::env::current_exe().map_err(|error| format!("拿不到自身路径：{error}"))?;
            let _ = fs::remove_file(out_path);
            let status = Command::new("pkexec")
                .arg(exe)
                .args(args)
                .stderr(Stdio::null())
                .status()
                .map_err(|error| format!("无法调起 pkexec：{error}"))?;
            if out_ready(out_path) {
                return Ok(());
            }
            tried.push(format!("pkexec 退出码 {}", status.code().unwrap_or(-1)));
        } else {
            tried.push("没有 pkexec".to_string());
        }

        // ③ sudo：先用时间戳（用户此前在别处 sudo 过、或刚输过一次），
        //    不行再吃界面里输过的那一次密码。
        //
        //    顺序别反：`sudo -n` 命中时连管道都不用开；而密码只在「这次会话还没
        //    攒到时间戳」时才需要。sudo -S 成功后立刻把密码从内存里丢掉 ——
        //    之后靠时间戳就够了，没必要继续留着。
        if find_in_path("sudo").is_some() {
            let exe = std::env::current_exe().map_err(|error| format!("拿不到自身路径：{error}"))?;
            let _ = fs::remove_file(out_path);
            if let Ok(status) = Command::new("sudo")
                .arg("-n")
                .arg(&exe)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                if out_ready(out_path) {
                    return Ok(());
                }
                if !password_present() {
                    tried.push(format!(
                        "sudo -n 退出码 {}（没攒到时间戳，也没输过密码）",
                        status.code().unwrap_or(-1)
                    ));
                }
            }
            if let Some(secret) = peek_password() {
                let mut cmd = Command::new("sudo");
                cmd.args(["-S", "-p", ""]).arg(&exe).args(args);
                cmd.stdin(Stdio::piped());
                cmd.stdout(Stdio::null());
                cmd.stderr(Stdio::piped());
                let _ = fs::remove_file(out_path);
                if let Ok(mut child) = cmd.spawn() {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(secret.as_bytes());
                        let _ = stdin.write_all(b"\n");
                        let _ = stdin.flush();
                    }
                    let _ = child.wait();
                    if out_ready(out_path) {
                        // 时间戳已攒到，密码没必要再留在内存里
                        set_password(None);
                        return Ok(());
                    }
                }
                tried.push("密码授权没成功（密码错？不在 sudo 组？）".to_string());
            }
        } else {
            tried.push("没有 sudo".to_string());
        }

        Err(format!("提权失败：{}", tried.join("；")))
    }

    /// AppImage 场景的执行副本：pkexec / sudo **执行不了 /tmp 下的文件**。
    ///
    /// AppImage 运行时把 squashfs 挂在 `/tmp/.mount_*`，而新版 Debian/Ubuntu 默认给
    /// /tmp 加了 `noexec` —— 于是 pkexec 报 "Error accessing ...: Permission denied"、
    /// sudo 报 "找不到命令"，免密组件在 AppImage 里永远装不上。处理：把自身复制到
    /// `$XDG_CACHE_HOME/anyboot/priv-stage/`（用户主目录所在盘，exec 必允许），
    /// 提权命令跑这份副本。root 子进程的 `current_exe()` 也会指到这份真文件，
    /// `install_as_root` 里复制助手副本的源同样跟着安全。
    fn stage_executable_copy() -> Result<PathBuf, String> {
        let exe = std::env::current_exe().map_err(|error| format!("拿不到自身路径：{error}"))?;
        if !needs_exec_staging(&exe) {
            return Ok(exe);
        }
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|home| home.join(".cache"))
            })
            .ok_or_else(|| "拿不到缓存目录（HOME 与 XDG_CACHE_HOME 都为空），无法暂存执行副本".to_string())?;
        let dir = cache.join("anyboot").join("priv-stage");
        fs::create_dir_all(&dir).map_err(|error| format!("创建 {} 失败：{error}", dir.display()))?;
        let target = dir.join("anyboot");
        // 写 .new 再 rename：目标可能正被上一个 root 子进程执行（inode 级安全，与
        // install_as_root 换助手副本同一招）
        let staged = dir.join("anyboot.new");
        fs::copy(&exe, &staged)
            .map_err(|error| format!("暂存执行副本到 {} 失败：{error}", staged.display()))?;
        let _ = fs::set_permissions(&staged, fs::Permissions::from_mode(0o755));
        fs::rename(&staged, &target)
            .map_err(|error| format!("落位执行副本失败：{error}"))?;
        Ok(target)
    }

    /// 判定要不要暂存：路径在 /tmp（或 AppImage 的 `.mount_` 挂载点）下就需要。
    /// 纯函数好测；只看路径形状，不做「/tmp 是否 noexec」的运行时探测 —— 与其猜
    /// 挂载选项，不如凡是挂载点里的都暂存一份，deb/rpm 安装路径不受影响。
    pub(super) fn needs_exec_staging(exe: &Path) -> bool {
        if exe.starts_with("/tmp") || exe.starts_with("/var/tmp") {
            return true;
        }
        exe.components().any(|c| {
            let part = c.as_os_str().to_string_lossy();
            part.starts_with(".mount_") || part == "mnt_appimage"
        })
    }

    /// 需要 root 才能做的「删 / 装」走的桥：`pkexec <self> --linux-priv-* --out <file>`。
    ///
    /// 和运行时那条链不同：这里**允许弹框**（用户就在屏幕前点按钮），并且优先 pkexec
    /// —— 装组件的动作本来就该让用户看见系统授权框。
    fn run_root_subcommand(flag: &str, password: Option<&str>) -> Result<(), String> {
        let exe = stage_executable_copy()?;
        // 落 /tmp 而不是 `env::temp_dir()`：root 侧要校验这个前缀，两边的 `$TMPDIR`
        // 不一定一样（用户会话常见 `/run/user/1000`）
        let out = PathBuf::from(SCRATCH_DIR).join(format!(
            "anyboot-priv-{}-{}.json",
            std::process::id(),
            flag.trim_start_matches("--")
        ));
        let _ = fs::remove_file(&out);
        let out_text = out.to_string_lossy().to_string();
        let mut tried: Vec<String> = Vec::new();

        if let Some(pkexec) = find_in_path("pkexec") {
            let status = Command::new(pkexec)
                .arg(&exe)
                .arg(flag)
                .arg("--out")
                .arg(&out_text)
                .stderr(Stdio::piped())
                .output()
                .map_err(|error| format!("无法调起 pkexec：{error}"))?;
            if out_ready(&out) {
                return read_root_result(&out);
            }
            let stderr = String::from_utf8_lossy(&status.stderr).trim().to_string();
            tried.push(if stderr.is_empty() {
                format!("pkexec 退出码 {}", status.status.code().unwrap_or(-1))
            } else {
                format!("pkexec：{stderr}")
            });
        } else {
            tried.push("没有 pkexec".to_string());
        }

        if find_in_path("sudo").is_some() {
            let mut cmd = Command::new("sudo");
            match password {
                Some(_) => {
                    cmd.args(["-S", "-p", ""]);
                }
                None => {
                    cmd.arg("-n");
                }
            }
            cmd.arg(&exe).arg(flag).arg("--out").arg(&out_text);
            cmd.stdin(if password.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            });
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::piped());
            if let Ok(mut child) = cmd.spawn() {
                if let Some(secret) = password {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = stdin.write_all(secret.as_bytes());
                        let _ = stdin.write_all(b"\n");
                        let _ = stdin.flush();
                    }
                }
                let output = child
                    .wait_with_output()
                    .map_err(|error| format!("等 sudo 失败：{error}"))?;
                if out_ready(&out) {
                    return read_root_result(&out);
                }
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                tried.push(if stderr.contains("password is required") {
                    "需要密码（在下面输入本机密码再试）".to_string()
                } else if stderr.is_empty() {
                    format!("sudo 退出码 {}", output.status.code().unwrap_or(-1))
                } else {
                    format!("sudo：{stderr}")
                });
            }
        }

        Err(tried.join("；"))
    }

    fn read_root_result(path: &Path) -> Result<(), String> {
        let text = fs::read_to_string(path).unwrap_or_default();
        let _ = fs::remove_file(path);
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|_| format!("root 子命令回传的不是 JSON：{text}"))?;
        if value.get("ok").and_then(|ok| ok.as_bool()).unwrap_or(false) {
            Ok(())
        } else {
            Err(value
                .get("error")
                .and_then(|error| error.as_str())
                .unwrap_or("root 侧没说原因")
                .to_string())
        }
    }

    /// 界面点「安装」：先弹一次授权（pkexec 或密码），装完再回一份新状态。
    pub(crate) fn install(password: Option<String>) -> Result<PrivStatus, String> {
        if euid() == 0 {
            install_as_root()?;
        } else {
            run_root_subcommand("--linux-priv-install", password.as_deref())?;
        }
        Ok(status())
    }

    /// 界面点「移除」：删 sudoers + 助手 + 元数据。删完不留任何常驻对象。
    pub(crate) fn uninstall(password: Option<String>) -> Result<PrivStatus, String> {
        if euid() == 0 {
            uninstall_as_root()?;
        } else {
            run_root_subcommand("--linux-priv-uninstall", password.as_deref())?;
        }
        Ok(status())
    }

    pub(crate) fn helper_ready() -> bool {
        Path::new(HELPER_PATH).is_file()
    }
}

// ============================ 非 Linux：接口照旧，全部不支持 ============================
#[cfg(not(target_os = "linux"))]
mod imp {
    use super::*;

    pub(crate) fn status() -> PrivStatus {
        PrivStatus {
            platform: false,
            state: "unsupported".to_string(),
            detail: describe("unsupported", "", ""),
            helper_path: HELPER_PATH.to_string(),
            sudoers_path: SUDOERS_PATH.to_string(),
            current_version: CURRENT_VERSION.to_string(),
            has_pkexec: false,
            has_sudo: false,
            is_sudoer: false,
            ..PrivStatus::default()
        }
    }

    pub(crate) fn install(_password: Option<String>) -> Result<PrivStatus, String> {
        Err("免密组件只在 Linux 上需要（Windows 走 UAC，macOS 走系统助手）".to_string())
    }

    pub(crate) fn uninstall(_password: Option<String>) -> Result<PrivStatus, String> {
        Err("免密组件只在 Linux 上存在".to_string())
    }

    pub(crate) fn install_as_root() -> Result<(), String> {
        Err("免密组件只在 Linux 上需要".to_string())
    }

    pub(crate) fn uninstall_as_root() -> Result<(), String> {
        Err("免密组件只在 Linux 上存在".to_string())
    }

    pub(crate) fn helper_ready() -> bool {
        false
    }

    pub(crate) fn set_password(_password: Option<String>) {}

    pub(crate) fn spawn_privileged(
        _args: &[String],
        _out_path: &Path,
        _allow_gui: bool,
    ) -> Result<(), String> {
        Err("这个平台没有 sudo 免密通道".to_string())
    }

    pub(crate) fn run_via_helper(_args: &[String], _out_path: &Path) -> Result<(), String> {
        Err("这个平台没有 sudo 免密通道".to_string())
    }

    pub(crate) fn read_meta() -> Option<PrivMeta> {
        None
    }
}

pub(crate) use imp::*;

/// `--linux-priv-install` 的子命令体：写结果 JSON，返回进程退出码。
pub fn install_main(out_path: &str) -> i32 {
    write_result(out_path, imp::install_as_root())
}

/// `--linux-priv-uninstall` 的子命令体。
pub fn uninstall_main(out_path: &str) -> i32 {
    write_result(out_path, imp::uninstall_as_root())
}

/// `--linux-mount-esp` 的子命令体：root 侧**只读**挂载一块 EFI 系统分区，结果写 JSON。
///
/// 启动自动挂载与卡片按钮共用这条链路。参数来自用户态主进程，而本进程是 root ——
/// 所以入口先过 [`validate_mount_request`] 白名单，再动手。
pub fn mount_esp_main(
    device: &str,
    mountpoint: &str,
    fstype: &str,
    out_path: &str,
) -> i32 {
    let outcome = match validate_mount_request(device, mountpoint, fstype) {
        Ok(()) => mount_esp_as_root(device, mountpoint, fstype),
        Err(error) => Err(error),
    };
    write_result(out_path, outcome)
}

/// 挂载请求白名单（纯函数，好测）。
///
/// 三样都卡死：
/// - **设备**必须 `/dev/` 开头、无 `..`、无空白（块设备名不会有空格）；
/// - **挂载点**必须绝对、无 `..`，且路径里有一段叫 `anyboot-esp` —— 只允许挂进
///   我们自己的挂载点目录，不给「帮 root 把东西挂到别处」留口子；
/// - **文件系统**为空（让 mount 自己探测）或 FAT 家族 —— 这条链路只为 ESP 存在，
///   别的文件系统本来也挂不动（APFS 没驱动，探测失败正是期望行为）。
fn validate_mount_request(device: &str, mountpoint: &str, fstype: &str) -> Result<(), String> {
    if !device.starts_with("/dev/")
        || device.contains("..")
        || device.split_whitespace().count() != 1
        || device.len() > 64
    {
        return Err(format!("拒绝挂载非常规设备路径: {device:?}"));
    }
    let path = Path::new(mountpoint);
    if !path.is_absolute()
        || mountpoint.contains("..")
        || !path
            .components()
            .any(|component| component.as_os_str() == "anyboot-esp")
    {
        return Err(format!("挂载点必须落在 anyboot-esp 目录下: {mountpoint:?}"));
    }
    if !fstype.is_empty() && !matches!(fstype, "vfat" | "fat" | "msdos" | "exfat") {
        return Err(format!("这条链路只挂 FAT 家族（ESP），不接受 {fstype:?}"));
    }
    Ok(())
}

/// root 侧的实际挂载。`uid`/`gid` 取 sudo 传进来的 `SUDO_UID`/`SUDO_GID` ——
/// FAT 上的文件归属由挂载选项决定，不给的话全归 root，普通用户连自己挂的
/// 分区都读不了。
fn mount_esp_as_root(device: &str, mountpoint: &str, fstype: &str) -> Result<(), String> {
    let mut options = String::from("ro");
    let sudo_uid = std::env::var("SUDO_UID").ok();
    let sudo_gid = std::env::var("SUDO_GID").ok();
    if let (Some(uid), Some(gid)) = (&sudo_uid, &sudo_gid) {
        if uid.bytes().all(|byte| byte.is_ascii_digit())
            && gid.bytes().all(|byte| byte.is_ascii_digit())
        {
            options = format!("{options},uid={uid},gid={gid}");
        }
    }
    let mut command = std::process::Command::new("mount");
    command.arg("-o").arg(&options);
    if !fstype.is_empty() {
        command.arg("-t").arg(fstype);
    }
    command.arg(device).arg(mountpoint);
    let output = command
        .output()
        .map_err(|error| format!("无法执行 mount: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "mount 失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn write_result(out_path: &str, outcome: Result<(), String>) -> i32 {
    // 先卡路径再写：这条是 root 侧执行体，`--out` 由调用方给
    if let Err(error) = guard_result_path(out_path) {
        eprintln!("{error}");
        return 2;
    }
    let payload = match outcome {
        Ok(()) => serde_json::json!({"ok": true}),
        Err(error) => serde_json::json!({"ok": false, "error": error}),
    };
    let ok = payload
        .get("ok")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    // 落盘统一走 channels 的那份「tmp + rename」：直接 write 会让父进程有机会
    // 读到半截 JSON，而这种失败只在机器卡顿时出现，最难复现。
    if crate::channels::write_result_file(out_path, &payload.to_string()) != 0 {
        return 2;
    }
    if ok {
        0
    } else {
        1
    }
}

// ============================ 命令面（前端「关于」页那张卡） ============================

/// 查当前状态。只读几个小文件，快，不需要 spawn_blocking。
#[tauri::command]
pub(crate) fn linux_priv_status() -> PrivStatus {
    imp::status()
}

/// 安装：弹一次授权（pkexec 系统框，或界面里输的密码），装完回新状态。
///
/// **必须 async + spawn_blocking**：pkexec 会同步等用户在授权框里点确认，
/// 可能几分钟 —— 直接跑在同步命令上会卡住 UI 线程。
#[tauri::command]
pub(crate) async fn linux_priv_install(password: Option<String>) -> Result<PrivStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = imp::install(password.clone());
        if result.is_ok() {
            // 会话内记住这次密码：装的过程中如果还要再提权一次，不用再问
            imp::set_password(password);
        }
        result
    })
    .await
    .map_err(|error| format!("安装免密组件执行失败: {error}"))?
}

/// 移除：删 `/etc/sudoers.d/anyboot` + 助手 + 元数据。删完这台机器上就只剩
/// `/usr/lib/anyboot/`（空目录也会一并删）。
#[tauri::command]
pub(crate) async fn linux_priv_uninstall(password: Option<String>) -> Result<PrivStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = imp::uninstall(password);
        // 移除之后没有任何东西还依赖那个密码了，立刻丢掉
        imp::set_password(None);
        result
    })
    .await
    .map_err(|error| format!("移除免密组件执行失败: {error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AppImage / noexec 场景：路径在 /tmp 或 .mount_* 下必须暂存执行副本，
    /// 否则 pkexec / sudo 执行不了（Permission denied / 找不到命令）。
    #[test]
    fn exec_staging_decision() {
        // AppImage 挂载点（用户实况）
        assert!(imp::needs_exec_staging(Path::new(
            "/tmp/.mount_AnyBooNlAmgG/usr/bin/anyboot"
        )));
        // /tmp、/var/tmp 直放（新版发行版默认 noexec）
        assert!(imp::needs_exec_staging(Path::new("/tmp/anyboot")));
        assert!(imp::needs_exec_staging(Path::new("/var/tmp/anyboot")));
        // deb/rpm 正常安装路径、开发 target —— 不暂存
        assert!(!imp::needs_exec_staging(Path::new(
            "/usr/bin/anyboot"
        )));
        assert!(!imp::needs_exec_staging(Path::new(
            "/home/jianghh/anyboot/src-tauri/target/release/anyboot"
        )));
    }

    /// 挂载请求白名单：助手以 root 跑 mount，参数必须一道道卡死。
    #[test]
    fn mount_request_whitelist() {
        // 正常路径：ESP 设备 + 我们自己的挂载点目录
        assert!(validate_mount_request(
            "/dev/nvme0n1p5",
            "/run/user/1000/anyboot-esp/nvme0n1p5",
            "vfat"
        )
        .is_ok());
        // fstype 留空 = 让 mount 自己探测，也放行
        assert!(validate_mount_request("/dev/sda2", "/tmp/anyboot-esp/sda2", "").is_ok());

        // 设备：不认非常规路径
        assert!(validate_mount_request(
            "nvme0n1p5",
            "/run/user/1000/anyboot-esp/nvme0n1p5",
            "vfat"
        )
        .is_err());
        assert!(validate_mount_request(
            "/dev/../etc/passwd",
            "/run/user/1000/anyboot-esp/x",
            "vfat"
        )
        .is_err());
        assert!(validate_mount_request(
            "/dev/sda1 /dev/sdb1",
            "/run/user/1000/anyboot-esp/x",
            "vfat"
        )
        .is_err());

        // 挂载点：必须绝对、无 ..、必须落在 anyboot-esp 下
        assert!(validate_mount_request(
            "/dev/sda2",
            "anyboot-esp/sda2",
            "vfat"
        )
        .is_err());
        assert!(validate_mount_request("/dev/sda2", "/home/user/x", "vfat").is_err());
        assert!(validate_mount_request(
            "/dev/sda2",
            "/tmp/anyboot-esp/../../etc/cron.d",
            "vfat"
        )
        .is_err());

        // 文件系统：只认 FAT 家族 —— APFS/ext4 都不是这条路该挂的
        assert!(validate_mount_request(
            "/dev/nvme0n1p5",
            "/run/user/1000/anyboot-esp/nvme0n1p5",
            "apfs"
        )
        .is_err());
        assert!(validate_mount_request(
            "/dev/nvme0n1p16",
            "/run/user/1000/anyboot-esp/nvme0n1p16",
            "ext4"
        )
        .is_err());
    }

    /// 用户名进 sudoers 前必须过白名单：能塞进换行/逗号/`!` 的名字就是提权漏洞。
    #[test]
    fn user_name_whitelist_rejects_injection() {
        assert!(is_valid_user_name("jianghh"));
        assert!(is_valid_user_name("john.doe_2"));
        assert!(is_valid_user_name("a-b"));
        // 换行能让第二条规则变成 `ALL ALL=(ALL) NOPASSWD: ALL`
        assert!(!is_valid_user_name("me\nALL ALL=(ALL) NOPASSWD: ALL"));
        assert!(!is_valid_user_name("me ALL=(ALL)"));
        assert!(!is_valid_user_name("me,other"));
        assert!(!is_valid_user_name("me#comment"));
        assert!(!is_valid_user_name(""));
        assert!(!is_valid_user_name("-oops"));
        assert!(!is_valid_user_name(&"x".repeat(40)));
        assert!(!is_valid_user_name("用户名"));
    }

    /// 生成的规则必须是「一行一条、只授权助手路径、带 NOPASSWD」。
    #[test]
    fn sudoers_text_has_one_nopasswd_line() {
        let text = sudoers_text("jianghh");
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .filter(|line| !line.trim().is_empty())
            .collect();
        assert_eq!(lines.len(), 1, "只能有一条规则：{text}");
        assert_eq!(
            lines[0],
            format!("jianghh ALL=(root) NOPASSWD: {HELPER_PATH}")
        );
        // 通配符一律不许出现：sudo 的参数匹配是 fnmatch，一个 `*` 就能执行任意命令
        assert!(!text.contains('*'));
        assert!(!text.contains("NOPASSWD: ALL"));
    }

    /// 状态分类：只剩一半必须是 `stale`（界面要能提示「有残留」），不能显示「已装好」。
    #[test]
    fn classify_flags_half_installed_state() {
        assert_eq!(classify(true, true, Some(CURRENT_VERSION)), "ready");
        assert_eq!(classify(true, true, Some("0.0.0-old")), "outdated");
        // 元数据丢了但两件文件都在：也算 outdated（界面会让人重装，能自愈）
        assert_eq!(classify(true, true, None), "outdated");
        assert_eq!(classify(false, false, None), "absent");
        assert_eq!(classify(true, false, None), "stale");
        assert_eq!(classify(false, true, None), "stale");
    }

    /// `/etc/sudoers.d` 是 0750 时普通用户 stat 授权文件只会 EACCES —— 必须
    /// 报「不可知」让调用方回落到 meta，**绝不能当成「没装」**（否则装好的
    /// 组件永远进不了 ready，免密变每次弹 pkexec，2026-10-30 真机事故）。
    #[test]
    fn sudoers_stat_permission_denied_is_unknown_not_missing() {
        assert_eq!(
            sudoers_stat_visibility(Err(std::io::Error::from(std::io::ErrorKind::NotFound))),
            Some(false)
        );
        assert_eq!(
            sudoers_stat_visibility(Err(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied
            ))),
            None
        );
        // Ok(true) 分支需要真实 Metadata，单测构造不了；其语义由 classify 的
        // (true, true) 用例锁住
    }

    #[test]
    fn describe_never_leaks_empty_user() {
        // 没装的时候不能出现「授权给 ，」这种断句
        let text = describe("absent", "", "");
        assert!(text.starts_with("未安装"));
        assert!(text.contains("输一次密码"));
        assert!(describe("ready", "jianghh", "0.1.0").contains("jianghh"));
    }

    /// 结果文件路径闸门：这是 root 侧唯一的「按调用方给的路径写」入口。
    #[test]
    fn result_path_guard_only_takes_flat_tmp_files() {
        assert!(guard_result_path("/tmp/anyboot-priv-123-linux-priv-install.json").is_ok());
        // 相对路径：root 进程的 cwd 我们控制不了
        assert!(guard_result_path("anyboot-priv-1.json").is_err());
        // `..` 能绕过 starts_with（组件比较时 `..` 只是普通组件）
        assert!(guard_result_path("/tmp/../etc/cron.d/anyboot").is_err());
        // 子目录可以被别人整目录换成指向 /etc 的符号链接
        assert!(guard_result_path("/tmp/anyboot/x.json").is_err());
        // /tmpfoo 与 /tmp 不是一回事，/var/tmp 也不在允许范围
        assert!(guard_result_path("/tmpfoo/x.json").is_err());
        assert!(guard_result_path("/var/tmp/x.json").is_err());
        assert!(guard_result_path("/etc/sudoers.d/anyboot").is_err());
        assert!(guard_result_path("/tmp").is_err());
    }

    /// 同名符号链接抢注：结果文件名含 pid，可预测，所以必须拒绝。
    #[cfg(unix)]
    #[test]
    fn result_path_guard_rejects_predropped_symlink() {
        let link = PathBuf::from(SCRATCH_DIR).join(format!(
            "anyboot-priv-guard-test-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink("/etc/passwd", &link).expect("建符号链接");
        let guarded = guard_result_path(&link.to_string_lossy());
        let _ = fs::remove_file(&link);
        assert!(guarded.is_err(), "指向 /etc/passwd 的符号链接必须被拒");
    }
}
