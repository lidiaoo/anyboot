//! 三平台 EFI 变量读写（BootNext / BootOrder / Boot#### / LoaderEntry*）。
//!
//! 只做 AnyBoot 用到的最小面：读、写（带提权回退）、BootOrder/LOAD_OPTION
//! 解析（纯函数好单测）。字节级约定：
//!
//! * efivarfs（Linux）的值文件 = 4 字节属性头（0x7 = NV|BS+RT，LE）+ 值本体；
//!   写也必须带同样的头，且**一次 write 写全**（efivarfs 不支持截断重写）。
//! * macOS `nvram`：读输出 `guid:name\t<值>`，二进制按 `%XX` 转义；写二进制
//!   用 `guid:name=%XX%XX…`。Windows 非提权进程连读都不行（需要
//!   SeSystemEnvironmentPrivilege）→ 统一 `NvramError::NeedElevation`。
//! * Windows：`Get/SetFirmwareEnvironmentVariableW`，缓冲区**不带**属性头
//!   （非 Ex 版隐含 NV|BS|RT），与 efivarfs 的带头格式差 4 字节 —— 平台层吸收。

use super::*;

/// UEFI 全局变量 GUID（BootOrder/Boot####/BootNext/BootCurrent）。
pub(crate) const GUID_GLOBAL: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
/// systemd-boot 的 GUID（LoaderEntryOneShot / LoaderEntryLastBooted）。
pub(crate) const GUID_SYSTEMD: &str = "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f";

/// efivarfs 属性头：NV + BS + RT。
/// （Linux 的 imp 与单测用；macOS/Windows 编译目标下会被判 dead，见下方各条的注）
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) const EFI_ATTR_NV_BS_RT: u32 = 0x7;

#[derive(Clone, Debug)]
pub(crate) enum NvramError {
    /// 当前进程提不了权/没被授权：UI 渲染 ⤴ 按钮与授权弹窗
    NeedElevation(String),
    /// 变量不存在（或本平台固件不暴露）
    NotFound(String),
    Failed(String),
}

impl NvramError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::NeedElevation(text) => format!("{text}（需要管理员权限）"),
            Self::NotFound(text) => text.clone(),
            Self::Failed(text) => text.clone(),
        }
    }
}

// ─────────────────────────── 纯函数层（全平台共用，单测覆盖） ───────────────────────────

/// efivarfs 文件名形式：`Name-GUID`，GUID 小写连字（Linux 内核约定）。
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn efivar_filename(name: &str, guid: &str) -> String {
    format!("{name}-{}", guid.to_ascii_lowercase())
}

/// 值本体 → efivarfs 文件内容（补 4 字节属性头）。
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn with_attr_header(value: &[u8]) -> Vec<u8> {
    let mut bytes = EFI_ATTR_NV_BS_RT.to_le_bytes().to_vec();
    bytes.extend_from_slice(value);
    bytes
}

/// efivarfs 文件内容 → 值本体（剥 4 字节属性头）。
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn strip_attr_header(bytes: &[u8]) -> Option<&[u8]> {
    bytes.get(4..)
}

/// BootOrder 值 → 启动项编号序列（UINT16 LE）。
pub(crate) fn parse_boot_order(value: &[u8]) -> Vec<u16> {
    value
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

/// `Boot####` 的编号 → 4 位十六进制变量名。
pub(crate) fn boot_var_name(number: u16) -> String {
    format!("Boot{number:04X}")
}

/// 从 EFI_LOAD_OPTION 里解析描述串（Attributes u32 + FilePathListLength u16
/// 之后的 UTF-16LE 描述，双 NUL 结尾）。供展示与单测；定位靠 UTF-16 搜索，不依赖它。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn load_option_description(value: &[u8]) -> Option<String> {
    if value.len() < 6 {
        return None;
    }
    let mut units = Vec::new();
    for pair in value[6..].chunks_exact(2) {
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16(&units).ok().filter(|text| !text.is_empty())
}

/// 判断一份 EFI_LOAD_OPTION 是不是指向 `marker`（如 `OC\OpenCore.efi`）：
/// 在整份值里按 UTF-16LE 编码搜子串。完整 DevicePath 解析太重（路径节点类型
/// 十几种），而「路径含该 .efi」就足以唯一认定 —— 已知误判风险低（单测覆盖样例）。
pub(crate) fn load_option_matches(value: &[u8], marker: &str) -> bool {
    let mut needle = Vec::with_capacity(marker.len() * 2);
    for unit in marker.encode_utf16() {
        needle.extend_from_slice(&unit.to_le_bytes());
    }
    // 大小写不敏感（ESP 的 FAT 目录大小写常被改写）：两边都按字节降 case
    fn lower(bytes: &[u8]) -> Vec<u8> {
        bytes.iter().map(|b| b.to_ascii_lowercase()).collect()
    }
    let (haystack, needle) = (lower(value), lower(&needle));
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_slice())
}

/// BootNext 值本体（UINT16 LE，不带属性头）。
pub(crate) fn boot_next_value(number: u16) -> Vec<u8> {
    number.to_le_bytes().to_vec()
}

/// macOS `nvram` 的一行输出 → 值本体：`%XX` 十六进制转义按字节还原，
/// 其余字符取 ASCII 字节。（nvram 只对不可打印字节转义，可打印原样 —— 解码无歧义：
/// 字面 `%` 会转义成 `%25`。）
pub(crate) fn decode_nvram_escaped(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '%' && index + 2 < chars.len() + 1 {
            let hex: String = chars[index + 1..(index + 3).min(chars.len())].iter().collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                bytes.push(byte);
                index += 3;
                continue;
            }
        }
        // 非 ASCII 字符按 UTF-8 落（不会出现在固件变量的转义输出里，兜底而已）
        let mut buffer = [0u8; 4];
        bytes.extend_from_slice(chars[index].encode_utf8(&mut buffer).as_bytes());
        index += 1;
    }
    bytes
}

