//! 远程接入网关 · cloudflared 二进制管理。
//!
//! 设计定则（design/remote-gateway-ui.html ⑤清单⑥ / 2026-10-23 调研）：
//! - **免安装**：三端都只用官方 release 的独立二进制，放进配置目录 `gateway/`
//!   缓存，不注册系统服务、不要管理员权限。win=exe 直接跑；linux=chmod +x；
//!   macOS=tgz 解压（App 自下载不经浏览器，不落 quarantine，无 Gatekeeper 拦截）。
//! - **版本固定 + 内置哈希表**：官方 GitHub release 不发 checksums 文件，业界
//!   先例（Chocolatey 包、各开源 binary_manager）都是「钉死版本 + 代码内置
//!   SHA-256」。bump [`CLOUDFLARED_VERSION`] 时必须同步刷新 [`SHA256_TABLE`]。
//! - **校验失败即删**：下载产物先落 `.tmp`，哈希对不上立刻删除并报错，永不
//!   执行未校验的二进制（#29 下载器按此约定落盘）。
//! - 用户手动指定的本地 exe 路径**不做哈希强校验**（兜底逃生口），但会跑
//!   `--version` 验证可执行并回读版本号（见 [`probe_manual_binary`])。

use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

/// 钉死的 cloudflared 版本（2026.8.3 为 2026-10 时点最新稳定版）。
/// bump 版本时：① 改这里 ② 刷新 [`SHA256_TABLE`] 五条哈希 ③ 跑一遍下载验证。
pub(crate) const CLOUDFLARED_VERSION: &str = "2026.8.3";

/// 官方 release 下载基址。
pub(crate) fn release_base_url() -> String {
    format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{CLOUDFLARED_VERSION}"
    )
}

/// 网关链路埋点：**无条件**追加 trace.log（对齐 `hub_power::power_log`）——
/// 「点了开启通道没反应」这类问题必须能查到按没按、走到哪、错在哪，
/// 不能依赖 debug_trace 开关。写入失败全部吞掉，绝不影响通道本身。
/// 注意：消息自带 `[gateway]` 前缀，这里只补 `[tunnel]` 类别与时间戳。
pub(crate) fn gw_log(app: &AppHandle, message: &str) {
    let line = format!("{}ms [tunnel] {message}\n", crate::unix_millis());
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
        .and_then(|mut file| file.write_all(line.as_bytes()));
}

/// 配置目录下的 gateway 子目录（二进制缓存根）。
pub(crate) fn cache_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = crate::config_dir(app)?.join("gateway");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("无法创建 gateway 缓存目录 {}: {error}", dir.display()))?;
    Ok(dir)
}

// ==================== 平台映射 ====================

/// 本机平台键 (system, arch)，映射失败（暂不支持的架构）给明确报错。
pub(crate) fn platform_key() -> Result<(&'static str, &'static str), String> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Ok(("windows", "amd64"));
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Ok(("linux", "amd64"));
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Ok(("linux", "arm64"));
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Ok(("darwin", "amd64"));
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Ok(("darwin", "arm64"));
    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )))]
    Err("当前平台/架构暂无内置 cloudflared 映射，请在远程接入页手动指定本地二进制路径".to_string())
}

/// 官方 release 资产名。darwin 是 .tgz（解压后取同名无扩展二进制）。
pub(crate) fn asset_name(key: (&str, &str)) -> &'static str {
    match key {
        ("windows", "amd64") => "cloudflared-windows-amd64.exe",
        ("linux", "amd64") => "cloudflared-linux-amd64",
        ("linux", "arm64") => "cloudflared-linux-arm64",
        ("darwin", "amd64") => "cloudflared-darwin-amd64.tgz",
        ("darwin", "arm64") => "cloudflared-darwin-arm64.tgz",
        // 不可达：platform_key 只会返回上面五种键。保底一个合法名字防 panic。
        _ => "cloudflared-linux-amd64",
    }
}

/// 资产下载 URL。
pub(crate) fn asset_url(key: (&str, &str)) -> String {
    format!("{}/{}", release_base_url(), asset_name(key))
}

// ==================== 内置 SHA-256 表 ====================
// 全部五条为 2026.8.3 官方 release 产物实测值（2026-10-25 逐条下载 shasum -a 256 核对）。
// **bump 版本必须整表刷新**——首版造表时 darwin 两条误填了解压后内核二进制的哈希
// （tgz 整包 ≠ 内核），导致 macOS 下载三源全被「哈希校验失败」拒装（Windows 能过掩盖了它）。

/// (system, arch) -> 资产 SHA-256（darwin 校验的是 .tgz 整包，非解压产物）。
pub(crate) fn expected_sha256(key: (&str, &str)) -> Option<&'static str> {
    match key {
        ("windows", "amd64") => Some("83e726ed18ea78c5ad5213c4c3a3a27051393950d2bc8ed4de69bec12d14eaae"),
        ("linux", "amd64") => Some("f29324fe934d1e100617484c78deef803c4dc2cd351d645bbde42e96b4fccc5e"),
        ("linux", "arm64") => Some("4bcfd35521a7cbc545ebfd5d57334a71ee180e2a64874981f374c81472118391"),
        ("darwin", "amd64") => Some("61e1316266a00fd70ce40da011d612badc805367fb65293dd1925f938f704c99"),
        ("darwin", "arm64") => Some("40c9144d86df8937c5b43293a1f7d2d2107029aa74725023dd46b1b27154352f"),
        _ => None,
    }
}

// ==================== 缓存路径与校验 ====================

/// 缓存的正式二进制路径（darwin 存解压产物，非 .tgz 本体）。
pub(crate) fn binary_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let name = if cfg!(target_os = "windows") {
        "cloudflared.exe"
    } else {
        "cloudflared"
    };
    Ok(cache_dir(app)?.join(name))
}

/// 对文件流式算 SHA-256（二进制 50MB 级，不整读进内存）。
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| format!("无法打开 {} 计算哈希: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)
            .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 下载产物验收：哈希匹配才从 `.tmp` 原子改名为正式路径；不匹配立即删除并报错。
/// 返回正式二进制路径。
pub(crate) fn verify_and_install(
    app: &tauri::AppHandle,
    tmp_path: &Path,
) -> Result<PathBuf, String> {
    let key = platform_key()?;
    let expected = expected_sha256(key)
        .ok_or_else(|| "当前平台无内置校验哈希，拒绝安装未验证的二进制".to_string())?;
    let actual = sha256_file(tmp_path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        let _ = std::fs::remove_file(tmp_path);
        return Err(format!(
            "cloudflared 哈希校验失败（预期 {expected}，实际 {actual}）。\
             已删除下载文件，可重试下载或手动指定本地二进制路径"
        ));
    }
    let final_path = binary_path(app)?;
    // Windows 上 rename 不能覆盖已存在文件：先移走旧的（失败说明正被占用，
    // 此刻不应有进程在跑——通道关闭时才会换二进制）。
    if final_path.exists() {
        let backup = final_path.with_extension("old");
        let _ = std::fs::remove_file(&backup);
        std::fs::rename(&final_path, &backup)
            .map_err(|error| format!("旧二进制被占用，无法替换: {error}"))?;
    }
    // darwin 下载物是 .tgz：哈希对的是整包，解出内核 cloudflared 再落正式位。
    #[cfg(target_os = "macos")]
    {
        let inner = extract_darwin(app, tmp_path)?;
        std::fs::rename(&inner, &final_path)
            .map_err(|error| format!("解压产物落盘失败: {error}"))?;
        let _ = std::fs::remove_file(tmp_path);
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::rename(tmp_path, &final_path)
            .map_err(|error| format!("二进制落盘失败: {error}"))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&final_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("chmod +x 失败: {error}"))?;
    }
    gw_log(
        app,
        &format!("[gateway] cloudflared {CLOUDFLARED_VERSION} 安装完成: {}", final_path.display()),
    );
    Ok(final_path)
}

/// macOS 专属：用系统 /usr/bin/tar 解开校验过的 .tgz，返回内核二进制路径。
/// 只在 darwin 编译（其余平台不下载 .tgz，无需引 tar/flate2 依赖）。
#[cfg(target_os = "macos")]
fn extract_darwin(app: &tauri::AppHandle, tgz: &Path) -> Result<PathBuf, String> {
    let out_dir = tgz.parent().unwrap_or_else(|| Path::new(".")).join("extract");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir)
        .map_err(|error| format!("创建解压目录失败: {error}"))?;
    let status = std::process::Command::new("/usr/bin/tar")
        .args(["-xzf"])
        .arg(tgz)
        .arg("-C")
        .arg(&out_dir)
        .status()
        .map_err(|error| format!("启动 tar 失败: {error}"))?;
    if !status.success() {
        return Err(format!("解压 .tgz 失败（tar 退出码 {:?}）", status.code()));
    }
    let inner = out_dir.join("cloudflared");
    if !inner.is_file() {
        return Err("解压后未找到 cloudflared 内核".to_string());
    }
    let _ = app;
    Ok(inner)
}

/// 二进制就绪状态（前端「通道卡」的安装态数据源）。
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct BinaryStatus {
    pub installed: bool,
    pub path: String,
    pub version: String,
}

/// 查询缓存二进制状态：存在且 `--version` 可跑才算 installed。
pub(crate) fn binary_status(app: &tauri::AppHandle) -> BinaryStatus {
    let path = binary_path(app).unwrap_or_default();
    let installed = !path.as_os_str().is_empty() && path.is_file();
    let version = if installed {
        probe_version(&path).unwrap_or_default()
    } else {
        String::new()
    };
    BinaryStatus {
        installed,
        path: path.display().to_string(),
        version,
    }
}

