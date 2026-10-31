//! 检查更新：GitHub releases/latest → 下载（镜像回退 + SHA-256）→ 覆盖安装。
//!
//! 设计定稿见 `docs/检查更新-设计.md`，UI 稿见 `docs/update-ui-mockup.html`。
//!
//! 三条铁律（与设计文档一致）：
//! 1. **双通道检查**：主通道 `HEAD github.com/<repo>/releases/latest` 读 302 的
//!    `Location` 取 tag —— 零速率限制；匿名 API 只当补充通道取更新日志，
//!    403 就静默降级（出口 IP 共享配额，实测会被陌生人耗光）。
//! 2. **严格只升不降**：版本解析失败按「无更新」处理，绝不猜。
//! 3. **安装要么成功要么原样不动**：SHA-256 校验不过不安装；覆盖前先落到
//!    临时名，成功才原子 mv。
//!
//! 线程模型沿用 IQ6 定稿：不引 async 运行时，网络与安装动作全在
//! `std::thread` 里跑，进度/状态经 Tauri 事件推给前端。

use super::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 仓库坐标（检查更新只认这一处）
pub(crate) const UPDATE_REPO: &str = "lidiaoo/anyboot";
/// 状态推送事件名（与 `ui-state-changed` 并列，不混进整份快照：
/// 下载进度 120ms 一次，塞进快照会连带重建托盘菜单，纯浪费）
pub(crate) const UPDATE_STATE_EVENT: &str = "update-state";
/// 自动检查：启动后延迟（秒）
const AUTO_CHECK_DELAY_SECS: u64 = 30;
/// 自动检查：周期（小时）—— 302 探测不限速，这个间隔是给「补充通道 API」留余量
const AUTO_CHECK_PERIOD_SECS: u64 = 12 * 3600;
/// 下载源前缀（与 hub_gateway 的 MIRROR_PREFIXES 同一批镜像：国内直连 GitHub
/// 常常几 KB/s 甚至断流，逐源回退是既有成熟做法）
const MIRROR_PREFIXES: [(&str, &str); 2] =
    [("ghfast", "https://ghfast.top/"), ("ghproxy", "https://gh-proxy.com/")];

/// 更新状态机（唯一权威，前端照它渲染）
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateState {
    /// idle / checking / up_to_date / available / downloading / verifying /
    /// ready_to_install / installing / error / updated
    pub(crate) phase: String,
    pub(crate) current_version: String,
    pub(crate) latest_version: String,
    /// release 正文（纯文本展示，前端不 innerHTML）
    pub(crate) notes: String,
    pub(crate) published_at: String,
    pub(crate) asset_name: String,
    pub(crate) asset_size: u64,
    pub(crate) downloaded: u64,
    pub(crate) total: u64,
    /// 非空即错误态；文案必须带「为什么 + 下一步该做什么」
    pub(crate) error: String,
    pub(crate) checked_at: String,
    /// 下载来源（直连 / 镜像名），下载完成后给用户一个可核对的凭据
    pub(crate) source: String,
    /// 用户点过「跳过这个版本」—— 仍为 available，只是不弹提醒
    pub(crate) skipped: bool,
}

/// 进程内单例：状态 + 单飞闸 + 已选好的资产（URL 不暴露给前端）
pub(crate) struct UpdateManager {
    pub(crate) state: std::sync::Mutex<UpdateState>,
    pub(crate) busy: AtomicBool,
    /// 选中的资产：下载与校验都用它
    asset: std::sync::Mutex<Option<Asset>>,
    /// 已下载并通过校验的安装包
    staged: std::sync::Mutex<Option<PathBuf>>,
}

impl Default for UpdateManager {
    fn default() -> Self {
        Self {
            state: std::sync::Mutex::new(UpdateState {
                phase: "idle".to_string(),
                ..Default::default()
            }),
            busy: AtomicBool::new(false),
            asset: std::sync::Mutex::new(None),
            staged: std::sync::Mutex::new(None),
        }
    }
}

#[derive(Clone, Debug)]
struct Asset {
    name: String,
    url: String,
    size: u64,
}

// ============================================================================
// 版本比较（纯函数，可单测）
// ============================================================================