/// 一批变量写入 → macOS 的「管理员授权」AppleScript。
///
/// 写启动项要动 `Boot####` + `BootOrder` 两条变量；逐条走授权框会连弹两次密码，
/// 所以拼成一条 `nvram A=B && nvram C=D` 交给**一次** `do shell script`。
/// 值都是 `%XX` 转义串（只有十六进制字符和 `%`），不会带出 shell 元字符。
pub(crate) fn elevated_nvram_script(entries: &[(&str, &str, &[u8])]) -> String {
    let commands: Vec<String> = entries
        .iter()
        .map(|(name, guid, value)| format!("/usr/sbin/nvram {}", nvram_write_arg(name, guid, value)))
        .collect();
    format!(
        "do shell script \"{}\" with administrator privileges",
        commands.join(" && ")
    )
}

/// 值本体 → macOS `nvram` 写入用的 `%XX` 转义串（含首字符的 `%`）。
pub(crate) fn encode_nvram_escaped(value: &[u8]) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value {
        out.push_str(&format!("%{byte:02X}"));
    }
    out
}

/// 写入一个变量时**唯一**的 argv：`guid:name=%XX%XX…`。
///
/// 必须是**同一个**参数。拆成 `nvram guid:name =%XX…` 两个参数时，`nvram` 会把第一个
/// 当「读取」（照原值打印一遍）、第二个当「设置一个空名字的变量」，**退出码 0 却什么
/// 都没写**——实测踩到：`OCBtOrder` 明明改了值，回读还是老值，而且报不出错。
pub(crate) fn nvram_write_arg(name: &str, guid: &str, value: &[u8]) -> String {
    format!("{guid}:{name}={}", encode_nvram_escaped(value))
}