/// 跑 `--version` 回读版本串（手动指定路径的验证也用它）。
///
/// **必须带 CREATE_NO_WINDOW**：这条在每次渲染通道卡（切进远程接入页）、点开启通道、
/// 快速/命名隧道切换时都会跑 —— 之前没带，Windows 上每跑一次就闪一个黑框，
/// 用户观感就是「随便点什么都会闪个黑窗口」。
pub(crate) fn probe_version(path: &Path) -> Result<String, String> {
    let mut command = std::process::Command::new(path);
    command.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command
        .output()
        .map_err(|error| format!("无法启动 {}: {error}", path.display()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() && text.trim().is_empty() {
        return Err(format!("{} 退出码 {:?}", path.display(), output.status.code()));
    }
    // 形如 "cloudflared version 2026.8.3 (built ...)"，只留版本号。
    let version = text
        .lines()
        .find_map(|line| line.split_whitespace().nth(2))
        .unwrap_or(text.trim());
    Ok(version.trim().to_string())
}

/// 手动指定路径验收：可执行 + `--version` 能回读。不做哈希强校验（逃生口），
/// 前端会把手动路径标记为「未内置校验」。
pub(crate) fn probe_manual_binary(path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("路径不是文件: {}", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !path.metadata().map_err(|e| e.to_string())?.permissions().mode() & 0o111 != 0 {
            return Err("文件没有可执行权限（chmod +x 后再试）".to_string());
        }
    }
    probe_version(path)
}

// ==================== 下载器 ====================
// 约定：纯线程模型（IQ6）——std::thread 里跑阻塞下载，进度/结果用事件推前端；
// 同一时刻只允许一个下载（AtomicBool 防重入）。GitHub 直连经常被墙，按
// 「直连 → 镜像加速 → 报错（用户走手动指定路径兜底）」顺序尝试。

/// 下载进度/结果事件。
pub(crate) const EVENT_DL_PROGRESS: &str = "gateway-binary-progress";
pub(crate) const EVENT_DL_DONE: &str = "gateway-binary-done";

/// 镜像加速前缀（拼在完整 GitHub URL 前）。这些公益站时常换域名，失效不致命
/// ——还有手动指定路径兜底；列表写在代码里便于随版本更新。
const MIRROR_PREFIXES: [&str; 2] = ["https://ghfast.top/", "https://gh-proxy.com/"];

/// 下载重入守卫。
static DL_IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct DownloadProgress {
    pub stage: String,     // "downloading" | "mirror-retry"
    pub source: String,    // 当前源标签
    pub received: u64,
    pub total: u64,
    pub percent: u32,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct DownloadDone {
    pub ok: bool,
    pub error: String,
    pub installed: bool,
    pub path: String,
    pub version: String,
}

/// 带进度回调的流式下载（cloudflared 与 Tailscale 安装包共用）。
/// 回调按 ~120ms 节流 —— 每 256KB 都 emit 一次会把 webview 淹掉；但末块一定发，
/// 保证进度条能走到 100%（否则停在 98% 看着像卡死）。
pub(crate) fn download_to_tmp_observed(
    url: &str,
    tmp: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(u64, u64), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(15))
        .timeout_read(std::time::Duration::from_secs(30))
        .build();
    let response = agent
        .get(url)
        .call()
        .map_err(|error| format!("HTTP 请求失败: {error}"))?;
    let total = response
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(tmp)
        .map_err(|error| format!("创建临时文件失败: {error}"))?;
    let mut buffer = [0u8; 256 * 1024];
    let mut received: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("下载中断 @ {received}: {error}"))?;
        if read == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buffer[..read])
            .map_err(|error| format!("写临时文件失败: {error}"))?;
        received += read as u64;
        if last_emit.elapsed() >= std::time::Duration::from_millis(120) {
            last_emit = std::time::Instant::now();
            on_progress(received, total);
        }
    }
    on_progress(received, total);
    if total != 0 && received != total {
        return Err(format!("下载不完整（{received}/{total}）"));
    }
    Ok((received, total))
}

/// 后台下载入口（线程里跑）。逐源尝试，成功即验收安装，全失败发 done{ok:false}。
fn download_worker(app: tauri::AppHandle) {
    let key = match platform_key() {
        Ok(key) => key,
        Err(error) => {
            finish_download(&app, Err(error));
            return;
        }
    };
    let primary = asset_url(key);
    let mut sources: Vec<(&str, String)> = vec![("GitHub 直连", primary)];
    for prefix in MIRROR_PREFIXES {
        sources.push(("镜像加速", format!("{prefix}{}", asset_url(key))));
    }
    let tmp = match binary_path(&app) {
        Ok(path) => path.with_extension("downloading"),
        Err(error) => {
            finish_download(&app, Err(error));
            return;
        }
    };
    let mut last_error = String::new();
    for (index, (label, url)) in sources.iter().enumerate() {
        if index > 0 {
            let _ = app.emit(
                EVENT_DL_PROGRESS,
                DownloadProgress {
                    stage: "mirror-retry".into(),
                    source: label.to_string(),
                    received: 0,
                    total: 0,
                    percent: 0,
                },
            );
            gw_log(&app, &format!("[gateway] 下载换源 {label}: {url}"));
        }
        let _ = std::fs::remove_file(&tmp);
        // 逐块进度（download_to_tmp_observed 内部已按 120ms 节流）
        let progress_app = app.clone();
        let progress_label = label.to_string();
        match download_to_tmp_observed(url, &tmp, move |received, total| {
            let percent = if total > 0 {
                ((received as f64 / total as f64) * 100.0).round() as u32
            } else {
                0
            };
            let _ = progress_app.emit(
                EVENT_DL_PROGRESS,
                DownloadProgress {
                    stage: "downloading".into(),
                    source: progress_label.clone(),
                    received,
                    total,
                    percent,
                },
            );
        }) {
            Ok((received, total)) => {
                gw_log(
                    &app,
                    &format!("[gateway] 下载完成 {label}: {received}/{total} bytes"),
                );
                match verify_and_install(&app, &tmp) {
                    Ok(_) => {
                        finish_download(&app, Ok(()));
                        return;
                    }
                    Err(error) => last_error = error,
                }
            }
            Err(error) => {
                let _ = std::fs::remove_file(&tmp);
                last_error = format!("{label}: {error}");
                gw_log(&app, &format!("[gateway] 下载失败 {last_error}"));
            }
        }
    }
    finish_download(&app, Err(format!(
        "所有下载源都失败（{last_error}）。可在远程接入页手动指定本地 cloudflared 路径兜底"
    )));
}

fn finish_download(app: &tauri::AppHandle, result: Result<(), String>) {
    DL_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
    let status = binary_status(app);
    let done = DownloadDone {
        ok: result.is_ok(),
        error: result.err().unwrap_or_default(),
        installed: status.installed,
        path: status.path,
        version: status.version,
    };
    let _ = app.emit(EVENT_DL_DONE, done);
}

// ==================== Tauri 命令 ====================

/// 查询二进制就绪状态。
#[tauri::command]
pub(crate) fn gateway_binary_status(app: tauri::AppHandle) -> BinaryStatus {
    binary_status(&app)
}

/// 发起后台下载（立即返回；进度/结果走事件）。已装好直接回状态。
#[tauri::command]
pub(crate) fn gateway_binary_download(
    app: tauri::AppHandle,
) -> Result<BinaryStatus, String> {
    let status = binary_status(&app);
    if status.installed {
        return Ok(status);
    }
    if DL_IN_FLIGHT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("下载已在进行中".to_string());
    }
    let worker_app = app.clone();
    std::thread::spawn(move || download_worker(worker_app));
    Ok(status)
}

/// 手动指定本地 cloudflared 路径：验证可执行后**复制**进缓存位（单一事实来源）。
/// 逃生口不做哈希强校验，但回读版本号；前端据此标注「手动路径」。
#[tauri::command]
pub(crate) fn gateway_binary_set_manual(
    app: tauri::AppHandle,
    path: String,
) -> Result<BinaryStatus, String> {
    let source = PathBuf::from(path.trim());
    let version = probe_manual_binary(&source)?;
    let final_path = binary_path(&app)?;
    if final_path != source {
        if final_path.exists() {
            let backup = final_path.with_extension("old");
            let _ = std::fs::remove_file(&backup);
            std::fs::rename(&final_path, &backup)
                .map_err(|error| format!("旧二进制被占用，无法替换: {error}"))?;
        }
        std::fs::copy(&source, &final_path)
            .map_err(|error| format!("复制进缓存目录失败: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&final_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("chmod +x 失败: {error}"))?;
        }
    }
    gw_log(
        &app,
        &format!("[gateway] 手动二进制就位: {} ({version})", final_path.display()),
    );
    let status = binary_status(&app);
    Ok(status)
}

/// 卸载 cloudflared：二进制恒在 AnyBoot 自家缓存位（下载 / 手动路径都复制进来，
/// 单一事实来源）——缓存位有货即「通过 AnyBoot 安装」，直接删缓存即可，
/// 不会碰用户手动指定路径的原始文件。通道在跑先停（Windows 上运行中的 exe 删不掉）。
///
/// `keep_data`：卸载后是否保留本机数据（命名隧道 Token / 对外地址 / 自启开关）。
/// 默认 **false = 一起清掉** —— 程序都删了，认证信息留着只会让下次重装继承一份
/// 用户以为已经没了的凭据（老版本就是这么留坑的）。
#[tauri::command]
pub(crate) async fn gateway_binary_uninstall(
    app: tauri::AppHandle,
    keep_data: Option<bool>,
) -> Result<BinaryStatus, String> {
    let keep_data = keep_data.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || binary_uninstall_blocking(&app, keep_data))
        .await
        .map_err(|error| format!("卸载任务失败: {error}"))?
}

fn binary_uninstall_blocking(
    app: &tauri::AppHandle,
    keep_data: bool,
) -> Result<BinaryStatus, String> {
    let status = binary_status(app);
    if !status.installed {
        return Err("cloudflared 未安装，无需卸载".to_string());
    }
    let path = binary_path(app)?;
    // 通道在跑先停：进程占着二进制，Windows 上删不掉
    let _ = gateway_tunnel_stop_blocking(app);
    let mut removed = Vec::new();
    for candidate in [path.clone(), path.with_extension("old")] {
        match std::fs::remove_file(&candidate) {
            Ok(()) => removed.push(candidate.display().to_string()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "删除 {} 失败（文件被占用？先关掉通道再试）: {error}",
                    candidate.display()
                ));
            }
        }
    }
    gw_log(
        app,
        &format!("[gateway] 已卸载: {}", removed.join("、")),
    );
    if keep_data {
        gw_log(app, "[gateway] 按「保留本机数据」卸载：命名隧道 Token / 对外地址 / 配置均未改动");
    } else {
        match clear_named_credentials(
            app,
            "gateway_binary_uninstall",
            "卸载 cloudflared：清理命名隧道凭据",
        ) {
            Ok(cleared) => {
                if cleared {
                    // 没二进制了，自启留着只会每次开机报一次失败
                    let _ = crate::settings_commands::update_settings_and_publish(
                        app,
                        "gateway_binary_uninstall",
                        "卸载 cloudflared：关闭自启",
                        |settings| settings.gateway_autostart = false,
                        |_| {},
                    );
                }
            }
            Err(error) => {
                return Err(format!("cloudflared 已删除，但清理登录凭据失败：{error}"));
            }
        }
    }
    Ok(binary_status(app))
}