/// 解析 `v0.2.1` / `0.2.1` → (0,2,1)。只认纯数字段；带预发布后缀的整体拒绝
/// （本项目不发预发布版，认不出就当没有 —— 与 GRUB 零回落同一哲学：
/// 宁可不显示，不显示错的）。
pub(crate) fn parse_version(raw: &str) -> Option<(u64, u64, u64)> {
    let text = raw.trim();
    let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
    if text.is_empty() {
        return None;
    }
    let mut parts = text.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = match parts.next() {
        Some(value) => value.parse::<u64>().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// 本机版本（`app.package_info().version`）→ 三元组。带构建元数据时只取前三段。
pub(crate) fn current_triple(raw: &str) -> (u64, u64, u64) {
    parse_version(raw).unwrap_or((0, 0, 0))
}

/// latest 是否严格新于 current。任何一方解析失败都返回 false（不提示更新）。
pub(crate) fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

// ============================================================================
// 资产矩阵（纯函数，可单测）
// ============================================================================

/// 按平台给出**有序**的资产候选名（发布 job 必须照这个名字产出，见设计文档 §6）
pub(crate) fn asset_candidates(version: &str) -> Vec<String> {
    let v = version.trim().trim_start_matches(['v', 'V']);
    let mut names: Vec<String> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        names.push(format!("AnyBoot_{v}_universal.dmg"));
    }
    #[cfg(target_os = "windows")]
    {
        names.push(format!("AnyBoot_{v}_x64-setup.exe"));
        names.push(format!("AnyBoot_{v}_x64.msi"));
    }
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("APPIMAGE").is_some() {
            names.push(format!("AnyBoot_{v}_amd64.AppImage"));
        }
        names.push(format!("AnyBoot_{v}_amd64.deb"));
        names.push(format!("AnyBoot_{v}_x86_64.rpm"));
        names.push(format!("AnyBoot_{v}_amd64.AppImage"));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = v;
    }
    names
}

// ============================================================================
// 网络：检查
// ============================================================================

fn user_agent(app: &AppHandle) -> String {
    format!("AnyBoot/{}", app.package_info().version)
}

fn agent(redirects: u32) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(20))
        .redirects(redirects)
        .build()
}

fn download_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .build()
}

/// 从 `Location: https://github.com/o/r/releases/tag/v0.2.1` 里抠 tag
pub(crate) fn tag_from_location(location: &str) -> Option<String> {
    let marker = "/releases/tag/";
    let index = location.find(marker)?;
    let tag = location[index + marker.len()..]
        .split(['?', '#', '/'])
        .next()
        .unwrap_or("")
        .trim();
    if tag.is_empty() {
        None
    } else {
        Some(tag.to_string())
    }
}

/// 主通道：302 探测（不跟随重定向，读 Location）
fn probe_latest_tag(app: &AppHandle) -> Result<String, String> {
    let url = format!("https://github.com/{UPDATE_REPO}/releases/latest");
    let response = agent(0)
        .head(&url)
        .set("User-Agent", &user_agent(app))
        .call();
    let location = match response {
        Ok(resp) => resp.header("location").map(str::to_string),
        // 关闭跟随重定向后，3xx 也可能以 Err(Status) 形式返回
        Err(ureq::Error::Status(code, resp)) if (300..400).contains(&code) => {
            resp.header("location").map(str::to_string)
        }
        Err(error) => return Err(format!("探测最新版本失败：{error}")),
    };
    let location = location.ok_or_else(|| "探测最新版本失败：响应里没有 Location".to_string())?;
    tag_from_location(&location).ok_or_else(|| format!("探测最新版本失败：Location 认不出 tag（{location}）"))
}

/// 补充通道：匿名 API（可能 403，失败不算错）
fn fetch_api_json(app: &AppHandle) -> Option<serde_json::Value> {
    let url = format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest");
    let response = agent(5)
        .get(&url)
        .set("User-Agent", &user_agent(app))
        .set("Accept", "application/vnd.github+json")
        .call()
        .ok()?;
    // ureq 没开 json feature（项目只开 tls）：手动转字符串再解析，不引新 feature
    let text = response.into_string().ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()
}

/// 从 API 的 assets 里按候选名挑第一个存在的
fn pick_asset_from_api(json: &serde_json::Value, version: &str) -> Option<Asset> {
    let candidates = asset_candidates(version);
    let assets = json.get("assets")?.as_array()?;
    for wanted in candidates {
        for item in assets {
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if name == wanted {
                let url = item
                    .get("browser_download_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !is_trusted_url(&url) {
                    continue;
                }
                let size = item.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                return Some(Asset { name: name.to_string(), url, size });
            }
        }
    }
    None
}

