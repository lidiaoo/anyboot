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
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
    // 注意：windows-sys 0.59 里这俩函数落在 WindowsProgramming 模块
    // （windows crate 才在 SystemInformation，别照搬）
    use windows_sys::Win32::System::WindowsProgramming::{
        GetFirmwareEnvironmentVariableW, SetFirmwareEnvironmentVariableW,
    };

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
        let mut buffer = [0u8; 1024];
        // GetFirmwareEnvironmentVariableW 没有「探大小」通道（不像 RegQueryValueEx），
        // 直接单次调用拿全量；超过 1KB 的固件变量不在 AnyBoot 的用途里（BootNext 等
        // 都是十几字节）
        let size = unsafe {
            GetFirmwareEnvironmentVariableW(
                name_w.as_ptr(),
                guid_w.as_ptr(),
                buffer.as_mut_ptr() as *mut core::ffi::c_void,
                buffer.len() as u32,
            )
        };
        if size == 0 {
            let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if error == ERROR_ACCESS_DENIED {
                return Err(NvramError::NeedElevation(
                    "读取 NVRAM 变量需要管理员权限".to_string(),
                ));
            }
            return Err(NvramError::NotFound(format!(
                "NVRAM 变量 {name} 不存在（GetLastError={error}）"
            )));
        }
        Ok(buffer[..size as usize].to_vec())
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
        if error == ERROR_ACCESS_DENIED {
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