/// 清掉命名隧道的凭据与对外地址（Token 就是 cloudflared 这一侧的登录凭据；
/// 地址跟着一起清，避免留下「有域名没 token」的半截配置把模式卡在 named）。
/// 返回是否真的清掉了东西（没有凭据时返回 Ok(false)，不写盘也不报错）。
pub(crate) fn clear_named_credentials(
    app: &tauri::AppHandle,
    action: &str,
    message: &str,
) -> Result<bool, String> {
    let settings = crate::stored_settings(app);
    let has = !settings.gateway_named_token.trim().is_empty()
        || !settings.gateway_named_url.trim().is_empty()
        || settings.gateway_mode == "named";
    if !has {
        return Ok(false);
    }
    crate::settings_commands::update_settings_and_publish(
        app,
        action,
        message,
        |settings| {
            settings.gateway_named_token.clear();
            settings.gateway_named_url.clear();
            settings.gateway_mode = "quick".to_string();
        },
        |_| {},
    )?;
    gw_log(
        app,
        "[gateway] 登录凭据已清除：命名隧道 Token / 对外地址已抹掉，模式回落「快速隧道」",
    );
    Ok(true)
}

/// 「清除登录凭据」（Cloudflare 侧，对应 Tailscale 的 logout）：停通道 + 抹掉
/// 命名隧道 Token 与对外地址。不动程序本体 —— 下次填新 Token 即可继续用。
#[tauri::command]
pub(crate) async fn gateway_clear_credentials(
    app: tauri::AppHandle,
) -> Result<GatewayConfig, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _ = gateway_tunnel_stop_blocking(&app);
        let cleared = clear_named_credentials(
            &app,
            "gateway_clear_credentials",
            "Cloudflare 通道：清除登录凭据",
        )?;
        if !cleared {
            return Err("没有保存过命名隧道的 Token，无需清除".to_string());
        }
        Ok(gateway_config(app))
    })
    .await
    .map_err(|error| format!("清除凭据任务失败: {error}"))?
}

// ==================== 通道生命周期 ====================
// 模式语义（设计稿 v4.2 / 任务 #30 MVP 边界）：
// - quick：全自动——spawn `cloudflared tunnel --url http://127.0.0.1:<port>`
//   `--no-autoupdate`，从 stderr 抠 trycloudflare.com 地址。重启后地址会变，
//   由过期判定/重扫机制消化。
// - named：用户已有 Tunnel Token，直接 `cloudflared tunnel run --token …`，
//   地址用用户存的固定域名（不自动建隧道）。
// - tailscale：不跑 cloudflared，指引用户装 tailscale 后把 ts.net 地址存进
//   电脑档案（后续期做 CLI 集成）。
// 期望状态 desired=true 时进程意外退出→退避重启（5/15/30/60/60s，最多 5 次），
// 每次都发「重启中 · 第 N 次重试」事件，不弹 toast（设计稿⑥补漏#6）。

/// 通道状态事件。
pub(crate) const EVENT_TUNNEL_STATE: &str = "gateway-tunnel-state";

/// 日志环形缓冲上限（设计稿⑥补漏#3：500 行）。
const LOG_CAP: usize = 500;

/// 退避重启序列（秒）。
const BACKOFF_SECS: [u64; 5] = [5, 15, 30, 60, 60];

/// 通道模式。
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TunnelMode {
    Quick,
    Named,
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct TunnelSnapshot {
    pub running: bool,
    pub mode: String,
    pub url: String,
    pub started_at: u64,
    pub restarts: u32,
    pub state: String, // starting|running|restarting|stopped|failed
    pub last_error: String,
}

/// Tauri managed state。
pub struct GatewayTunnelState {
    /// 进程持有；None=没在跑。try_wait/kill 共用一把锁（各只锁一瞬）。
    proc: Mutex<Option<TunnelProc>>,
    /// 用户期望：true=要求开着（允许退避重启）；false=用户关了。
    desired: std::sync::atomic::AtomicBool,
    /// 日志环形缓冲（最新在尾）。
    log: Mutex<std::collections::VecDeque<String>>,
    /// 被上限挤掉的历史行数（日志窗口首行明示，避免「是不是少了日志」的怀疑）。
    log_dropped: std::sync::atomic::AtomicUsize,
    /// 展示快照（事件冗余一份，status 命令随时可查）。
    snapshot: Mutex<TunnelSnapshot>,
    /// 沿用的残留通道（没有 Child 句柄，只记 pid；stop/kill_all 按 pid 收掉）。
    adopted: Mutex<Option<AdoptedTunnel>>,
}

impl Default for GatewayTunnelState {
    fn default() -> Self {
        Self {
            proc: Mutex::new(None),
            desired: std::sync::atomic::AtomicBool::new(false),
            log: Mutex::new(std::collections::VecDeque::new()),
            log_dropped: std::sync::atomic::AtomicUsize::new(0),
            snapshot: Mutex::new(TunnelSnapshot {
                running: false,
                mode: "quick".into(),
                url: String::new(),
                started_at: 0,
                restarts: 0,
                state: "stopped".into(),
                last_error: String::new(),
            }),
            adopted: Mutex::new(None),
        }
    }
}

struct TunnelProc {
    child: Mutex<std::process::Child>,
    #[allow(dead_code)]
    pid: u32,
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl GatewayTunnelState {
    fn push_log(&self, line: &str) {
        if let Ok(mut log) = self.log.lock() {
            if log.len() >= LOG_CAP {
                log.pop_front();
                self.log_dropped
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            log.push_back(line.to_string());
        }
    }

    fn set_snapshot(&self, patch: impl FnOnce(&mut TunnelSnapshot)) {
        if let Ok(mut snapshot) = self.snapshot.lock() {
            patch(&mut snapshot);
        }
    }

    fn emit_state(&self, app: &tauri::AppHandle) {
        if let Ok(snapshot) = self.snapshot.lock() {
            let _ = app.emit(EVENT_TUNNEL_STATE, snapshot.clone());
        }
    }
}

/// 回源地址：HTTP 端口与 apply_http_config 同源（环境变量可覆盖）。
fn origin_url(app: &tauri::AppHandle) -> String {
    let port = std::env::var("ANYBOOT_HTTP_PORT")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or_else(|| crate::stored_settings(app).http_port);
    format!("http://127.0.0.1:{port}")
}

/// 从一行日志里抠 quick 隧道地址（形如 `|  https://xxx.trycloudflare.com |`）。
fn parse_quick_url(line: &str) -> Option<String> {
    let marker = line.find("https://")?;
    let rest = &line[marker..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|' || c == ',')
        .unwrap_or(rest.len());
    let url = &rest[..end];
    url.contains("trycloudflare.com").then(|| url.to_string())
}

/// 启动通道（quick/named）。已在跑则报错；手动路径/下载兜底由前端先调 #29 命令。
/// async + spawn_blocking（对齐 hub_tunnel_start）：起子进程不许留在 UI 线程。
#[tauri::command]
pub(crate) async fn gateway_tunnel_start(
    app: tauri::AppHandle,
    mode: String,
    named_token: String,
    named_url: String,
) -> Result<TunnelSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        tunnel_start_blocking(&app, mode, named_token, named_url)
    })
    .await
    .map_err(|error| format!("启动通道任务失败: {error}"))?
}

/// 清理孤儿 cloudflared：可执行路径精确等于我们 gateway 目录下那个的都算。
/// 正常退出有 kill_all 钩子；但强杀、崩溃时钩子不执行 → 旧隧道进程活着，
/// 旧地址照样能连（快速隧道每次重启本该换地址，客户端会「莫名其妙还能连旧的」）。
/// 在 spawn 自己的 cloudflared 之前调用；此时 state.proc 必为 None，同路径的全是孤儿。
fn cleanup_orphan_cloudflared(app: &tauri::AppHandle) {
    #[cfg(windows)]
    {
        let Ok(binary) = binary_path(app) else { return };
        let Some(exe) = binary.to_str() else { return };
        // PowerShell 字符串单引号内翻倍转义；-eq 不区分大小写，正合适
        let script = format!(
            "Get-CimInstance Win32_Process -Filter \"Name='cloudflared.exe'\" | \
             Where-Object {{ $_.ExecutablePath -eq '{}' }} | \
             ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}",
            exe.replace('\'', "''"),
        );
        let mut command = std::process::Command::new("powershell");
        command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        match command.output() {
            Ok(out) => {
                let killed = String::from_utf8_lossy(&out.stdout);
                let killed = killed.trim();
                if !killed.is_empty() {
                    gw_log(app, &format!("[gateway] 清理孤儿 cloudflared: {killed}"));
                }
            }
            Err(error) => gw_log(app, &format!("[gateway] 孤儿清理失败: {error}")),
        }
    }
    #[cfg(not(windows))]
    {
        // macOS/Linux 没有 PowerShell（首版硬编码导致 trace 刷「孤儿清理失败」）：
        // 直接复用 ps 实现的进程列表逐个 kill
        let orphans = list_cloudflared_pids(app, &[]);
        let pids: Vec<String> = orphans.iter().map(|(pid, _)| pid.to_string()).collect();
        for (pid, _) in orphans {
            kill_cloudflared_pid(pid);
        }
        if !pids.is_empty() {
            gw_log(app, &format!("[gateway] 清理孤儿 cloudflared: {}", pids.join(",")));
        }
    }
}

// ==================== 残留通道（异常退出留下的孤儿 cloudflared） ====================
// 语义：强杀/崩溃时 kill_all 钩子不执行 → cloudflared 成孤儿继续跑。
// 不再一刀切杀掉，而是列出来让用户选「沿用（地址不变）/ 终止（重建新地址）」；
// 勾了自启且残留通道有地址记录时，自动沿用 —— 快速隧道地址得以跨软件重启保持。

/// 最近一次隧道进程的 pid→url 映射（沿用残留通道时对回地址）。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct TunnelLast {
    pid: u32,
    #[serde(default)]
    url: String,
}

fn tunnel_last_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(cache_dir(app)?.join("tunnel_last.json"))
}

fn save_tunnel_last(app: &AppHandle, pid: u32, url: &str) {
    if let Ok(path) = tunnel_last_path(app) {
        if let Ok(bytes) = serde_json::to_vec(&TunnelLast { pid, url: url.to_string() }) {
            let _ = std::fs::write(path, bytes);
        }
    }
}

fn load_tunnel_last(app: &AppHandle) -> TunnelLast {
    tunnel_last_path(app)
        .ok()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 列出可执行路径精确等于我们 cloudflared 的进程（pid + 启动时间）。
/// exclude 里的 pid 不返回（受管/已沿用的不算残留）。
#[cfg(windows)]
fn list_cloudflared_pids(app: &AppHandle, exclude: &[u32]) -> Vec<(u32, String)> {
    let Ok(binary) = binary_path(app) else { return Vec::new() };
    let Some(exe) = binary.to_str() else { return Vec::new() };
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='cloudflared.exe'\" | \
         Where-Object {{ $_.ExecutablePath -eq '{}' }} | \
         ForEach-Object {{ \"{{0}}|{{1}}\" -f $_.ProcessId, $_.CreationDate.ToString('yyyy-MM-dd HH:mm:ss') }}",
        exe.replace('\'', "''"),
    );
    let mut command = std::process::Command::new("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let Ok(output) = command.output() else { return Vec::new() };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, time) = line.trim().split_once('|')?;
            let pid: u32 = pid.parse().ok()?;
            (!exclude.contains(&pid)).then(|| (pid, time.to_string()))
        })
        .collect()
}