/// 只信 GitHub 自家域名：API 的下载地址 / 页面 / 对象存储
pub(crate) fn is_trusted_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    let rest = &url["https://".len()..];
    let host = rest.split('/').next().unwrap_or("");
    host == "github.com"
        || host == "api.github.com"
        || host.ends_with(".githubusercontent.com")
        || host.ends_with(".github.com")
}

/// API 不可用时的兜底：按命名规则拼 URL 并 HEAD 探测存在性
fn probe_asset_by_name(app: &AppHandle, version: &str) -> Option<Asset> {
    for name in asset_candidates(version) {
        let url = format!(
            "https://github.com/{UPDATE_REPO}/releases/download/v{version}/{name}"
        );
        let result = agent(5)
            .head(&url)
            .set("User-Agent", &user_agent(app))
            .call();
        if let Ok(resp) = result {
            if resp.status() == 200 {
                let size = resp
                    .header("content-length")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                return Some(Asset { name, url, size });
            }
        }
    }
    None
}

fn now_text() -> String {
    // 不引 chrono：本地时间只用于「最近检查」展示，秒级 Unix 时间戳够用，
    // 前端自己 format。
    format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    )
}

fn set_phase(app: &AppHandle, update: impl FnOnce(&mut UpdateState)) {
    let manager = app.state::<UpdateManager>();
    let snapshot = {
        let mut state = manager.state.lock().expect("update mutex poisoned");
        update(&mut state);
        state.clone()
    };
    let _ = app.emit(UPDATE_STATE_EVENT, &snapshot);
}

fn snapshot(app: &AppHandle) -> UpdateState {
    app.state::<UpdateManager>()
        .state
        .lock()
        .expect("update mutex poisoned")
        .clone()
}