// ─────────────────────────── 平台实现层 ───────────────────────────

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    fn efivar_path(name: &str, guid: &str) -> PathBuf {
        PathBuf::from("/sys/firmware/efi/efivars").join(efivar_filename(name, guid))
    }

    pub(crate) fn read_var(name: &str, guid: &str) -> Result<Vec<u8>, NvramError> {
        let path = efivar_path(name, guid);
        let raw = fs::read(&path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => NvramError::NotFound(format!(
                "NVRAM 变量 {name} 不存在（本机固件没有暴露它）"
            )),
            io::ErrorKind::PermissionDenied => NvramError::NeedElevation(format!(
                "读 {name} 需要权限（efivarfs 的变量文件是 root 的）"
            )),
            _ => NvramError::Failed(format!("读 {} 失败: {error}", path.display())),
        })?;
        strip_attr_header(&raw)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| NvramError::Failed(format!("{} 内容短于属性头", path.display())))
    }

    pub(crate) fn write_var(name: &str, guid: &str, value: &[u8]) -> Result<(), NvramError> {
        let path = efivar_path(name, guid);
        let content = with_attr_header(value);
        // 已经是 root（或 efivarfs 可写）就直写；efivarfs 必须一次 write 写全
        let direct = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .and_then(|mut file| file.write_all(&content));
        match direct {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {}
            Err(error) => {
                return Err(NvramError::Failed(format!(
                    "写 {} 失败: {error}",
                    path.display()
                )))
            }
        }
        // 非 root：暂存 + pkexec 单命令（与 grub_esp 的 stage_and_install 同模式）
        if let Some(error) = gui_auth_forbidden() {
            return Err(NvramError::NeedElevation(error));
        }
        let staged = unique_temp_path("anyboot-nvram-stage");
        if let Err(error) = stage_and_install(&content, &path) {
            let _ = fs::remove_file(&staged);
            return Err(NvramError::NeedElevation(error));
        }
        let _ = fs::remove_file(&staged);
        Ok(())
    }

    /// 本机有没有 efivarfs（没有 = 不是 UEFI 启动，或 efivarfs 没挂）。
    pub(crate) fn available() -> bool {
        Path::new("/sys/firmware/efi/efivars").is_dir()
    }

    /// efivarfs 可写（提权后）→ 能直接写 Boot*。
    pub(crate) fn can_write_boot_vars() -> bool {
        true
    }

    /// 批量写：Linux 上每条都能独立走「直写 / pkexec」，逐条来即可。
    pub(crate) fn write_vars(entries: &[(&str, &str, &[u8])]) -> Result<(), NvramError> {
        for (name, guid, value) in entries {
            write_var(name, guid, value)?;
        }
        Ok(())
    }

    /// Linux 的 efivarfs 没有独立特权位：文件权限即一切，提权后天然可读写。
    pub(crate) fn enable_system_environment_privilege() -> bool {
        true
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    /// `nvram guid:name` 的一行输出。成功输出形如
    /// `8be4df61-…:BootOrder<TAB>%01%00…`（二进制按 %XX 转义）。
    fn run_nvram(args: &[&str]) -> Result<Output, NvramError> {
        command("/usr/sbin/nvram")
            .args(args)
            .stderr(Stdio::null())
            .output()
            .map_err(|error| NvramError::Failed(format!("无法执行 nvram: {error}")))
    }

    pub(crate) fn read_var(name: &str, guid: &str) -> Result<Vec<u8>, NvramError> {
        let key = format!("{guid}:{name}");
        let output = run_nvram(&[key.as_str()])?;
        if !output.status.success() {
            return Err(NvramError::NotFound(format!(
                "NVRAM 变量 {name} 不存在（nvram 退出码非 0）"
            )));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        // 找到第一个 TAB，其后是值
        let value = text
            .lines()
            .find(|line| line.contains('\t'))
            .and_then(|line| line.split_once('\t'))
            .map(|(_, value)| value)
            .ok_or_else(|| NvramError::Failed(format!("nvram 输出格式无法解析: {text:?}")))?;
        Ok(decode_nvram_escaped(value))
    }

    /// 直写一条（不提权）：root 下、或该变量本身可写时成功。
    fn direct_write(name: &str, guid: &str, value: &[u8]) -> Result<(), NvramError> {
        // 写路径要拿 stderr 判「是权限问题还是固件拒绝」，所以不能走 run_nvram（它吞 stderr）
        let output = command("/usr/sbin/nvram")
            .arg(nvram_write_arg(name, guid, value))
            .output()
            .map_err(|error| NvramError::Failed(format!("无法执行 nvram: {error}")))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(NvramError::NeedElevation(if stderr.is_empty() {
            format!("写 {name} 需要管理员授权（nvram 退出码非 0）")
        } else {
            format!("写 {name} 需要管理员授权：{stderr}")
        }))
    }

    pub(crate) fn write_var(name: &str, guid: &str, value: &[u8]) -> Result<(), NvramError> {
        write_vars(&[(name, guid, value)])
    }

    /// 批量写。macOS 上非 root 写 NVRAM 必然失败，所以：
    /// ① 能直写就直写（root 下、或该变量本来就允许写 —— 不弹框）；
    /// ② 否则用**一次**管理员授权框把整批写完（`Boot####` + `BootOrder` 只弹一次密码）。
    pub(crate) fn write_vars(entries: &[(&str, &str, &[u8])]) -> Result<(), NvramError> {
        let mut first_error = None;
        for (name, guid, value) in entries {
            if let Err(error) = direct_write(name, guid, value) {
                first_error = Some(error);
                break;
            }
        }
        let Some(first_error) = first_error else {
            return Ok(());
        };
        // ② 免密助手（如果开着）：它就是为「别再弹密码」装的，所以排在密码框之前。
        //    远程请求也走这条路 —— 它不需要「本机有人点确认」，而 osascript 需要。
        //    注意分类：助手**明确拒绝**（变量名越界、nvram 报错）时不能再弹密码框，
        //    那只会让用户输一次密码、再失败一次，并把真正的原因埋掉。
        if crate::helper_install::installed() {
            match crate::helper_install::request(entries) {
                Ok(()) => return Ok(()),
                Err(crate::helper_install::HelperError::Rejected(text)) => {
                    return Err(NvramError::Failed(text))
                }
                // 没通路（launchd 没加载 / 超时）：退回密码框，功能不能因此失效
                Err(crate::helper_install::HelperError::Unavailable(_)) => {}
            }
        }
        if let Some(error) = gui_auth_forbidden() {
            return Err(NvramError::NeedElevation(error));
        }
        let script = elevated_nvram_script(entries);
        let output = match command("osascript").args(["-e", script.as_str()]).output() {
            Ok(output) => output,
            // 系统里没有 osascript（极罕见）：如实报第一条的真实原因，别掩盖
            Err(_) => return Err(first_error),
        };
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(NvramError::NeedElevation(if stderr.is_empty() {
            "管理员授权被取消，NVRAM 没有改动".to_string()
        } else {
            format!("管理员授权失败：{stderr}")
        }))
    }

    pub(crate) fn available() -> bool {
        Path::new("/usr/sbin/nvram").exists()
    }

    /// macOS **写不了** Boot*：Apple 的 EFI 运行时按白名单过滤 NVRAM 写入（实测
    /// root 也返回 `(iokit/common) not permitted`）。设置下次启动只能走 `bless`。
    pub(crate) fn can_write_boot_vars() -> bool {
        false
    }

    /// macOS 原生读得到 NVRAM（`/usr/sbin/nvram`），没有特权启用这一说。
    pub(crate) fn enable_system_environment_privilege() -> bool {
        true
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    // 注意：windows-sys 0.59 里这俩函数落在 WindowsProgramming 模块
    // （windows crate 才在 SystemInformation，别照搬）
    use windows_sys::Win32::System::WindowsProgramming::{
        GetFirmwareEnvironmentVariableW, SetFirmwareEnvironmentVariableW,
    };
    // 非提权进程读固件变量报 PRIVILEGE_NOT_HELD（1314），提权了但权限没启用报
    // ACCESS_DENIED（5）—— 两个都该翻译成「要提权」，绝不能当成「变量不存在」
    // （那是 Hidden 分支，会把权限问题误诊成「引导器托管」，实测踩过）。
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_PRIVILEGE_NOT_HELD,
    };

    /// 读写固件变量时两种「没权限」的错误码。
    fn is_elevation_error(error: u32) -> bool {
        error == ERROR_ACCESS_DENIED || error == ERROR_PRIVILEGE_NOT_HELD
    }

    fn utf16_key(name: &str, guid: &str) -> (Vec<u16>, Vec<u16>) {
        // GUID 要 UEFI 的 `{…}` 花括号形态
        (
            name.encode_utf16().chain(std::iter::once(0)).collect(),
            format!("{{{guid}}}")
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect(),
        )
    }

    pub(crate) fn read_var(name: &str, guid: &str) -> Result<Vec<u8>, NvramError> {
        let (name_w, guid_w) = utf16_key(name, guid);
        // `GetFirmwareEnvironmentVariableW` 没有「探大小」通道（不像 RegQueryValueEx），
        // 一次调用就得给足缓冲区。先按常见的 1 KB 起，报 122（缓冲区不够）再放大到 8 KB。
        // **绝不能把 122 当成「变量不存在」**：一条带长 DevicePath 的 `Boot####` 完全可能
        // 超过 1 KB，误判成 NotFound 会让上层得出「固件里没有这个引导项」这种相反的结论，
        // 而且全程没有任何报错 —— 比直接失败更难查。
        let read = |capacity: usize| -> Result<Vec<u8>, u32> {
            let mut buffer = vec![0u8; capacity];
            let size = unsafe {
                GetFirmwareEnvironmentVariableW(
                    name_w.as_ptr(),
                    guid_w.as_ptr(),
                    buffer.as_mut_ptr() as *mut core::ffi::c_void,
                    buffer.len() as u32,
                )
            };
            if size == 0 {
                return Err(unsafe { windows_sys::Win32::Foundation::GetLastError() });
            }
            buffer.truncate(size as usize);
            Ok(buffer)
        };
        let outcome = read(1024).or_else(|error| {
            if error == ERROR_INSUFFICIENT_BUFFER {
                read(8192)
            } else {
                Err(error)
            }
        });
        match outcome {
            Ok(value) => Ok(value),
            Err(error) if is_elevation_error(error) => Err(NvramError::NeedElevation(
                "读取 NVRAM 变量需要管理员权限".to_string(),
            )),
            Err(ERROR_INSUFFICIENT_BUFFER) => Err(NvramError::Failed(format!(
                "NVRAM 变量 {name} 超过 8 KB，读不下来（注意：这不是「变量不存在」）"
            ))),
            Err(error) => Err(NvramError::NotFound(format!(
                "NVRAM 变量 {name} 不存在（GetLastError={error}）"
            ))),
        }
    }

    pub(crate) fn write_var(name: &str, guid: &str, value: &[u8]) -> Result<(), NvramError> {
        if let Some(error) = gui_auth_forbidden() {
            return Err(NvramError::NeedElevation(error));
        }
        let (name_w, guid_w) = utf16_key(name, guid);
        // 非 Ex 版：缓冲区不带属性头，隐含 NV|BS|RT —— 正合 AnyBoot 的用途
        let ok = unsafe {
            SetFirmwareEnvironmentVariableW(
                name_w.as_ptr(),
                guid_w.as_ptr(),
                value.as_ptr() as *const core::ffi::c_void,
                value.len() as u32,
            )
        };
        if ok != 0 {
            return Ok(());
        }
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        if is_elevation_error(error) {
            return Err(NvramError::NeedElevation(
                "写 NVRAM 变量需要管理员权限（请以管理员身份运行 AnyBoot）".to_string(),
            ));
        }
        Err(NvramError::Failed(format!(
            "写 NVRAM 变量 {name} 失败（GetLastError={error}）"
        )))
    }

    pub(crate) fn available() -> bool {
        // Windows 上的 EFI 变量接口常在；真的没有会在 read 时报错
        true
    }

    /// 管理员进程下 `SetFirmwareEnvironmentVariableW` 能写 Boot*。
    pub(crate) fn can_write_boot_vars() -> bool {
        true
    }

    /// 批量写：Windows 用同一个进程权限，逐条写即可。
    pub(crate) fn write_vars(entries: &[(&str, &str, &[u8])]) -> Result<(), NvramError> {
        for (name, guid, value) in entries {
            write_var(name, guid, value)?;
        }
        Ok(())
    }

    /// 启用 `SeSystemEnvironmentPrivilege`。**提权 ≠ 特权已启用**：管理员令牌里
    /// 这条特权通常处于 disabled 态，`Get/SetFirmwareEnvironmentVariableW` 要求
    /// enabled 才肯干活（否则报 5/1314，和「没提权」一个症状，极易误诊）。
    /// 提权子进程（dump / 设默认）读写变量前都该先调一次。
    pub(crate) fn enable_system_environment_privilege() -> bool {
        use windows_sys::Win32::Security::{
            AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
            SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
            TOKEN_QUERY,
        };
        // windows-sys 0.59 的坑位：OpenProcessToken 不在 Security，在 Threading
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        unsafe {
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            ) == 0
            {
                return false;
            }
            let mut luid = core::mem::zeroed();
            // 特权名不含 \\（本机），后缀补 NUL
            let name: Vec<u16> = "SeSystemEnvironmentPrivilege"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) == 0 {
                windows_sys::Win32::Foundation::CloseHandle(token);
                return false;
            }
            let tp = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            let ok = AdjustTokenPrivileges(
                token,
                0,
                &tp,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            // 这个 API 的坑：**即使令牌里根本没有这条特权，它也返回非零**，只在
            // GetLastError 里留 ERROR_NOT_ALL_ASSIGNED。不查这一下，返回值就是谎话 ——
            // 而它存在的意义正是区分「没提权」与「提权了但特权没启用」这两种同症状的失败。
            let assigned = ok != 0
                && windows_sys::Win32::Foundation::GetLastError()
                    != windows_sys::Win32::Foundation::ERROR_NOT_ALL_ASSIGNED;
            windows_sys::Win32::Foundation::CloseHandle(token);
            assigned
        }
    }
}