/// unix 版：`ps -axo pid=,lstart=,command=` 后在 Rust 侧精确匹配命令行。
/// 不用 pgrep -f（路径进正则要转义、语义还是子串），逐行 splitn 最可靠：
/// pid 1 段 + lstart 固定 5 段（周 月 日 时:分:秒 年）+ 命令行剩余全部。
#[cfg(not(windows))]
fn list_cloudflared_pids(app: &AppHandle, exclude: &[u32]) -> Vec<(u32, String)> {
    let Ok(binary) = binary_path(app) else { return Vec::new() };
    let Some(exe) = binary.to_str() else { return Vec::new() };
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "pid=,lstart=,command="])
        .output()
    else {
        return Vec::new();
    };
    let with_arg_prefix = format!("{exe} ");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.trim().splitn(7, ' ').collect();
            if fields.len() != 7 {
                return None;
            }
            let pid: u32 = fields[0].parse().ok()?;
            // argv[0] 必须精确是我们 spawn 的路径（我们 spawn 不经 shell，无包裹）
            let command = fields[6];
            if command != exe && !command.starts_with(&with_arg_prefix) {
                return None;
            }
            let started_at = fields[1..6].join(" ");
            (!exclude.contains(&pid)).then(|| (pid, started_at))
        })
        .collect()
}

/// 强杀指定 cloudflared 进程（对齐 Windows taskkill /F 语义）。
#[cfg(windows)]
fn kill_cloudflared_pid(pid: u32) {
    let script = format!("Stop-Process -Id {pid} -Force -ErrorAction SilentlyContinue");
    let mut command = std::process::Command::new("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let _ = command.output();
}

#[cfg(not(windows))]
fn kill_cloudflared_pid(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output();
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct OrphanTunnel {
    pub pid: u32,
    /// 进程启动时间（本地时区字符串，直接来自 CIM）。
    pub started_at: String,
    /// 沿用时可对回的隧道地址（空=没有记录，地址未知）。
    pub url: String,
}

fn tunnel_orphans_inner(app: &AppHandle) -> Vec<OrphanTunnel> {
    let state = app.state::<GatewayTunnelState>();
    let mut exclude = Vec::new();
    if let Some(p) = state.proc.lock().unwrap().as_ref() {
        exclude.push(p.pid);
    }
    if let Some(a) = state.adopted.lock().unwrap().as_ref() {
        exclude.push(a.pid);
    }
    let last = load_tunnel_last(app);
    list_cloudflared_pids(app, &exclude)
        .into_iter()
        .map(|(pid, started_at)| OrphanTunnel {
            pid,
            started_at,
            url: if last.pid == pid { last.url.clone() } else { String::new() },
        })
        .collect()
}

/// 残留通道列表（网关页「本机通道」卡里展示）。CIM 查询是起子进程 —— 必须 spawn_blocking。
#[tauri::command]
pub(crate) async fn gateway_tunnel_orphans(app: tauri::AppHandle) -> Vec<OrphanTunnel> {
    tauri::async_runtime::spawn_blocking(move || tunnel_orphans_inner(&app))
        .await
        .unwrap_or_default()
}

/// 已沿用的残留通道（没有 Child 句柄，只记 pid；stop/kill_all 时按 pid 收掉）。
struct AdoptedTunnel {
    pid: u32,
    url: String,
}

fn tunnel_adopt_inner(app: &AppHandle, pid: u32) -> Result<TunnelSnapshot, String> {
    let state = app.state::<GatewayTunnelState>();
    if state.proc.lock().unwrap().is_some() {
        return Err("本应用的通道已在运行，无需沿用".to_string());
    }
    if !list_cloudflared_pids(app, &[]).iter().any(|(p, _)| *p == pid) {
        return Err(format!("进程 {pid} 已经不在了（可能刚退出）"));
    }
    let last = load_tunnel_last(app);
    let url = if last.pid == pid { last.url.clone() } else { String::new() };
    *state.adopted.lock().unwrap() = Some(AdoptedTunnel { pid, url: url.clone() });
    *state.snapshot.lock().unwrap() = TunnelSnapshot {
        running: true,
        mode: "quick".into(),
        url: url.clone(),
        started_at: now_millis(),
        restarts: 0,
        state: "running".into(),
        last_error: String::new(),
    };
    state.emit_state(app);
    gw_log(app, &format!("[gateway] 沿用残留通道 pid={pid} url={url}"));
    state
        .snapshot
        .lock()
        .map(|s| s.clone())
        .map_err(|_| "状态锁中毒".to_string())
}

/// 沿用残留通道：不新建进程，把已在跑的孤儿记到本应用名下（地址从 pid 映射找回）。
#[tauri::command]
pub(crate) async fn gateway_tunnel_adopt(
    app: tauri::AppHandle,
    pid: u32,
) -> Result<TunnelSnapshot, String> {
    match tauri::async_runtime::spawn_blocking(move || tunnel_adopt_inner(&app, pid)).await {
        Ok(result) => result,
        Err(error) => Err(error.to_string()),
    }
}

/// 终止一个残留通道进程，返回最新残留列表（省一次来回）。
#[tauri::command]
pub(crate) async fn gateway_tunnel_kill_orphan(app: tauri::AppHandle, pid: u32) -> Vec<OrphanTunnel> {
    tauri::async_runtime::spawn_blocking(move || {
        if list_cloudflared_pids(&app, &[]).iter().any(|(p, _)| *p == pid) {
            kill_cloudflared_pid(pid);
            gw_log(&app, &format!("[gateway] 终止残留通道 pid={pid}"));
        }
        tunnel_orphans_inner(&app)
    })
    .await
    .unwrap_or_default()
}

fn tunnel_start_blocking(
    app: &tauri::AppHandle,
    mode: String,
    named_token: String,
    named_url: String,
) -> Result<TunnelSnapshot, String> {
    let state = app.state::<GatewayTunnelState>();
    if state.proc.lock().unwrap().is_some() {
        return Err("通道已在运行".to_string());
    }
    // 点了「开启通道」= 明确要新的：沿用中的残留也按孤儿清掉
    *state.adopted.lock().unwrap() = None;
    cleanup_orphan_cloudflared(app);
    let mode = match mode.as_str() {
        "quick" => TunnelMode::Quick,
        "named" => TunnelMode::Named,
        other => return Err(format!("未知通道模式: {other}")),
    };
    let binary = binary_path(app)?;
    if !binary.is_file() {
        return Err("cloudflared 未就位：先下载或手动指定路径".to_string());
    }
    if mode == TunnelMode::Named && named_token.trim().is_empty() {
        return Err("命名隧道需要 Tunnel Token".to_string());
    }
    state
        .desired
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let named_url = named_url.trim().to_string();
    *state.snapshot.lock().unwrap() = TunnelSnapshot {
        running: false,
        mode: mode_label(&mode).to_string(),
        url: named_url.clone(),
        started_at: now_millis(),
        restarts: 0,
        state: "starting".into(),
        last_error: String::new(),
    };
    state.emit_state(app);
    gw_log(app, &format!("[gateway] 启动通道 mode={:?}", mode));
    let worker_app = app.clone();
    std::thread::spawn(move || tunnel_worker(worker_app, mode, named_token, named_url));
    state.snapshot.lock().map(|s| s.clone()).map_err(|_| "状态锁中毒".to_string())
}

fn mode_label(mode: &TunnelMode) -> &'static str {
    match mode {
        TunnelMode::Quick => "quick",
        TunnelMode::Named => "named",
    }
}

/// 工作线程：spawn → 读 stderr → 意外退出按退避重启，直到 desired=false 或次数用完。
fn tunnel_worker(app: tauri::AppHandle, mode: TunnelMode, named_token: String, named_url: String) {
    let state = app.state::<GatewayTunnelState>();
    let binary = binary_path(&app).expect("启动前已验证二进制存在");
    let origin = origin_url(&app);
    let mut restarts: u32 = 0;

    loop {
        let mut command = std::process::Command::new(&binary);
        command.arg("--no-autoupdate");
        match mode {
            TunnelMode::Quick => {
                command.args(["tunnel", "--url", &origin]);
            }
            TunnelMode::Named => {
                command.args(["tunnel", "run", "--token", named_token.trim()]);
            }
        }
        command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                state.push_log(&format!("[启动失败] {error}"));
                state.set_snapshot(|s| {
                    s.running = false;
                    s.state = "failed".into();
                    s.last_error = format!("无法启动 cloudflared: {error}");
                });
                state.emit_state(&app);
                break;
            }
        };
        let pid = child.id();
        gw_log(&app, &format!("[gateway] cloudflared pid={pid}"));
        save_tunnel_last(&app, pid, "");
        *state.proc.lock().unwrap() = Some(TunnelProc {
            child: Mutex::new(child),
            pid,
        });
        state.set_snapshot(|s| {
            s.running = true;
            s.state = "starting".into();
            s.started_at = now_millis();
            s.last_error.clear();
        });
        state.emit_state(&app);

        // stderr 读取：进环形日志 + 抠 quick URL + 识别 named「已注册」。
        let stderr = state
            .proc
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|p| p.child.lock().unwrap().stderr.take());
        if let Some(stderr) = stderr {
            let reader_app = app.clone();
            let reader_mode = mode.clone();
            let reader_url = named_url.clone();
            std::thread::spawn(move || {
                pipe_log_lines(reader_app, stderr, reader_mode, reader_url, pid);
            });
        }

        // 等退出。先算 try_wait 结果（内层锁随即释放），再动外层槽位，避免嵌套可变借用。
        let exit_info = loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let mut guard = state.proc.lock().unwrap();
            let finished = match guard.as_ref() {
                Some(p) => match p.child.lock().unwrap().try_wait() {
                    Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
                    Ok(None) => None,
                    Err(_) => Some(-1),
                },
                None => Some(-1),
            };
            if let Some(code) = finished {
                *guard = None;
                break Some(code);
            }
        };
        let desired = state.desired.load(std::sync::atomic::Ordering::SeqCst);
        if !desired {
            state.set_snapshot(|s| {
                s.running = false;
                s.state = "stopped".into();
            });
            state.emit_state(&app);
            gw_log(&app, "[gateway] 通道已停止（用户关闭）");
            break;
        }
        if restarts as usize >= BACKOFF_SECS.len() {
            state.set_snapshot(|s| {
                s.running = false;
                s.state = "failed".into();
                s.last_error = format!("重试 {restarts} 次后仍退出（exit={exit_info:?}）");
            });
            state.emit_state(&app);
            gw_log(&app, "[gateway] 退避重启次数用完，标记失败");
            break;
        }
        let wait = BACKOFF_SECS[restarts as usize];
        restarts += 1;
        state.set_snapshot(|s| {
            s.running = false;
            s.state = "restarting".into();
            s.restarts = restarts;
            s.last_error = format!("进程退出（exit={exit_info:?}），{wait} 秒后第 {restarts} 次重试");
        });
        state.emit_state(&app);
        state.push_log(&format!("[重启] {wait} 秒后第 {restarts} 次重试"));
        std::thread::sleep(std::time::Duration::from_secs(wait));
    }
}