/// 检查更新（同步，两个短请求；最坏几秒，调用方在命令线程上跑）
pub(crate) fn check_blocking(app: &AppHandle) -> Result<UpdateState, String> {
    let current = app.package_info().version.to_string();
    set_phase(app, |state| {
        state.phase = "checking".to_string();
        state.current_version = current.clone();
        state.error.clear();
    });

    let tag = probe_latest_tag(app)?;
    let latest = tag.trim_start_matches(['v', 'V']).to_string();
    let api = fetch_api_json(app);

    let notes = api
        .as_ref()
        .and_then(|json| json.get("body"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let published = api
        .as_ref()
        .and_then(|json| json.get("published_at"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let asset = api
        .as_ref()
        .and_then(|json| pick_asset_from_api(json, &latest))
        .or_else(|| probe_asset_by_name(app, &latest));

    let skip = stored_settings(app).update_skip_version.trim().to_string();
    let newer = is_newer(&latest, &current);

    let result = {
        let manager = app.state::<UpdateManager>();
        *manager.asset.lock().expect("update mutex poisoned") = asset.clone();
        let mut state = manager.state.lock().expect("update mutex poisoned");
        state.current_version = current.clone();
        state.latest_version = latest.clone();
        state.notes = notes;
        state.published_at = published;
        state.checked_at = now_text();
        state.downloaded = 0;
        state.total = asset.as_ref().map(|a| a.size).unwrap_or(0);
        state.asset_size = state.total;
        state.asset_name = asset.as_ref().map(|a| a.name.clone()).unwrap_or_default();
        state.source.clear();
        state.error.clear();
        state.skipped = !skip.is_empty() && skip == latest;
        state.phase = if !newer {
            "up_to_date".to_string()
        } else if asset.is_none() {
            // 有新版但本平台没有安装包：仍然是 error（文案给出下一步），
            // 与「已是最新」不是一回事，绝不能混。
            state.error = format!(
                "发现 v{latest}，但 release 里没有本平台的安装包（期望 {}）",
                asset_candidates(&latest).join(" / ")
            );
            "error".to_string()
        } else {
            "available".to_string()
        };
        state.clone()
    };
    let _ = app.emit(UPDATE_STATE_EVENT, &result);
    trace(
        app,
        "update",
        &format!(
            "检查完成 current={current} latest={latest} phase={} asset={}",
            result.phase, result.asset_name
        ),
    );
    Ok(result)
}

// ============================================================================
// 网络：下载 + SHA-256 校验
// ============================================================================

/// 下载源顺序：auto = 直连 + 全部镜像；direct = 只直连；指定镜像 = 只用它
fn download_sources(settings: &Settings) -> Vec<(String, String)> {
    let choice = settings.update_mirror.trim();
    let mut list: Vec<(String, String)> = Vec::new();
    let direct = || ("直连".to_string(), String::new());
    match choice {
        "direct" => list.push(direct()),
        name if !name.is_empty() && name != UPDATE_MIRROR_AUTO => {
            if let Some((label, prefix)) = MIRROR_PREFIXES.iter().find(|(label, _)| *label == name) {
                list.push((label.to_string(), prefix.to_string()));
            } else {
                list.push(direct());
            }
        }
        _ => {
            list.push(direct());
            for (label, prefix) in MIRROR_PREFIXES {
                list.push((label.to_string(), prefix.to_string()));
            }
        }
    }
    list
}

fn apply_prefix(prefix: &str, url: &str) -> String {
    if prefix.is_empty() {
        url.to_string()
    } else {
        format!("{prefix}{url}")
    }
}

/// 流式下载 + 120ms 节流进度（与 hub_gateway::download_to_tmp_observed 同策略：
/// 末块必发，进度条不会停在 98% 看着像卡死）
fn download_to(
    app: &AppHandle,
    url: &str,
    target: &Path,
) -> Result<u64, String> {
    let response = download_agent()
        .get(url)
        .set("User-Agent", &user_agent(app))
        .call()
        .map_err(|error| format!("HTTP 请求失败：{error}"))?;
    let total = response
        .header("content-length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(target)
        .map_err(|error| format!("创建临时文件失败：{error}"))?;
    let mut buffer = [0u8; 256 * 1024];
    let mut received: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("下载中断 @ {received} 字节：{error}"))?;
        if read == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buffer[..read])
            .map_err(|error| format!("写临时文件失败：{error}"))?;
        received += read as u64;
        if last_emit.elapsed() >= Duration::from_millis(120) {
            last_emit = std::time::Instant::now();
            set_phase(app, |state| {
                state.downloaded = received;
                if total > 0 {
                    state.total = total;
                }
            });
        }
    }
    set_phase(app, |state| state.downloaded = received);
    if total != 0 && received != total {
        return Err(format!("下载不完整（{received}/{total} 字节）"));
    }
    Ok(received)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| format!("打开文件失败：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 256 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("读取文件失败：{error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 解析 `SHA256SUMS.txt`：`<64 hex>  <name>`
pub(crate) fn parse_sums(text: &str, wanted: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next().unwrap_or("");
        let name = name.trim_start_matches('*');
        if name == wanted && hash.len() == 64 {
            return Some(hash.to_lowercase());
        }
    }
    None
}

fn fetch_sums(app: &AppHandle, tag: &str, prefix: &str) -> Result<String, String> {
    let url = format!(
        "https://github.com/{UPDATE_REPO}/releases/download/{tag}/SHA256SUMS.txt"
    );
    let response = agent(5)
        .get(&apply_prefix(prefix, &url))
        .set("User-Agent", &user_agent(app))
        .call()
        .map_err(|error| format!("取 SHA256SUMS.txt 失败：{error}"))?;
    response
        .into_string()
        .map_err(|error| format!("读 SHA256SUMS.txt 失败：{error}"))
}

fn staging_dir() -> PathBuf {
    std::env::temp_dir().join("anyboot-update")
}

/// 下载入口（在线程里跑）。成功 → ready_to_install；失败 → error（文案带下一步）
pub(crate) fn download_worker(app: AppHandle) {
    let manager_guard = app.state::<UpdateManager>();
    let (asset, tag) = {
        let asset = manager_guard.asset.lock().expect("update mutex poisoned").clone();
        let state = manager_guard.state.lock().expect("update mutex poisoned");
        (asset, state.latest_version.clone())
    };
    drop(manager_guard);

    let result = (|| -> Result<(PathBuf, String), String> {
        let asset = asset.ok_or_else(|| "还没有选中的安装包，请先检查更新".to_string())?;
        if !is_trusted_url(&asset.url) {
            return Err("安装包地址不在可信域名内（只接受 github.com）".to_string());
        }
        let dir = staging_dir();
        std::fs::create_dir_all(&dir).map_err(|error| format!("创建临时目录失败：{error}"))?;
        let temp = dir.join(format!("{}.downloading", asset.name));
        let final_path = dir.join(&asset.name);

        let settings = stored_settings(&app);
        let mut failures: Vec<String> = Vec::new();
        for (label, prefix) in download_sources(&settings) {
            set_phase(&app, |state| {
                state.phase = "downloading".to_string();
                state.source = label.clone();
                state.downloaded = 0;
                state.error.clear();
            });
            let url = apply_prefix(&prefix, &asset.url);
            match download_to(&app, &url, &temp) {
                Ok(_) => {
                    set_phase(&app, |state| state.phase = "verifying".to_string());
                    let sums = fetch_sums(&app, &format!("v{tag}"), &prefix);
                    let expected = match sums {
                        Ok(text) => parse_sums(&text, &asset.name),
                        Err(error) => {
                            failures.push(format!("{label}：{error}"));
                            let _ = std::fs::remove_file(&temp);
                            continue;
                        }
                    };
                    let expected = match expected {
                        Some(hash) => hash,
                        None => {
                            failures.push(format!("{label}：SHA256SUMS.txt 里没有 {}", asset.name));
                            let _ = std::fs::remove_file(&temp);
                            continue;
                        }
                    };
                    let actual = sha256_file(&temp)?;
                    if actual != expected {
                        failures.push(format!("{label}：SHA-256 不匹配"));
                        let _ = std::fs::remove_file(&temp);
                        continue;
                    }
                    std::fs::rename(&temp, &final_path)
                        .map_err(|error| format!("落盘失败：{error}"))?;
                    return Ok((final_path, label));
                }
                Err(error) => {
                    failures.push(format!("{label}：{error}"));
                    let _ = std::fs::remove_file(&temp);
                }
            }
        }
        Err(format!(
            "下载失败（已尝试 {} 个源）：{}。下一步：点「重试」重新下载；反复失败请检查网络或代理设置。",
            failures.len(),
            failures.join("；")
        ))
    })();

    match result {
        Ok((path, label)) => {
            *app
                .state::<UpdateManager>()
                .staged
                .lock()
                .expect("update mutex poisoned") = Some(path);
            set_phase(&app, |state| {
                state.phase = "ready_to_install".to_string();
                state.source = label;
                state.error.clear();
                state.downloaded = state.total;
            });
            trace(&app, "update", "下载完成并通过 SHA-256 校验");
        }
        Err(error) => {
            trace(&app, "update", &format!("下载失败：{error}"));
            set_phase(&app, |state| {
                state.phase = "error".to_string();
                state.error = error;
            });
        }
    }
    app.state::<UpdateManager>().busy.store(false, Ordering::SeqCst);
}

// ============================================================================
// 安装（三平台策略）
// ============================================================================

#[cfg(target_os = "macos")]
fn install_macos(bundle: &Path, dmg: &Path) -> Result<(), String> {
    // 目标 .app 与所在目录：用户可能装在 ~/Applications，不硬编码 /Applications
    let target = bundle.to_path_buf();
    // 脚本与挂载点都放临时目录：/Applications 是 root:admin 775，非管理员用户
    // 根本写不进去 —— 把「能不能写目标目录」和「能不能落脚本」两件事分开。
    let scratch = std::env::temp_dir().join("anyboot-update");
    std::fs::create_dir_all(&scratch)
        .map_err(|error| format!("创建临时目录失败：{error}"))?;
    let mounted = scratch.join("mnt");
    let script_path = scratch.join("install-macos.sh");
    let script = format!(
        r#"#!/bin/bash
set -u
sleep 1
MNT='{mnt}'
DMG='{dmg}'
TARGET='{target}'
NEW="$TARGET.new"
rm -rf "$MNT"; mkdir -p "$MNT"
if ! hdiutil attach -nobrowse -readonly -mountpoint "$MNT" "$DMG" >/dev/null 2>&1; then
  echo "挂载失败" >&2; exit 1
fi
APP=$(/usr/bin/find "$MNT" -maxdepth 1 -name '*.app' -print -quit)
if [ -z "$APP" ]; then hdiutil detach "$MNT" >/dev/null 2>&1; echo "dmg 里没有 .app" >&2; exit 1; fi
rm -rf "$NEW"
/usr/bin/ditto "$APP" "$NEW" || ( hdiutil detach "$MNT" >/dev/null 2>&1; exit 1 )
hdiutil detach "$MNT" >/dev/null 2>&1 || true
rm -rf "$TARGET.old"
if [ -d "$TARGET" ]; then mv "$TARGET" "$TARGET.old" || exit 1; fi
if ! mv "$NEW" "$TARGET"; then
  [ -d "$TARGET.old" ] && mv "$TARGET.old" "$TARGET"
  exit 1
fi
/usr/bin/open "$TARGET"
rm -rf "$TARGET.old"
rm -f '{script}'
exit 0
"#,
        mnt = mounted.display(),
        dmg = dmg.display(),
        target = target.display(),
        script = script_path.display(),
    );
    std::fs::write(&script_path, script).map_err(|error| format!("写安装脚本失败：{error}"))?;
    std::process::Command::new("/bin/bash")
        .arg(&script_path)
        .spawn()
        .map_err(|error| format!("启动安装脚本失败：{error}"))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn install_windows(installer: &Path) -> Result<(), String> {
    // NSIS 安装器 manifest 要求管理员：CreateProcess 会 ERROR_ELEVATION_REQUIRED，
    // 必须走 ShellExecute 的 runas。不引 windows crate，用 PowerShell 的
    // Start-Process -Verb RunAs 等价实现。
    let escaped = installer.display().to_string().replace('\'', "''");
    let command = format!(
        "Start-Process -FilePath '{escaped}' -ArgumentList '/S' -Verb RunAs"
    );
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &command])
        .spawn()
        .map_err(|error| format!("启动安装器失败：{error}"))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_linux(package: &Path) -> Result<(), String> {
    let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);
    if let Some(current) = appimage {
        // 当前就是 AppImage 运行：原子替换自身，留一份 .old 供回退
        let new_path = current.with_extension("AppImage.new");
        std::fs::copy(package, &new_path).map_err(|error| format!("复制新版本失败：{error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new_path, std::fs::Permissions::from_mode(0o755))
                .map_err(|error| format!("设置可执行位失败：{error}"))?;
        }
        let old_path = current.with_extension("AppImage.old");
        if current.exists() {
            let _ = std::fs::rename(&current, &old_path);
        }
        std::fs::rename(&new_path, &current).map_err(|error| format!("替换程序失败：{error}"))?;
        std::process::Command::new(&current)
            .spawn()
            .map_err(|error| format!("重启新版本失败：{error}"))?;
        return Ok(());
    }
    // deb / rpm：走 pkexec 提权安装（保持用户配置目录不动）
    let name = package
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let (program, args): (&str, Vec<String>) = if name.ends_with(".deb") {
        ("dpkg", vec!["-i".to_string(), package.display().to_string()])
    } else if name.ends_with(".rpm") {
        (
            "rpm",
            vec!["-Uvh".to_string(), "--replacepkgs".to_string(), package.display().to_string()],
        )
    } else {
        return Err("未知的安装包类型（只支持 .deb / .rpm / AppImage）".to_string());
    };
    let status = std::process::Command::new("pkexec")
        .arg(program)
        .args(&args)
        .status()
        .map_err(|error| format!("调用 pkexec 失败：{error}"))?;
    if !status.success() {
        return Err("安装失败（pkexec 返回非零）".to_string());
    }
    Ok(())
}

/// 找当前运行的 .app 包（macOS）
#[cfg(target_os = "macos")]
fn current_app_bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| format!("定位自身失败：{error}"))?;
    for ancestor in exe.ancestors() {
        if ancestor.extension().map(|e| e == "app").unwrap_or(false) {
            return Ok(ancestor.to_path_buf());
        }
    }
    Err("没找到 .app 包（不是从应用包启动的？）".to_string())
}

/// 安装入口（在线程里跑；安装前由命令层过安全闸）
pub(crate) fn install_worker(app: AppHandle) {
    let staged = app
        .state::<UpdateManager>()
        .staged
        .lock()
        .expect("update mutex poisoned")
        .clone();
    let Some(package) = staged else {
        set_phase(&app, |state| {
            state.phase = "error".to_string();
            state.error = "还没有下载好的安装包，请先下载".to_string();
        });
        app.state::<UpdateManager>().busy.store(false, Ordering::SeqCst);
        return;
    };

    set_phase(&app, |state| state.phase = "installing".to_string());

    #[cfg(target_os = "macos")]
    let outcome = current_app_bundle().and_then(|bundle| install_macos(&bundle, &package));
    #[cfg(target_os = "windows")]
    let outcome = install_windows(&package);
    #[cfg(target_os = "linux")]
    let outcome = install_linux(&package);
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let outcome: Result<(), String> = Err("当前平台不支持应用内更新".to_string());

    match outcome {
        Ok(()) => {
            trace(&app, "update", "安装动作已交接，准备退出");
            // 不在这里 sleep：macOS 的脚本自己等 1s 再挂载（等本进程退出），
            // Windows 的安装器会自己处理运行中实例，Linux 已经 spawn 了新进程。
            app.exit(0);
        }
        Err(error) => {
            trace(&app, "update", &format!("安装失败：{error}"));
            set_phase(&app, |state| {
                state.phase = "error".to_string();
                state.error = format!("{error}。下一步：可到 Releases 页手动下载安装。");
            });
            app.state::<UpdateManager>().busy.store(false, Ordering::SeqCst);
        }
    }
}

// ============================================================================
// 命令层
// ============================================================================

#[tauri::command]
pub(crate) fn update_state(app: AppHandle) -> UpdateState {
    let mut state = snapshot(&app);
    state.current_version = app.package_info().version.to_string();
    state
}

/// 手动/自动检查的实际实现（同步，供后台线程与 async 命令共用）
pub(crate) fn check_with_busy(app: AppHandle) -> Result<UpdateState, String> {
    let manager = app.state::<UpdateManager>();
    if manager.busy.swap(true, Ordering::SeqCst) {
        return Err("更新任务正在进行中（下载或安装），请稍候".to_string());
    }
    let result = check_blocking(&app);
    manager.busy.store(false, Ordering::SeqCst);
    match result {
        Ok(state) => Ok(state),
        Err(error) => {
            set_phase(&app, |state| {
                state.phase = "error".to_string();
                state.error = format!("{error}。下一步：检查网络后点「重试」。");
            });
            Err(error)
        }
    }
}

/// 检查更新命令（async + spawn_blocking）。
///
/// **为什么不能是同步命令**：命令跑在 WebView 的 IPC 回调线程上，而这里有两个
/// 网络请求（连接超时 10s、读超时 20s）——网络被黑洞时窗口会整块卡住，正是
/// `blocking_guard.rs` 记录的那类事故（那个守卫只认固定的阻塞 token，不认 ureq）。
#[tauri::command]
pub(crate) async fn update_check(app: AppHandle) -> Result<UpdateState, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || check_with_busy(handle))
        .await
        .map_err(|error| format!("检查线程异常：{error}"))?
}