// ─────────────────────────── 平台无关入口 ───────────────────────────

pub(crate) fn read_var(name: &str, guid: &str) -> Result<Vec<u8>, NvramError> {
    imp::read_var(name, guid)
}

pub(crate) fn write_var(name: &str, guid: &str, value: &[u8]) -> Result<(), NvramError> {
    imp::write_var(name, guid, value)
}

/// 一次写多条变量（启动项 = `Boot####` + `BootOrder`，两条必须一起改才有意义）。
pub(crate) fn write_vars(entries: &[(&str, &str, &[u8])]) -> Result<(), NvramError> {
    imp::write_vars(entries)
}

/// 本机固件变量接口可用吗（不是 UEFI 启动 / 没有工具时 false）。
/// 留给「通道不可用」的 UI 提示与冒烟自检；当前主链路直接读，失败即报错。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn available() -> bool {
    imp::available()
}

/// Boot#### 变量名 → 编号（`Boot0003` → 3）。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn boot_var_number(name: &str) -> Option<u16> {
    let rest = name.strip_prefix("Boot")?;
    if rest.len() != 4 || !rest.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(rest, 16).ok()
}

/// OS 侧能不能写 `Boot*` 变量（`BootNext` / `Boot####` / `BootOrder`）。
///
/// macOS 恒 false：Apple 的 EFI 运行时对 NVRAM 有变量白名单，`Boot####` 不在其中，
/// **root 也一样**（实测 `nvram 8be4df61…:Boot0081=%01%00…` → `(iokit/common) not
/// permitted`）。macOS 上设置下次启动的唯一正规渠道是 `bless`。Linux（efivarfs +
/// 提权）与 Windows（`SetFirmwareEnvironmentVariableW`）都能直接写。
pub(crate) fn can_write_boot_vars() -> bool {
    imp::can_write_boot_vars()
}