/// stderr 按行读 → 环形日志 + 事件。pid 用于把地址写进 tunnel_last.json（沿用残留时对账）。
fn pipe_log_lines(
    app: tauri::AppHandle,
    mut stderr: std::process::ChildStderr,
    mode: TunnelMode,
    named_url: String,
    pid: u32,
) {
    let state = app.state::<GatewayTunnelState>();
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stderr.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=pos).collect();
                    let text = String::from_utf8_lossy(&line).trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    state.push_log(&text);
                    if mode == TunnelMode::Quick {
                        if let Some(url) = parse_quick_url(&text) {
                            state.set_snapshot(|s| {
                                s.url = url.clone();
                                s.state = "running".into();
                            });
                            state.emit_state(&app);
                            save_tunnel_last(&app, pid, &url);
                            gw_log(&app, &format!("[gateway] 隧道地址 {url}"));
                        }
                    } else if text.contains("Registered tunnel connection") {
                        state.set_snapshot(|s| {
                            s.url = named_url.clone();
                            s.state = "running".into();
                        });
                        state.emit_state(&app);
                        save_tunnel_last(&app, pid, &named_url);
                    }
                }
            }
        }
    }
}

/// 停止通道（用户关闭：不重启）。杀残留要起 PowerShell 子进程 —— 必须 spawn_blocking。
#[tauri::command]
pub(crate) async fn gateway_tunnel_stop(app: tauri::AppHandle) -> Result<TunnelSnapshot, String> {
    match tauri::async_runtime::spawn_blocking(move || gateway_tunnel_stop_blocking(&app)).await {
        Ok(result) => result,
        Err(error) => Err(error.to_string()),
    }
}

fn gateway_tunnel_stop_blocking(app: &AppHandle) -> Result<TunnelSnapshot, String> {
    let state = app.state::<GatewayTunnelState>();
    state
        .desired
        .store(false, std::sync::atomic::Ordering::SeqCst);
    if let Some(proc_handle) = state.proc.lock().unwrap().as_ref() {
        let _ = proc_handle.child.lock().unwrap().kill();
    }
    // 沿用的残留进程没有 Child 句柄，按 pid 收掉
    if let Some(a) = state.adopted.lock().unwrap().take() {
        kill_cloudflared_pid(a.pid);
    }
    gw_log(app, "[gateway] 用户请求停止通道");
    // 进程退出由工作线程感知并落 stopped 快照；这里也直接兜底置态，避免 UI 干等。
    state.set_snapshot(|s| {
        s.running = false;
        s.state = "stopped".into();
    });
    state.emit_state(app);
    state
        .snapshot
        .lock()
        .map(|s| s.clone())
        .map_err(|_| "状态锁中毒".to_string())
}

/// 通道状态查询。
#[tauri::command]
pub(crate) fn gateway_tunnel_status(
    state: tauri::State<'_, GatewayTunnelState>,
) -> TunnelSnapshot {
    state
        .snapshot
        .lock()
        .map(|s| s.clone())
        .unwrap_or(TunnelSnapshot {
            running: false,
            mode: "quick".into(),
            url: String::new(),
            started_at: 0,
            restarts: 0,
            state: "stopped".into(),
            last_error: String::new(),
        })
}

/// 日志（最新在尾，最多 500 行）。
#[tauri::command]
pub(crate) fn gateway_tunnel_logs(state: tauri::State<'_, GatewayTunnelState>) -> Vec<String> {
    let mut lines: Vec<String> = state
        .log
        .lock()
        .map(|log| log.iter().cloned().collect())
        .unwrap_or_default();
    // 超出上限时明说丢了多少行 —— 否则用户会怀疑日志被过滤/没打全
    let dropped = state.log_dropped.load(std::sync::atomic::Ordering::Relaxed);
    if dropped > 0 {
        lines.insert(
            0,
            format!("[日志] 已丢弃最早 {dropped} 行（内存上限 {LOG_CAP} 行，只保留最近输出）"),
        );
    }
    lines
}