/// 下载：起线程立刻返回，进度走 update-state 事件
#[tauri::command]
pub(crate) fn update_download(app: AppHandle) -> Result<(), String> {
    let manager = app.state::<UpdateManager>();
    if manager.busy.swap(true, Ordering::SeqCst) {
        return Err("更新任务正在进行中，请稍候".to_string());
    }
    if manager.asset.lock().expect("update mutex poisoned").is_none() {
        manager.busy.store(false, Ordering::SeqCst);
        return Err("没有可下载的版本，请先检查更新".to_string());
    }
    let handle = app.clone();
    std::thread::spawn(move || download_worker(handle));
    Ok(())
}

/// 安装：过安全闸 → 线程里交接安装器 → 退出
#[tauri::command]
pub(crate) fn update_install(app: AppHandle) -> Result<(), String> {
    // blocking-ok: 命令体自身不做任何阻塞动作 —— 只 spawn 一个线程；真正的
    // `Command::new` 在 install_worker 里、跑在独立线程上。守卫按「同文件同步
    // 调用链」静态判定，看不到线程边界，这里的豁免是「线程已隔离」的显式声明。
    // 安全闸：任何进行中的动作都不允许覆盖安装（设计文档 §1 规则）
    if let Some(reason) = install_blocked_reason(&app) {
        return Err(reason);
    }
    let manager = app.state::<UpdateManager>();
    if manager.busy.swap(true, Ordering::SeqCst) {
        return Err("更新任务正在进行中，请稍候".to_string());
    }
    if manager.staged.lock().expect("update mutex poisoned").is_none() {
        manager.busy.store(false, Ordering::SeqCst);
        return Err("安装包还没下载好，请先下载".to_string());
    }
    let handle = app.clone();
    std::thread::spawn(move || install_worker(handle));
    Ok(())
}