/// `BootCurrent` 读不到时的三种处境。**修法完全不同**，所以不能合成一个 bool：
///
/// * `Visible` —— 读得到，`BootOrder` 就是固件真相。
/// * `Hidden` —— 变量不存在：引导器（OC 的 `UEFI→Quirks→RequestBootVarRouting`）
///   把 `Boot*` 改写到自己命名空间，或固件不暴露。再提权也没用。
/// * `NeedElevation` —— 变量在，但本进程没权限读：Windows 非管理员进程读 EFI 变量
///   需要 `SeSystemEnvironmentPrivilege`（`GetFirmwareEnvironmentVariableW` 直接
///   `ACCESS_DENIED`）。这时说「被引导器托管」是**误判**，用户按提示去查 OC 配置
///   只会白费功夫——真因是「用管理员身份重开一次」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BootVarsView {
    Visible,
    Hidden,
    NeedElevation,
}

impl BootVarsView {
    /// 拿不到真相（无论因为托管还是权限）→ 不能拿 `BootOrder` 反推「固件里有没有某项」。
    pub(crate) fn is_blind(self) -> bool {
        !matches!(self, Self::Visible)
    }
}

/// 判据用 `BootCurrent`：真实 UEFI 一定有它（本次是从哪个 `Boot####` 启动的）。
///
/// 看不见时**不能拿 `BootOrder` 去判断"固件里有没有某个引导项"**：那张表可能只残留
/// 当前系统那一项，而固件里其实早已注册好 OpenCore（BIOS 启动菜单里看得见、也确实
/// 能从中启动）。早期版本就是在这里判错的。
pub(crate) fn boot_vars_view() -> BootVarsView {
    match read_var("BootCurrent", GUID_GLOBAL) {
        Ok(_) => BootVarsView::Visible,
        Err(NvramError::NeedElevation(_)) => BootVarsView::NeedElevation,
        Err(_) => BootVarsView::Hidden,
    }
}

/// 读 BootOrder（不存在返回空序列 —— 固件没设过 BootOrder 也是合法状态）。
pub(crate) fn read_boot_order() -> Vec<u16> {
    read_var("BootOrder", GUID_GLOBAL)
        .map(|value| parse_boot_order(&value))
        .unwrap_or_default()
}

/// 读一个 Boot#### 项的 LOAD_OPTION 原值（不存在 → None）。
pub(crate) fn read_boot_option(number: u16) -> Option<Vec<u8>> {
    read_var(&boot_var_name(number), GUID_GLOBAL).ok()
}

/// 启用 `SeSystemEnvironmentPrivilege`（Windows 有实际动作，其他平台恒 true）。
/// 提权子进程读写 NVRAM 前都该先调 —— 提权 ≠ 特权已启用。
pub(crate) fn enable_system_environment_privilege() -> bool {
    imp::enable_system_environment_privilege()
}

// ─────────────────────────── NVRAM dump（提权子命令） ───────────────────────────

/// dump 出来的一个变量：名字 + GUID + 原始字节 hex。只收录**读到**的，
/// 不存在的变量不进列表（调用方看 count 就知道 OC 到底写没写过槽）。
///
/// `Deserialize` 是为了父进程能解析提权子进程回传的这份数据（见
/// [`BootVarsSnapshot`]）。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DumpVar {
    pub(crate) name: String,
    pub(crate) guid: String,
    pub(crate) len: usize,
    pub(crate) hex: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct DumpFile {
    pub(crate) ok: bool,
    /// 特权启用是否成功（Windows：提权 ≠ SeSystemEnvironmentPrivilege 已启用）
    pub(crate) privilege_enabled: bool,
    pub(crate) count: usize,
    pub(crate) vars: Vec<DumpVar>,
    pub(crate) error: Option<String>,
}