/// 清空通道日志（UI「清空」按钮：源头清掉 + 屏幕清屏，下次拉取不再恢复）。
#[tauri::command]
pub(crate) fn gateway_tunnel_logs_clear(state: tauri::State<'_, GatewayTunnelState>) {
    if let Ok(mut log) = state.log.lock() {
        log.clear();
        state
            .log_dropped
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// 应用退出时收掉通道进程（对齐 hub_tunnel::kill_all，在 lib.rs 退出钩子注册）。
pub(crate) fn kill_all(app: &AppHandle) {
    if let Some(state) = app.try_state::<GatewayTunnelState>() {
        state
            .desired
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(proc_handle) = state.proc.lock().unwrap().as_ref() {
            let _ = proc_handle.child.lock().unwrap().kill();
        }
        // 沿用的残留进程一并收掉
        if let Some(a) = state.adopted.lock().unwrap().take() {
            kill_cloudflared_pid(a.pid);
        }
    }
}

// ==================== 配对令牌（每客户端签发/吊销） ====================
// 语义（设计稿 v4.2 / 任务 #31）：
// - 每个接入端（另一台 PC 的 hub、一部手机）单独签发 32 位十六进制令牌；
//   落盘只存 SHA-256，不存明文（明文只在签发瞬间出现在配对串里）。
// - 令牌是 http_token 的**等价凭据**：挂进既有 token 取值链
//   （X-Auth-Token → ?token= → ?ab_token= 别名 → cookie → body，见 http.rs），
//   跨源 iframe 场景走 query（不能带自定义 Header）。
// - 吊销 = revoked=true：HTTP 侧下一次请求即拒；已建立的 WebSocket 一律
//   踢线（全部重连，只有凭据有效的能连上）。
// - last_seen：鉴权成功时更新（配对管理「最近访问」的数据源）。

/// 客户端档案文件名。
const CLIENTS_FILE: &str = "gateway-clients.json";

/// 配对令牌签发结果（明文 token 只在这里出现一次）。
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct PairIssued {
    pub client: GatewayClient,
    /// 明文令牌（签发时唯一一次返回）。
    pub token: String,
    /// anyboot://pair?... 配对串（供二维码/复制）。
    pub pair: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct GatewayClient {
    pub id: String,
    pub name: String,
    /// SHA-256(明文令牌)。
    pub token_sha256: String,
    pub created_at: u64,
    /// 最近一次鉴权成功时间（0=从未用过）。
    pub last_seen: u64,
    pub revoked: bool,
    /// 出码时的隧道地址（快速隧道每次重启会变，留着对账：客户端现在连的是不是这个）。
    #[serde(default)]
    pub pair_addr: String,
    /// 出码时的通道类型：`tunnel` / `ts` / `funnel` / `tsnet`（空 = 老记录，按地址判型）。
    /// 三套 Tailscale 的地址长得一样，只看域名分不出「系统 Funnel」还是「内置 Funnel」，
    /// 列表上的来源 chip 就靠这个字段，别再混合成一个「Tailscale」。
    #[serde(default)]
    pub pair_kind: String,
}

fn clients_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(cache_dir(app)?.join(CLIENTS_FILE))
}

fn load_clients(app: &tauri::AppHandle) -> Vec<GatewayClient> {
    let path = match clients_path(app) {
        Ok(path) => path,
        Err(_) => return Vec::new(),
    };
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn save_clients(app: &tauri::AppHandle, clients: &[GatewayClient]) -> Result<(), String> {
    let path = clients_path(app)?;
    let bytes = serde_json::to_vec_pretty(clients).map_err(|error| error.to_string())?;
    std::fs::write(&path, bytes).map_err(|error| format!("写 clients.json 失败: {error}"))
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// 网关客户端令牌校验（挂进 handle_remote_request / websocket_hello 的闸门）。
/// 命中且未吊销 → 更新 last_seen 并放行。空 token 直接 false。
pub(crate) fn gateway_token_valid(app: &tauri::AppHandle, provided: &str) -> bool {
    if provided.trim().is_empty() {
        return false;
    }
    let hash = sha256_hex(provided.trim());
    let mut clients = load_clients(app);
    let Some(client) = clients
        .iter_mut()
        .find(|c| c.token_sha256 == hash && !c.revoked)
    else {
        return false;
    };
    let now = now_millis();
    if now - client.last_seen > 60_000 {
        // 节流写盘：一分钟内重复访问不反复落盘
        client.last_seen = now;
        let _ = save_clients(app, &clients);
    }
    true
}

/// 当前隧道地址（配对串用）：quick=运行中的 trycloudflare URL；named=快照里
/// 用户存的固定域名；都没跑就空串（配对串仍可签发，地址字段由前端提示补扫）。
fn current_tunnel_url(app: &tauri::AppHandle) -> String {
    if let Some(state) = app.try_state::<GatewayTunnelState>() {
        if let Ok(snapshot) = state.snapshot.lock() {
            if snapshot.running && !snapshot.url.is_empty() {
                return snapshot.url.clone();
            }
        }
    }
    String::new()
}

/// 配对串 v2（客户端只认 `v=2`，老串一律拒收）。
///
/// 为什么必须带来源：客户端的幂等键是 `(mid, kind)`，还要在列表里显示「来自哪台机器、
/// 哪个系统」。老串只有一个 `gateway=主机名`，同名机器分不开、重复扫码也去不了重。
///
/// 参数含义见 `docs/remote-pairs.md` §4.5：
/// - `mid` = 本机 AnyBoot 配置的身份（`settings.json` 里的 machine_id）
/// - `os` = windows / macos / linux（列表徽标与来源显示）
/// - `host` = 主机名（纯显示）
/// - `type` = cloudflare / ts / tsnet（**区块**，不是通道 id）
/// - `tunnel_id` = named tunnel 的 id（识别同一隧道被多套系统共用）；拿不到就空
pub(crate) fn build_pair_string(
    mid: &str,
    os: &str,
    host: &str,
    kind: &str,
    tunnel: &str,
    token: &str,
    tunnel_id: &str,
) -> String {
    // `type` 必须是**区块** id（cloudflare / ts / tsnet），不是通道 id。
    // 调用方传的是通道（tunnel / funnel / ts / tsnet）——`funnel` 与 `ts` 同属 ts 区块，
    // 直接写进去客户端会当成非法值拒收。映射不上（空 / 未知）就留空，由客户端按地址判型。
    let block = crate::remote_servers::block_of_route(kind).unwrap_or("");
    format!(
        "anyboot://pair?v=2&mid={}&os={}&host={}&gateway={}&type={}&tunnel={}&token={}&tunnel_id={}",
        percent_encode(mid.trim()),
        percent_encode(os.trim()),
        percent_encode(host.trim()),
        percent_encode(host.trim()),
        percent_encode(block),
        percent_encode(tunnel.trim()),
        token.trim(),
        percent_encode(tunnel_id.trim())
    )
}

/// named tunnel 的 id（配对串 `tunnel_id=` 用）。
///
/// cloudflared 的 tunnel token 是 `base64(JSON{a,t,s})`，`t` 就是 tunnel id；
/// quick 模式每次域名都不同、没有稳定 id，返回空。解不出来也返回空 —— 这个字段只用于
/// 「同一隧道被多套系统共用」的提示，缺了不影响连接。
fn current_tunnel_id(app: &tauri::AppHandle) -> String {
    // 读盘而不是读内存态：named token 是用户存下来的固定凭据，出码时磁盘才是真相源
    // （与 `remote_list` 等命令一致，都走 `stored_settings`）。
    let settings = crate::stored_settings(app);
    if settings.gateway_mode != "named" {
        return String::new();
    }
    decode_tunnel_id(&settings.gateway_named_token)
}

/// 最小 base64 解码（标准/URL-safe、带不带填充都试）。
///
/// cloudflared 的 tunnel token 形态不保证恒定：官方 `tunnel token` 输出标准 base64，
/// 但用户从别的面板/脚本来回拷可能变成 URL-safe 或去掉 `=` 填充。三种都试一遍，
/// 全失败就返回 None —— 这个字段只用于「同一隧道被多套系统共用」的提示。
fn decode_base64(raw: &str) -> Option<Vec<u8>> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    use base64::Engine as _;
    let text = raw.trim_end_matches('=');
    // 四种引擎都是同一个具体类型（GeneralPurpose），直接数组循环 —— `Engine`
    // 带泛型方法，不能做 trait object。
    for engine in [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD] {
        if let Ok(bytes) = engine.decode(text) {
            return Some(bytes);
        }
    }
    None
}

/// 从 cloudflared tunnel token 里挖出 tunnel id（`{"a":…,"t":"<id>","s":…}`）。
/// 只做最小的 base64 + 字符串扫描，不引 JSON 解析依赖（token 里只有三个字符串字段，
/// 且这里允许失败：拿不到就不显示「共用隧道」提示）。
fn decode_tunnel_id(token: &str) -> String {
    let raw = token.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Some(payload) = decode_base64(raw) else {
        return String::new();
    };
    let Ok(text) = String::from_utf8(payload) else {
        return String::new();
    };
    let bytes = text.as_bytes();
    let mut index = 0usize;
    // 逐个找 `"t"` 再校验「后面是冒号 + 引号」——值里恰好出现 "t" 时不会误取。
    while let Some(offset) = text[index..].find("\"t\"") {
        let mut cursor = index + offset + 3;
        while bytes.get(cursor).is_some_and(|b| b.is_ascii_whitespace()) {
            cursor += 1;
        }
        if bytes.get(cursor) == Some(&b':') {
            cursor += 1;
            while bytes.get(cursor).is_some_and(|b| b.is_ascii_whitespace()) {
                cursor += 1;
            }
            if bytes.get(cursor) == Some(&b'"') {
                let start = cursor + 1;
                if let Some(end) = text[start..].find('"') {
                    return text[start..start + end].trim().to_string();
                }
            }
        }
        index += offset + 3;
        if index >= text.len() {
            break;
        }
    }
    String::new()
}

/// 签发配对：新客户端令牌 + anyboot://pair 配对串。
///
/// `kind` 是这条地址属于哪个通道（`tunnel` / `ts` / `funnel` / `tsnet`），会写进配对串的
/// `type=` 参数。**必须带**：手机端四套远程槽（Cloudflare / Tailscale 直连 / 系统 Funnel /
/// 内置 tsnet）以前只能靠地址长相猜类型，三套 *.ts.net 长得一模一样 → 配一个顶掉另一个，
/// 用户的配对码「凭空消失」。现在由签发端声明，手机端照单落槽。
#[tauri::command]
pub(crate) fn gateway_pair_issue(
    app: tauri::AppHandle,
    name: String,
    tunnel: String,
    kind: Option<String>,
) -> Result<PairIssued, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("客户端名称不能为空".to_string());
    }
    let token = crate::http_server::generate_token();
    // 地址优先用调用方给的（前端可能刚生成还没跑），否则取当前运行中的。
    let tunnel = {
        let given = tunnel.trim().to_string();
        if given.is_empty() {
            current_tunnel_url(&app)
        } else {
            given
        }
    };
    // 通道类型归一：只认四个合法值，别的一律空
    let kind = match kind.as_deref().map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value == "tunnel" || value == "ts" || value == "funnel" || value == "tsnet" => {
            value
        }
        Some(value) if value == "tailscale" => "ts".to_string(),
        Some(value) if value == "builtin" || value == "builtin_ts" => "tsnet".to_string(),
        Some(value) if value == "cloudflare" || value == "cf" => "tunnel".to_string(),
        _ => String::new(),
    };
    // 判不出类型就按地址形态定：100.64/10 与 `*.ts.net` 都属 ts 区块，其余算 Cloudflare。
    // **不猜 tsnet** —— 内建 tsnet 与系统 Funnel 的域名同形，猜错就会把两条路混成一条。
    // 配对码的 `type=` 必须落在这三个区块里，否则客户端按「非法值」拒收（§4.5）。
    let kind = if kind.is_empty() {
        if crate::remote_servers::is_tailscale_origin(&tunnel) {
            "ts".to_string()
        } else {
            "tunnel".to_string()
        }
    } else {
        kind
    };
    let client = GatewayClient {
        id: format!("gw-{}", &token[..8]),
        name,
        token_sha256: sha256_hex(&token),
        created_at: now_millis(),
        last_seen: 0,
        revoked: false,
        pair_addr: tunnel.clone(),
        pair_kind: kind.clone(),
    };
    let mut clients = load_clients(&app);
    clients.push(client.clone());
    save_clients(&app, &clients)?;
    let settings = crate::stored_settings(&app);
    let pair = build_pair_string(
        &settings.machine_id,
        crate::sysinfo_lite::os_family(),
        &crate::http_server::machine_name(),
        &kind,
        &tunnel,
        &token,
        &current_tunnel_id(&app),
    );
    gw_log(&app, &format!("[gateway] 签发配对 {} ({})", client.id, client.name));
    Ok(PairIssued {
        client,
        token,
        pair,
    })
}

/// 已配对客户端列表（含已吊销，前端展示分层）。
#[tauri::command]
pub(crate) fn gateway_pair_list(app: tauri::AppHandle) -> Vec<GatewayClient> {
    load_clients(&app)
}

/// 吊销：立即生效（HTTP 下次请求拒、WebSocket 全踢重连）。
#[tauri::command]
pub(crate) fn gateway_pair_revoke(
    app: tauri::AppHandle,
    id: String,
) -> Result<Vec<GatewayClient>, String> {
    let mut clients = load_clients(&app);
    let Some(client) = clients.iter_mut().find(|c| c.id == id) else {
        return Err(format!("客户端不存在: {id}"));
    };
    if !client.revoked {
        client.revoked = true;
        let label = format!("{} ({})", client.id, client.name);
        save_clients(&app, &clients)?;
        crate::websocket_disconnect_all();
        gw_log(&app, &format!("[gateway] 吊销 {label}"));
    }
    Ok(clients)
}

/// 删除已吊销的客户端记录（列表清理用）。
#[tauri::command]
pub(crate) fn gateway_pair_delete(
    app: tauri::AppHandle,
    id: String,
) -> Result<Vec<GatewayClient>, String> {
    let mut clients = load_clients(&app);
    let before = clients.len();
    clients.retain(|c| !(c.id == id && c.revoked));
    if clients.len() == before {
        return Err(format!("不存在可删除的已吊销记录: {id}"));
    }
    save_clients(&app, &clients)?;
    Ok(clients)
}

/// 最小 percent-encode（配对串里的名称/地址防注入 & 空格）。
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ==================== 在线拉取（PC↔PC 配对握手，#32） ====================
// 链路（设计稿③ / 补7）：A 没有且不该有 B 的令牌——这正是配对要解决的，
// 所以这两个端点**必须在 token 闸门之前豁免**（handle_remote_request 前置）：
//   1. A → B  POST /api/gateway-pair-request  body: name=A机器名, nonce=A生成
//      B 存 pending + 给 hub 窗口发事件弹确认框；响应不含任何机密。
//   2. B 用户点同意 → gateway_pair_decide(true)：按 pending 里的名字签发令牌，
//      pair 串存进结果表（nonce 索引）；点拒绝 → 清 pending。
//   3. A 轮询 GET /api/gateway-pair-poll?nonce=… → pending / approved(含 pair,
//      单次消费) / declined / expired。
// 防滥用：pending 同时只有一个（新请求顶掉旧的，旧 nonce 作废）；120 秒过期；
// poll 必须持有 nonce；nonce 限 16~64 位十六进制。确认框永不自动同意。

/// 握手有效期（秒）。
const PAIR_TTL_SECS: u64 = 120;

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct PendingPair {
    pub nonce: String,
    pub name: String,
    pub expires_at: u64,
}

static PAIR_PENDING: Mutex<Option<PendingPair>> = Mutex::new(None);
static PAIR_RESULTS: Mutex<Option<std::collections::HashMap<String, (String, u64)>>> =
    Mutex::new(None);

fn pair_expired(ts: u64) -> bool {
    now_millis() > ts
}

/// nonce 形状校验：16~64 位十六进制。
fn nonce_ok(nonce: &str) -> bool {
    let n = nonce.len();
    (16..=64).contains(&n) && nonce.bytes().all(|b| b.is_ascii_hexdigit())
}