/// 跳过此版本（只写设置，不改状态机）
#[tauri::command]
pub(crate) fn update_dismiss(app: AppHandle, version: String) -> Result<(), String> {
    let version = version.trim().trim_start_matches(['v', 'V']).to_string();
    update_settings_and_publish(
        &app,
        "检查更新",
        if version.is_empty() { "已取消跳过" } else { "已跳过此版本" },
        |settings| settings.update_skip_version = version.clone(),
        |_app| {},
    )?;
    set_phase(&app, |state| state.skipped = !version.is_empty());
    Ok(())
}

/// 自动检查开关（设置页）
#[tauri::command]
pub(crate) fn set_update_auto_check(app: AppHandle, enabled: bool) -> Result<(), String> {
    update_settings_and_publish(
        &app,
        "自动检查更新",
        if enabled { "已开启" } else { "已关闭" },
        |settings| settings.update_auto_check = enabled,
        |_app| {},
    )?;
    Ok(())
}

/// 下载源策略（auto / direct / ghfast / ghproxy）
#[tauri::command]
pub(crate) fn set_update_mirror(app: AppHandle, value: String) -> Result<(), String> {
    let value = match value.trim() {
        "" => UPDATE_MIRROR_AUTO.to_string(),
        other => other.to_string(),
    };
    update_settings_and_publish(
        &app,
        "更新下载源",
        &format!("已设为 {value}"),
        |settings| settings.update_mirror = value,
        |_app| {},
    )?;
    Ok(())
}