/// 读一组「OC 私有 + 固件全局」启动变量。槽位全扫 0x0000-0x00FF：
/// 不存在的变量一次 API 调用就返回，开销可忽略；OC 的槽编号跟随 BootOrder，
/// 不假设它只用哪半区。
pub(crate) fn dump_boot_vars() -> Vec<DumpVar> {
    let _ = imp::enable_system_environment_privilege();
    let oc_guid = super::oc_default::GUID_OC_VENDOR;
    let mut vars = Vec::new();
    let mut push = |name: String, guid: &str| {
        if let Ok(value) = read_var(&name, guid) {
            vars.push(DumpVar {
                hex: value
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                name,
                guid: guid.to_string(),
                len: value.len(),
            });
        }
    };
    push("OCBtOrder".to_string(), oc_guid);
    push("OCBtNext".to_string(), oc_guid);
    push("BootOrder".to_string(), GUID_GLOBAL);
    push("BootCurrent".to_string(), GUID_GLOBAL);
    push("BootNext".to_string(), GUID_GLOBAL);
    for number in 0x0000u16..=0x00FF {
        push(boot_var_name(number), GUID_GLOBAL);
        push(format!("OCBt{}", boot_var_name(number).trim_start_matches("Boot")), oc_guid);
    }
    vars
}

/// 读 OCBt*/Boot* 全量并序列化成 JSON 文本（文件路径与特权服务共用同一份）。
pub(crate) fn privileged_dump_json() -> Result<String, String> {
    let privilege_enabled = imp::enable_system_environment_privilege();
    let vars = dump_boot_vars();
    let count = vars.len();
    let payload = DumpFile {
        ok: true,
        privilege_enabled,
        count,
        vars,
        error: None,
    };
    serde_json::to_string(&payload).map_err(|error| format!("序列化 dump 结果失败：{error}"))
}

/// `--nvram-dump-privileged` 子命令体：提权进程里读 OCBt*/Boot* 全量，
/// hex 落 JSON。两个用途：① 排障时直接看 OC 到底写了什么槽；
/// ② 「路径 2」（凭真实字节复刻 APFS load option）的样本采集。
/// 只在提权子进程里被调（main.rs ⓪.7 分支），绝不进 GUI。
pub(crate) fn privileged_dump_main(out_path: &str) -> i32 {
    match privileged_dump_json() {
        Ok(text) => super::write_result_file(out_path, &text),
        Err(error) => {
            eprintln!("{error}");
            2
        }
    }
}

// ─────────────── 提权读到的启动项快照（主进程没权限读时靠它回显） ───────────────

/// 一次提权读取拿到的启动项视图。
///
/// 为什么需要它：Windows 上非管理员进程读固件变量直接报 1314，而 OC 私有的
/// `OCBtOrder` / `OCBt####` 只是**另一个 GUID 下的同样受保护**的变量 —— 一样读不到。
/// 于是预演的启动顺序表整表为空，「默认项」标记永远点不亮：用户点了「设为默认」、
/// 提示成功，页面上却毫无变化（真机踩到）。提权子进程本来就要读这张表，顺手把
/// 读到的结果带回来，主进程没权限时用它回显。
#[derive(Clone, Debug)]
pub(crate) struct BootVarsSnapshot {
    /// `true` = OC 私有 `OCBt*`；`false` = 固件全局 `Boot*`
    pub(crate) custom: bool,
    pub(crate) order: Vec<u16>,
    /// 槽编号 → `EFI_LOAD_OPTION` 原始字节（只收 order 里出现且读到的）
    pub(crate) options: Vec<(u16, Vec<u8>)>,
    pub(crate) taken_at: std::time::Instant,
}

/// 与 Preboot 提权缓存同档。启动顺序变动很少，30 分钟只是「NVRAM 可能被别的
/// 系统动过、缓存该失效」的护栏。
const BOOT_VARS_SNAPSHOT_TTL: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// 只保留两个命名空间各一份（后写入的覆盖旧的）。
static BOOT_VARS_SNAPSHOTS: std::sync::OnceLock<std::sync::Mutex<Vec<BootVarsSnapshot>>> =
    std::sync::OnceLock::new();

fn snapshot_cell() -> &'static std::sync::Mutex<Vec<BootVarsSnapshot>> {
    BOOT_VARS_SNAPSHOTS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// 存快照（同一命名空间的旧快照被替换）。
pub(crate) fn store_boot_vars_snapshots(items: Vec<BootVarsSnapshot>) {
    let Ok(mut guard) = snapshot_cell().lock() else {
        return;
    };
    for item in items {
        guard.retain(|old| old.custom != item.custom);
        guard.push(item);
    }
}

/// 取 TTL 内的快照；没有/过期 → None。
pub(crate) fn cached_boot_vars(custom: bool) -> Option<BootVarsSnapshot> {
    let guard = snapshot_cell().lock().ok()?;
    guard
        .iter()
        .find(|item| item.custom == custom && item.taken_at.elapsed() < BOOT_VARS_SNAPSHOT_TTL)
        .cloned()
}

/// `"80 00 82 00"` → `[80, 00, 82, 00]`。
fn parse_hex_bytes(text: &str) -> Option<Vec<u8>> {
    text.split_whitespace()
        .map(|part| u8::from_str_radix(part, 16).ok())
        .collect()
}

/// dump 出来的变量列表 → 各命名空间的快照（读不到对应 order 的那个不产出）。
pub(crate) fn snapshots_from_vars(vars: &[DumpVar]) -> Vec<BootVarsSnapshot> {
    let oc_guid = super::oc_default::GUID_OC_VENDOR;
    let mut out = Vec::new();
    for custom in [true, false] {
        let (order_name, guid) = if custom {
            ("OCBtOrder", oc_guid)
        } else {
            ("BootOrder", GUID_GLOBAL)
        };
        let Some(order_var) = vars
            .iter()
            .find(|var| var.name.eq_ignore_ascii_case(order_name) && var.guid.eq_ignore_ascii_case(guid))
        else {
            continue;
        };
        let Some(raw) = parse_hex_bytes(&order_var.hex) else {
            continue;
        };
        let order = parse_boot_order(&raw);
        if order.is_empty() {
            continue;
        }
        let mut options = Vec::new();
        for number in &order {
            let wanted = if custom {
                format!("OCBt{number:04X}")
            } else {
                boot_var_name(*number)
            };
            if let Some(var) = vars
                .iter()
                .find(|var| var.name.eq_ignore_ascii_case(&wanted) && var.guid.eq_ignore_ascii_case(guid))
            {
                if let Some(value) = parse_hex_bytes(&var.hex) {
                    options.push((*number, value));
                }
            }
        }
        out.push(BootVarsSnapshot {
            custom,
            order,
            options,
            taken_at: std::time::Instant::now(),
        });
    }
    out
}