/// /api/gateway-pair-request（token 闸门前调用）。
pub(crate) fn pair_request_reply(app: &tauri::AppHandle, request: &crate::remote_routing::RemoteRequest) -> crate::remote_routing::RemoteReply {
    let name = request.param("name").unwrap_or_default();
    let nonce = request.param("nonce").unwrap_or_default();
    if !nonce_ok(&nonce) {
        return crate::remote_routing::RemoteReply::error(400, "bad_nonce", "nonce 非法");
    }
    let name = name.trim();
    if name.is_empty() || name.len() > 40 {
        return crate::remote_routing::RemoteReply::error(400, "bad_name", "客户端名称为空或过长");
    }
    *PAIR_PENDING.lock().unwrap() = Some(PendingPair {
        nonce,
        name: name.to_string(),
        expires_at: now_millis() + PAIR_TTL_SECS * 1000,
    });
    // 清掉过期旧结果，防表膨胀
    if let Some(results) = PAIR_RESULTS.lock().unwrap().as_mut() {
        results.retain(|_, (_, ts)| !pair_expired(*ts));
    }
    gw_log(app, &format!("[gateway] 收到配对请求 from {name}"));
    let _ = app.emit(
        "gateway-pair-request",
        serde_json::json!({ "name": name }),
    );
    crate::remote_routing::RemoteReply::ok(
        200,
        serde_json::json!({ "ok": true, "message": "请在对方电脑上确认配对请求" }),
    )
}

/// /api/gateway-pair-poll（token 闸门前调用）。approved 结果单次消费。
pub(crate) fn pair_poll_reply(request: &crate::remote_routing::RemoteRequest) -> crate::remote_routing::RemoteReply {
    let nonce = request.param("nonce").unwrap_or_default();
    if !nonce_ok(&nonce) {
        return crate::remote_routing::RemoteReply::error(400, "bad_nonce", "nonce 非法");
    }
    let results = PAIR_RESULTS.lock().unwrap();
    if let Some(results_map) = results.as_ref() {
        if let Some((pair, ts)) = results_map.get(&nonce) {
            if pair_expired(*ts) {
                drop(results);
                if let Some(map) = PAIR_RESULTS.lock().unwrap().as_mut() {
                    map.remove(&nonce);
                }
                return crate::remote_routing::RemoteReply::ok(
                    200,
                    serde_json::json!({ "status": "expired" }),
                );
            }
            let pair = pair.clone();
            drop(results);
            // 单次消费：取走即删
            if let Some(map) = PAIR_RESULTS.lock().unwrap().as_mut() {
                map.remove(&nonce);
            }
            return crate::remote_routing::RemoteReply::ok(
                200,
                serde_json::json!({ "status": "approved", "pair": pair }),
            );
        }
    }
    drop(results);
    let matches_pending = PAIR_PENDING
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| p.nonce == nonce && !pair_expired(p.expires_at))
        .unwrap_or(false);
    let status = if matches_pending { "pending" } else { "expired" };
    crate::remote_routing::RemoteReply::ok(200, serde_json::json!({ "status": status }))
}

/// B 端当前待确认请求（hub 弹确认框用；事件冗余一份，可拉兜底）。
#[tauri::command]
pub(crate) fn gateway_pair_pending() -> Option<PendingPair> {
    let pending = PAIR_PENDING.lock().unwrap();
    pending
        .clone()
        .filter(|p| !pair_expired(p.expires_at))
}

/// 请求方 nonce：32 位十六进制。nonce 是 A 自己生成自己轮询用的，
/// 只要不可枚举即可 —— 猜中也只能顶掉 B 的 pending 单槽（防滥用设计里已接受）。
fn fresh_nonce() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = std::process::id() as u64;
    format!("{:016x}{:016x}", nanos ^ pid.rotate_left(17), pid | nanos.swap_bytes())
}

/// 对 B 的局域网地址发一条免凭据 GET，回 (状态码, 响应体)。
/// 手写 TCP（与 probe_health 同款；对端是局域网 HTTP，不用 TLS）。
fn peer_get(target: &str, path: &str) -> Result<(u16, String), String> {
    let addr = crate::remote_servers::with_default_port_pub(target);
    let mut stream = std::net::TcpStream::connect(&addr)
        .map_err(|error| format!("连接 {addr} 失败: {error}"))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|error| format!("设置读超时失败: {error}"))?;
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(4)))
        .map_err(|error| format!("设置写超时失败: {error}"))?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    use std::io::Read as _;
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("发送请求失败: {error}"))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|error| format!("读取响应失败: {error}"))?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = match text.find("\r\n\r\n") {
        Some(index) => text[index + 4..].to_string(),
        None => String::new(),
    };
    Ok((status, body))
}

fn reply_message(status: u16, body: &str) -> Result<serde_json::Value, String> {
    if status == 0 {
        return Err("对方响应不是有效的 HTTP".to_string());
    }
    let value: serde_json::Value = serde_json::from_str(body.trim())
        .map_err(|error| format!("对方返回的不是合法 JSON（{status}）：{error}"))?;
    if status >= 400 {
        let message = value
            .get("message")
            .and_then(|item| item.as_str())
            .unwrap_or("请求被拒绝");
        return Err(message.to_string());
    }
    Ok(value)
}

/// 「从对方获取」请求方：向 B 的局域网地址发配对请求并轮询结果，B 端确认后
/// 返回 anyboot://pair 配对串。**必须 async + spawn_blocking**（轮询 + sleep）。
#[tauri::command]
pub(crate) async fn gateway_pair_fetch(app: tauri::AppHandle, peer: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || gateway_pair_fetch_blocking(&app, &peer))
        .await
        .map_err(|error| format!("配对任务失败: {error}"))?
}

fn gateway_pair_fetch_blocking(app: &tauri::AppHandle, peer: &str) -> Result<String, String> {
    let peer = peer.trim();
    if peer.is_empty() {
        return Err("先填对方的局域网地址".to_string());
    }
    let nonce = fresh_nonce();
    let name = crate::http_server::machine_name();
    let query = format!(
        "name={}&nonce={}",
        crate::remote_servers::encode_query_pub(&name),
        nonce
    );
    let (status, body) = peer_get(peer, &format!("/api/gateway-pair-request?{query}"))?;
    reply_message(status, &body)?;
    gw_log(app, &format!("[gateway] 已向 {peer} 发起配对请求"));
    // 轮询：2s 一次、最多 40 次（80s > B 端 120s TTL 的有效窗口足够）
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(2000));
        let (status, body) = match peer_get(peer, &format!("/api/gateway-pair-poll?nonce={nonce}")) {
            Ok(result) => result,
            Err(error) => return Err(format!("轮询对方确认结果失败：{error}")),
        };
        let value = reply_message(status, &body)?;
        match value.get("status").and_then(|item| item.as_str()).unwrap_or("") {
            "pending" => continue,
            "approved" => {
                let pair = value
                    .get("pair")
                    .and_then(|item| item.as_str())
                    .unwrap_or("")
                    .to_string();
                if pair.is_empty() {
                    return Err("对方已确认，但没返回配对串".to_string());
                }
                return Ok(pair);
            }
            _ => return Err("对方未确认（可能已拒绝或超时）".to_string()),
        }
    }
    Err("等待对方确认超时（80 秒）—— 让对方看一眼屏幕上的确认框".to_string())
}

/// B 端裁决：approve=签发令牌并把 pair 串放进结果表（A 的下一次 poll 取走）。
#[tauri::command]
pub(crate) fn gateway_pair_decide(
    app: tauri::AppHandle,
    approve: bool,
) -> Result<Option<PairIssued>, String> {
    let pending = PAIR_PENDING.lock().unwrap().take();
    let Some(pending) = pending else {
        return Err("没有待确认的配对请求".to_string());
    };
    if pair_expired(pending.expires_at) {
        return Err("配对请求已过期".to_string());
    }
    if !approve {
        gw_log(&app, &format!("[gateway] 拒绝配对 {}", pending.name));
        return Ok(None);
    }
    // 局域网互授配对：地址走当前隧道（没有就留空，由对方补扫），类型取 tunnel
    let issued = gateway_pair_issue(
        app.clone(),
        pending.name.clone(),
        String::new(),
        Some("tunnel".to_string()),
    )?;
    if let Some(map) = PAIR_RESULTS.lock().unwrap().as_mut() {
        map.insert(
            pending.nonce,
            (issued.pair.clone(), now_millis() + PAIR_TTL_SECS * 1000),
        );
    } else {
        let mut map = std::collections::HashMap::new();
        map.insert(
            pending.nonce,
            (issued.pair.clone(), now_millis() + PAIR_TTL_SECS * 1000),
        );
        *PAIR_RESULTS.lock().unwrap() = Some(map);
    }
    gw_log(&app, &format!("[gateway] 同意配对 {}", pending.name));
    Ok(Some(issued))
}

// ==================== 通道配置（settings 扩展，#33） ====================

/// 通道配置 + 实况聚合视图（前端通道卡单次拉取的数据源）。
/// 注意**不回传 named token**，只给 `named_token_set` 布尔。
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct GatewayConfig {
    pub mode: String,
    pub named_url: String,
    pub named_token_set: bool,
    pub autostart: bool,
    pub binary: BinaryStatus,
    pub tunnel: TunnelSnapshot,
}

/// 读配置聚合视图。
#[tauri::command]
pub(crate) fn gateway_config(app: tauri::AppHandle) -> GatewayConfig {
    let settings = crate::stored_settings(&app);
    let tunnel = app
        .try_state::<GatewayTunnelState>()
        .and_then(|state| state.snapshot.lock().ok().map(|s| s.clone()))
        .unwrap_or(TunnelSnapshot {
            running: false,
            mode: "quick".into(),
            url: String::new(),
            started_at: 0,
            restarts: 0,
            state: "stopped".into(),
            last_error: String::new(),
        });
    GatewayConfig {
        mode: settings.gateway_mode.clone(),
        named_url: settings.gateway_named_url.clone(),
        named_token_set: !settings.gateway_named_token.trim().is_empty(),
        autostart: settings.gateway_autostart,
        binary: binary_status(&app),
        tunnel,
    }
}

/// 保存通道配置。mode 取 "" | quick | named；named 时 token 必须已存或本次给出。
/// named_token 只在非空时覆盖（前端不必回传已存 token）。
#[tauri::command]
pub(crate) fn gateway_save_config(
    app: tauri::AppHandle,
    mode: String,
    named_token: String,
    named_url: String,
    autostart: bool,
) -> Result<crate::state::SettingsView, String> {
    let mode = mode.trim().to_lowercase();
    if !matches!(mode.as_str(), "" | "quick" | "named") {
        return Err(format!("未知通道模式: {mode}"));
    }
    if mode == "named"
        && named_token.trim().is_empty()
        && crate::stored_settings(&app).gateway_named_token.trim().is_empty()
    {
        return Err("命名隧道需要 Tunnel Token".to_string());
    }
    crate::settings_commands::update_settings_and_publish(
        &app,
        "gateway_save_config",
        &format!("远程接入：模式={}", if mode.is_empty() { "未配置" } else { &mode }),
        |settings| {
            settings.gateway_mode = mode.clone();
            if !named_token.trim().is_empty() {
                settings.gateway_named_token = named_token.trim().to_string();
            }
            settings.gateway_named_url = named_url.trim().to_string();
            settings.gateway_autostart = autostart;
        },
        |_| {},
    )
}