/// 安装前置条件：任何进行中的引导切换 / 电源操作 / 备份都不许装
fn install_blocked_reason(app: &AppHandle) -> Option<String> {
    let busy = busy_snapshot(app);
    let text = busy.describe();
    if !text.trim().is_empty() {
        return Some(format!(
            "当前有进行中的操作（{text}），装完会打断它。等它跑完再点安装。"
        ));
    }
    None
}

// ============================================================================
// 自动检查（后台线程）
// ============================================================================

/// 启动后台自动检查：延迟 30s 首查，之后每 12h 一次。
/// 失败静默（只有手动检查才把错误显示给用户 —— 自动通道坏了不该变成每日骚扰）。
pub(crate) fn spawn_auto_check(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(AUTO_CHECK_DELAY_SECS));
        loop {
            let settings = stored_settings(&app);
            if settings.update_auto_check {
                let manager = app.state::<UpdateManager>();
                if !manager.busy.swap(true, Ordering::SeqCst) {
                    match check_blocking(&app) {
                        Ok(state) => {
                            if state.phase == "available" && !state.skipped {
                                trace(&app, "update", "自动检查发现新版本");
                            }
                        }
                        Err(error) => {
                            trace(&app, "update", &format!("自动检查失败（静默）：{error}"));
                            set_phase(&app, |state| {
                                // 自动检查失败回到 idle：不把错误摆在设置页里
                                state.phase = "idle".to_string();
                                state.error.clear();
                            });
                        }
                    }
                    manager.busy.store(false, Ordering::SeqCst);
                }
            }
            std::thread::sleep(Duration::from_secs(AUTO_CHECK_PERIOD_SECS));
        }
    });
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_and_ordering() {
        assert_eq!(parse_version("v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("1"), None);
        assert_eq!(parse_version("v1.2.3-beta"), None);
        assert!(is_newer("v0.2.1", "0.2.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
        // 解析失败一律「无更新」，绝不猜
        assert!(!is_newer("latest", "0.2.0"));
        assert!(!is_newer("0.2.1", ""));
    }

    #[test]
    fn tag_from_location_reads_github_redirect() {
        assert_eq!(
            tag_from_location("https://github.com/lidiaoo/anyboot/releases/tag/v0.2.0"),
            Some("v0.2.0".to_string())
        );
        assert_eq!(
            tag_from_location("https://github.com/o/r/releases/tag/v1.0.0?x=1"),
            Some("v1.0.0".to_string())
        );
        assert_eq!(tag_from_location("https://github.com/o/r/releases"), None);
    }

    #[test]
    fn sums_parsing_matches_asset() {
        let hash_a = "a".repeat(64);
        let hash_b = "b".repeat(64);
        let text = format!("{hash_a}  AnyBoot_0.2.1_universal.dmg\n{hash_b}  *other.zip\n");
        assert_eq!(
            parse_sums(&text, "AnyBoot_0.2.1_universal.dmg"),
            Some(hash_a.clone())
        );
        // 带 * 的二进制标记也要认（sha256sum -b 的输出）
        assert_eq!(parse_sums(&text, "other.zip"), Some(hash_b));
        assert_eq!(parse_sums(&text, "missing.dmg"), None);
        // 长度不是 64 的哈希不算数（防止误把说明行当校验行）
        assert_eq!(parse_sums("deadbeef  a.dmg", "a.dmg"), None);
    }

    #[test]
    fn trusted_urls_only() {
        assert!(is_trusted_url(
            "https://github.com/lidiaoo/anyboot/releases/download/v0.2.1/x.dmg"
        ));
        assert!(is_trusted_url("https://objects.githubusercontent.com/x"));
        assert!(!is_trusted_url("http://github.com/x"));
        assert!(!is_trusted_url("https://evil.example.com/x"));
    }

    #[test]
    fn asset_candidates_are_versioned() {
        let names = asset_candidates("v0.2.1");
        assert!(names.iter().all(|name| name.contains("0.2.1")), "{names:?}");
        assert!(names.iter().all(|name| !name.contains("v0.2.1")), "{names:?}");
    }
}