/// 直接拿一批 dump 变量更新快照；返回是否至少读到一份启动顺序表。
pub(crate) fn store_snapshots_from_vars(vars: Vec<DumpVar>) -> bool {
    let snapshots = snapshots_from_vars(&vars);
    let hit = !snapshots.is_empty();
    store_boot_vars_snapshots(snapshots);
    hit
}

/// 弹一次授权，提权读一遍 `OCBt*` / `Boot*` 全量，把启动顺序表存进进程内缓存。
///
/// **必须在 async 命令的工作线程上调**（Windows 的 ShellExecuteExW 同步等 UAC）。
#[cfg(any(target_os = "windows", target_os = "linux", test))]
pub(crate) fn refresh_boot_vars_elevated() -> Result<String, String> {
    let out_path = super::elevate::result_file_path("nvram-dump");
    let args = vec![
        "--nvram-dump-privileged".to_string(),
        "--out".to_string(),
        out_path.to_string_lossy().to_string(),
    ];
    super::elevate::run_privileged_op("nvram_dump", serde_json::json!({}), &args, &out_path)?;
    let text = std::fs::read_to_string(&out_path)
        .map_err(|error| format!("提权子进程退出了但结果文件读不到：{error}"))?;
    let file: DumpFile = serde_json::from_str(&text)
        .map_err(|error| format!("提权读取的结果不是合法 JSON：{error}"))?;
    if !file.ok {
        return Err(file
            .error
            .unwrap_or_else(|| "提权读取失败（子进程没给原因）".to_string()));
    }
    let snapshots = snapshots_from_vars(&file.vars);
    if snapshots.is_empty() {
        return Err(
            "提权读到了 NVRAM，但里面没有启动顺序表（`OCBtOrder` / `BootOrder` 都读不到）：\
             OC 还没在这个系统上记过默认项"
                .to_string(),
        );
    }
    let summary = snapshots
        .iter()
        .map(|snap| {
            format!(
                "{} {} 项",
                if snap.custom { "OCBtOrder" } else { "BootOrder" },
                snap.order.len()
            )
        })
        .collect::<Vec<_>>()
        .join(" / ");
    store_boot_vars_snapshots(snapshots);
    Ok(format!("已提权读取启动顺序（{summary}），本页已刷新"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attr_header_roundtrip() {
        let value = boot_next_value(0x0003);
        let raw = with_attr_header(&value);
        assert_eq!(raw.len(), 6);
        assert_eq!(&raw[..4], &[0x07, 0x00, 0x00, 0x00]);
        assert_eq!(strip_attr_header(&raw), Some(&[0x03, 0x00][..]));
    }

    fn dump_var(name: &str, guid: &str, hex: &str) -> DumpVar {
        DumpVar {
            name: name.to_string(),
            guid: guid.to_string(),
            len: hex.split_whitespace().count(),
            hex: hex.to_string(),
        }
    }

    #[test]
    fn snapshots_split_by_namespace_and_keep_only_order_slots() {
        let oc = super::super::oc_default::GUID_OC_VENDOR;
        let vars = vec![
            // OC 私有：80 在首位（用户按过 Ctrl+Enter 之后就是这个样子）
            dump_var("OCBtOrder", oc, "80 00 82 00 81 00"),
            dump_var("OCBt0080", oc, "01 00 00 00 02 00 4D 00 41 00 43 00"),
            // 固件全局：顺序不同，值也不同 —— 两套不能混
            dump_var("BootOrder", GUID_GLOBAL, "82 00 80 00"),
            dump_var("Boot0082", GUID_GLOBAL, "01 00 00 00 00 00 00 00"),
        ];
        let snapshots = snapshots_from_vars(&vars);
        assert_eq!(snapshots.len(), 2);

        let private = snapshots
            .iter()
            .find(|snap| snap.custom)
            .expect("OCBt 命名空间快照");
        assert_eq!(private.order, vec![0x0080, 0x0082, 0x0081]);
        // 只收「order 里出现 + 真的读到」的槽：OCBt0081/0082 没给值就不进列表
        assert_eq!(private.options.len(), 1);
        assert_eq!(private.options[0].0, 0x0080);
        // 4 字节属性 + 2 字节路径长度 + "MAC" 的 UTF-16（6 字节）
        assert_eq!(private.options[0].1.len(), 12);

        let global = snapshots
            .iter()
            .find(|snap| !snap.custom)
            .expect("Boot 命名空间快照");
        assert_eq!(global.order, vec![0x0082, 0x0080]);
        assert_eq!(global.options.len(), 1);
    }

    #[test]
    fn snapshots_skip_namespace_without_order() {
        // 只有槽没有顺序表 → 无法确定默认项，不产出
        let oc = super::super::oc_default::GUID_OC_VENDOR;
        let snapshots = snapshots_from_vars(&[dump_var(
            "OCBt0080",
            oc,
            "01 00 00 00 02 00 4D 00 41 00 43 00",
        )]);
        assert!(snapshots.is_empty());
        // 空的 order 值同样不算
        assert!(snapshots_from_vars(&[dump_var("OCBtOrder", oc, "")]).is_empty());
    }

    #[test]
    fn boot_order_parsing() {
        assert_eq!(parse_boot_order(&[1, 0, 3, 0, 0, 0]), vec![1, 3, 0]);
        assert_eq!(parse_boot_order(&[]), Vec::<u16>::new());
        assert_eq!(parse_boot_order(&[5, 0]), vec![5]);
    }

    #[test]
    fn boot_var_names() {
        assert_eq!(boot_var_name(3), "Boot0003");
        assert_eq!(boot_var_name(0x0A2F), "Boot0A2F");
        assert_eq!(boot_var_number("Boot0003"), Some(3));
        assert_eq!(boot_var_number("Boot0A2F"), Some(0x0A2F));
        assert_eq!(boot_var_number("Boot"), None);
        assert_eq!(boot_var_number("Boot12"), None);
        assert_eq!(boot_var_number("BootXYZ1"), None);
    }

    #[test]
    fn load_option_marker_search() {
        // 手工构造一份最小 LOAD_OPTION：
        // Attributes(u32=1) + FilePathListLength(u16) + 描述 UTF-16 "Windows Boot Manager"
        // + 双 NUL + DevicePath（UTF-16 路径里含 \EFI\OC\OpenCore.efi）
        fn utf16le(text: &str) -> Vec<u8> {
            let mut bytes = Vec::new();
            for unit in text.encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            bytes
        }
        let mut oc = vec![1, 0, 0, 0, 10, 0];
        oc.extend_from_slice(&utf16le("Windows Boot Manager"));
        oc.extend_from_slice(&[0, 0]); // 描述终止
        oc.extend_from_slice(&utf16le("\\EFI\\OC\\OpenCore.efi"));
        oc.extend_from_slice(&[0, 0]);
        assert!(load_option_matches(&oc, "OC\\OpenCore.efi"));
        assert!(!load_option_matches(&oc, "CLOVER\\CLOVERX64.efi"));
        // 描述里恰含路径串也能命中（宽松匹配的已知行为，方向是对的）
        assert_eq!(load_option_description(&oc).as_deref(), Some("Windows Boot Manager"));
        // 小写路径也能命中（FAT 大小写不改写语义）
        let mut lower = vec![1, 0, 0, 0, 0, 0];
        lower.extend_from_slice(&utf16le("\\efi\\oc\\opencore.efi"));
        assert!(load_option_matches(&lower, "OC\\OpenCore.efi"));
        // 空值/残缺值不 panic
        assert!(!load_option_matches(&[], "OC\\OpenCore.efi"));
        assert!(!load_option_matches(&[1, 0], "OC\\OpenCore.efi"));
        assert_eq!(load_option_description(&[]), None);
    }

    #[test]
    fn nvram_escape_roundtrip() {
        let value = vec![0x07, 0x00, 0x0A, 0xFF, b'a'];
        let encoded = encode_nvram_escaped(&value);
        assert_eq!(decode_nvram_escaped(&encoded), value);
        // 可打印字符原样
        assert_eq!(decode_nvram_escaped("arch.conf"), b"arch.conf".to_vec());
        // 字面 % 会被转义成 %25，解码无歧义
        assert_eq!(decode_nvram_escaped("%25"), b"%".to_vec());
    }

    #[test]
    fn guid_filename_form() {
        assert_eq!(
            efivar_filename("LoaderEntryOneShot", GUID_SYSTEMD),
            "LoaderEntryOneShot-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f"
        );
    }

    #[test]
    fn elevation_script_batches_all_writes_in_one_prompt() {
        // Boot#### + BootOrder 必须塞进同一条 AppleScript：两条命令一次授权框
        let entry_value = [1u8, 0, 0, 0];
        let order_value = [0x80u8, 0, 0x81, 0];
        let script = elevated_nvram_script(&[
            ("Boot0081", GUID_GLOBAL, &entry_value),
            ("BootOrder", GUID_GLOBAL, &order_value),
        ]);
        assert!(script.starts_with("do shell script \""));
        assert!(script.ends_with("\" with administrator privileges"));
        assert!(script.contains(&format!("{GUID_GLOBAL}:Boot0081=%01%00%00%00")));
        assert!(script.contains(&format!("{GUID_GLOBAL}:BootOrder=%80%00%81%00")));
        assert_eq!(script.matches("nvram").count(), 2);
        assert_eq!(script.matches(" with administrator privileges").count(), 1);
    }

    #[test]
    fn write_arg_is_a_single_argv() {
        // 真机踩到过的坑：`nvram <key> =%XX…` 是两个参数 —— nvram 把第一个当读取、
        // 第二个当「设置空名变量」，退出码 0 却什么都没写。整条 `key=value` 必须
        // 是同一个 argv（不含空格，直接能当 shell 参数用）。
        let arg = nvram_write_arg("OCBtOrder", "4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102", &[0x80, 0x00]);
        assert_eq!(arg, "4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102:OCBtOrder=%80%00");
        assert!(!arg.contains(' '), "整条必须是一个 argv，中间不许有空格");
        assert_eq!(arg.matches('=').count(), 1, "只有一个等号（值是 % 转义，不会有裸等号）");
        // 提权脚本里的每条命令也必须是这个形态
        let script = elevated_nvram_script(&[("OCBtOrder", "4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102", &[0x80, 0x00])]);
        assert!(script.contains(&format!("/usr/sbin/nvram {arg}")));
    }
}