/// 三张通道卡统一的「启动软件时自动开启」开关。
/// channel 取 `tunnel`（Cloudflare 通道）/ `tailscale`（系统 Tailscale 直连）/
/// `tsnet`（内置 Tailscale 引擎）。Cloudflare 那张卡平时走 gateway_save_config
/// （它连带保存模式与 token），但这个入口同样有效 —— 三个复选框一套后端语义。
#[tauri::command]
pub(crate) fn gateway_set_autostart(
    app: tauri::AppHandle,
    channel: String,
    enabled: bool,
) -> Result<crate::state::SettingsView, String> {
    let key = channel.trim().to_string();
    let label = match key.as_str() {
        "tunnel" => "Cloudflare 通道",
        "tailscale" => "Tailscale 直连",
        "tsnet" => "内置 Tailscale 引擎",
        other => return Err(format!("未知通道: {other}")),
    };
    crate::settings_commands::update_settings_and_publish(
        &app,
        "gateway_set_autostart",
        &format!("远程接入：{label} 启动自启={enabled}"),
        |settings| match key.as_str() {
            "tunnel" => settings.gateway_autostart = enabled,
            "tailscale" => settings.tailscale_autostart = enabled,
            _ => settings.tsnet_autostart = enabled,
        },
        |_| {},
    )
}

/// 启动时的通道自启：勾了「启动软件时自动开启通道」就在这里拉起。
/// quick 也支持 —— 地址每次会变，但通道先就位，新地址在本页/出码里都是现成的；
/// named 必须已存 Tunnel Token，quick 不需要。mode 为空（未配置过）不自启。
pub(crate) fn autostart_tunnel(app: &tauri::AppHandle) {
    let settings = crate::stored_settings(app);
    // 没勾自启：残留通道不动，网关页列出来让用户选「沿用 / 终止」
    if !settings.gateway_autostart || settings.gateway_mode.is_empty() {
        return;
    }
    if settings.gateway_mode == "named" && settings.gateway_named_token.trim().is_empty() {
        return;
    }
    let app = app.clone();
    let mode = settings.gateway_mode.clone();
    let token = settings.gateway_named_token.clone();
    let url = settings.gateway_named_url.clone();
    std::thread::spawn(move || {
        // 自启 + 崩溃残留：有地址记录的残留直接沿用（快速隧道地址跨软件重启保持）；
        // 没记录的（拿不到地址）走 tunnel_start_blocking 清掉重开。
        let orphans = tunnel_orphans_inner(&app);
        if let Some(orphan) = orphans.iter().find(|o| !o.url.is_empty()) {
            if tunnel_adopt_inner(&app, orphan.pid).is_ok() {
                return;
            }
        }
        let _ = tunnel_start_blocking(&app, mode, token, url);
    });
}

// ==================== 测试 ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_platforms_have_assets_and_hashes() {
        let keys = [
            ("windows", "amd64"),
            ("linux", "amd64"),
            ("linux", "arm64"),
            ("darwin", "amd64"),
            ("darwin", "arm64"),
        ];
        for key in keys {
            assert!(asset_name(key).starts_with("cloudflared-"), "{key:?}");
            let hash = expected_sha256(key).expect("哈希表缺项");
            assert_eq!(hash.len(), 64, "{key:?} 哈希应为 64 位十六进制");
            assert!(hash.chars().all(|c| c.is_ascii_hexdigit()), "{key:?}");
        }
        assert!(asset_url(("windows", "amd64")).contains(CLOUDFLARED_VERSION));
        assert!(asset_url(("darwin", "arm64")).ends_with(".tgz"));
    }

    #[test]
    fn sha256_file_matches_known_vector() {
        // "abc" 的 SHA-256 标准测试向量。
        let path = std::env::temp_dir().join(format!(
            "anyboot-gw-test-{}",
            std::process::id()
        ));
        std::fs::write(&path, b"abc").unwrap();
        let hash = sha256_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[cfg(unix)]
    #[test]
    fn manual_probe_rejects_non_executable() {
        let path = std::env::temp_dir().join(format!(
            "anyboot-gw-noexec-{}",
            std::process::id()
        ));
        std::fs::write(&path, b"not a binary").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = probe_manual_binary(&path).unwrap_err();
        std::fs::remove_file(&path).ok();
        assert!(error.contains("可执行权限"), "{error}");
    }

    #[test]
    fn nonce_validation() {
        assert!(nonce_ok("abcdef0123456789"));
        assert!(nonce_ok("ABCDEF0123456789ABCDEF0123456789"));
        assert!(!nonce_ok("short"));
        assert!(!nonce_ok("zzzzzzzzzzzzzzzz"));
        assert!(!nonce_ok(""));
    }

    #[test]
    fn parses_quick_url_from_log_line() {
        let line = "  |  https://odd-months-win.trycloudflare.com                              |";
        assert_eq!(
            parse_quick_url(line).as_deref(),
            Some("https://odd-months-win.trycloudflare.com")
        );
        assert_eq!(parse_quick_url("2026-10-23T08:00:00Z INF +----------------------------"), None);
        assert_eq!(parse_quick_url("https://example.com/other"), None);
        // 带尾随标点/空白也能截干净。
        assert_eq!(
            parse_quick_url("url=https://a-b-c.trycloudflare.com, connIndex=0").as_deref(),
            Some("https://a-b-c.trycloudflare.com")
        );
    }

    // ---------- 配对串 v2（S3） ----------

    /// 真实 cloudflared tunnel token 的形状：base64(JSON{a,t,s})。
    const REAL_TUNNEL_TOKEN: &str = "eyJhIjoiMDEyMzQ1Njc4OWFiY2RlZiIsInQiOiI3ZjNjOWY0YS0xYjJjLTRkNWUtOGY5MC1hYmNkZWYwMTIzNDUiLCJzIjoiYzJWamNtVjBMWE5sWTNKbGRBPT0ifQ==";

    #[test]
    fn build_pair_string_emits_v2_with_every_field() {
        let pair = build_pair_string(
            "mid-abc",
            "windows",
            "书房 PC",
            "tunnel",
            "https://odd-months-win.trycloudflare.com",
            "gw1234567890abcdef",
            "7f3c9f4a-1b2c-4d5e-8f90-abcdef012345",
        );
        assert!(pair.starts_with("anyboot://pair?v=2&"), "必须是 v2：{pair}");
        // 名称里的空格/中文必须转义，否则下游按 `&`/`=` 切分会切错
        assert!(pair.contains("&host=%E4%B9%A6%E6%88%BF%20PC"), "{pair}");
        assert!(pair.contains("&mid=mid-abc"), "{pair}");
        assert!(pair.contains("&os=windows"), "{pair}");
        assert!(pair.contains("&token=gw1234567890abcdef"), "{pair}");
        assert!(pair.contains("&tunnel_id=7f3c9f4a-"), "{pair}");
        // 令牌不做百分号编码：它是 32 位十六进制，编了反而要多解一层
        assert!(!pair.contains("token=gw1234567890abcdef%"), "{pair}");
    }

    #[test]
    fn build_pair_string_maps_channel_to_block_type() {
        // `type=` 必须是**区块** id，`funnel` 与 `ts` 同属 ts 区块
        let ty = |kind: &str| {
            let pair = build_pair_string("m", "linux", "h", kind, "https://a.ts.net", "t", "");
            pair.split("&type=")
                .nth(1)
                .and_then(|rest| rest.split('&').next())
                .unwrap_or("")
                .to_string()
        };
        assert_eq!(ty("tunnel"), "cloudflare");
        assert_eq!(ty("ts"), "ts");
        assert_eq!(ty("funnel"), "ts");
        assert_eq!(ty("tsnet"), "tsnet");
        // 传入未知通道（不该发生，但不许画出 `type=` 空串让客户端拒收）
        assert_eq!(ty("nonsense"), "");
    }

    #[test]
    fn pair_string_round_trips_through_parser() {
        let pair = build_pair_string(
            "mid-abc",
            "macos",
            "书房 PC",
            "funnel",
            "https://win-pc.tailabc.ts.net",
            "gwdeadbeef",
            "",
        );
        let parsed = crate::remote_servers::parse_pair_string(&pair).expect("自家签的串必须能解");
        assert_eq!(parsed.mid, "mid-abc");
        assert_eq!(parsed.os, "macos");
        assert_eq!(parsed.host, "书房 PC");
        assert_eq!(parsed.kind, "ts", "funnel 落在 ts 区块");
        assert_eq!(parsed.tunnel, "https://win-pc.tailabc.ts.net");
        assert_eq!(parsed.token, "gwdeadbeef");
        assert_eq!(parsed.tunnel_id, "");
    }

    #[test]
    fn decode_tunnel_id_reads_named_tunnel_token() {
        assert_eq!(
            decode_tunnel_id(REAL_TUNNEL_TOKEN),
            "7f3c9f4a-1b2c-4d5e-8f90-abcdef012345"
        );
        // 去掉填充也要能解（用户从脚本/配置文件里拷回来常丢 `=`）
        let unpadded = REAL_TUNNEL_TOKEN.trim_end_matches('=');
        assert_eq!(
            decode_tunnel_id(unpadded),
            "7f3c9f4a-1b2c-4d5e-8f90-abcdef012345"
        );
        // quick 模式没有 token / 垃圾输入 → 空（这个字段只用于「共用隧道」提示，缺了不影响连接）
        assert_eq!(decode_tunnel_id(""), "");
        assert_eq!(decode_tunnel_id("   "), "");
        assert_eq!(decode_tunnel_id("not-base64!!!"), "");
        // 合法 base64 但不是 cloudflared token
        assert_eq!(decode_tunnel_id("aGVsbG8gd29ybGQ="), "");
    }

    #[test]
    fn decode_base64_accepts_url_safe_alphabet() {
        // `-` `_` 是 URL-safe 字母表里的 62/63；标准表里它们是 `+` `/`
        assert_eq!(decode_base64("-_-_"), Some(vec![0xfb, 0xff, 0xbf]));
        assert_eq!(decode_base64("+/+/"), Some(vec![0xfb, 0xff, 0xbf]));
        assert_eq!(decode_base64("不是 base64"), None);
    }

    #[test]
    fn decode_tunnel_id_ignores_stray_t_in_values() {
        // 值里恰好出现 "t" 时不能误取：必须先看到 `"t"` 再看到冒号
        let payload = r#"{"a":"t","t":"11111111-2222-3333-4444-555555555555"}"#;
        let encoded = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(payload.as_bytes())
        };
        assert_eq!(
            decode_tunnel_id(&encoded),
            "11111111-2222-3333-4444-555555555555"
        );
    }
}
