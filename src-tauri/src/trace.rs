//! trace 日志与时间工具

use super::*;

/// 阶段 0 的取证日志：只追加、不参与任何逻辑判断，写在应用配置目录。
/// 里面**永不写入** httpToken（只记前 8 位指纹）。
pub(crate) const TRACE_FILE: &str = "trace.log";

/// 是否记 trace。**注意**：会锁 Settings 互斥量，所以禁止在已持有该锁的
/// 代码路径里调用 trace()（会死锁）。
///
/// 启动早期（设置文件还没读进内存时）只有环境变量生效，
/// 想抓启动阶段的日志请用 `ANYBOOT_TRACE=1`。
pub(crate) fn trace_enabled(app: &AppHandle) -> bool {
    let from_env = env_flag_from(std::env::var("ANYBOOT_TRACE").ok().as_deref());
    from_env.unwrap_or_else(|| stored_settings(app).debug_trace)
}

pub(crate) fn trace_file_path(app: &AppHandle) -> Option<PathBuf> {
    config_dir(app)
        .ok()
        .map(|directory| directory.join(TRACE_FILE))
}

pub(crate) fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// 无依赖的 UTC 时间戳（本地时区需要平台 API，这里统一记 UTC，便于三端互相比对）。
pub(crate) fn utc_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Howard Hinnant 的 civil_from_days：1970-01-01 起的天数 → (年, 月, 日)。
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}
