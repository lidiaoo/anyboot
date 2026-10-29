//! root 助手的执行器：真正「碰设备」的地方都在这里。
//!
//! 单独一个文件是刻意的分层：
//! - [`crate::helper_proto`] 只做协议（收请求 / 校验 / 回执），**不碰任何设备**，
//!   所以能在非 root、非 macOS 下把校验逻辑完整单测掉；
//! - 这里只在 root 助手进程里被调用，一行单测都跑不到（要真设备），
//!   于是它要尽可能薄 —— 每个方法都是一次转发。
//!
//! 加新能力时先想清楚：**能不能不在这里加**。这里每多一个方法，
//! 就多一条「任何用户进程都能让 root 替它做事」的通道。

use crate::helper_proto::Executor;

/// 生产执行器。
pub struct System;

impl Executor for System {
    fn nvram(&self, ops: &[(String, String, Vec<u8>)]) -> Result<String, String> {
        apply_nvram(ops)
    }

    fn linux_probe(&self, esp: &str) -> Result<Vec<u8>, String> {
        let payload = crate::channels::linux_grub::probe_raw(esp)?;
        serde_json::to_vec(&payload).map_err(|error| format!("探测结果序列化失败：{error}"))
    }

    fn linux_write_env(&self, dev: &str, rel: &str, block: &[u8]) -> Result<String, String> {
        crate::channels::linux_grub::write_env_block(dev, rel, block)
    }
}

/// 逐条调 `nvram`。
///
/// 一次请求里可能有多条（`OCBtOrder` + 一条 `OCBt####`），顺序由调用方决定：
/// 先把单项写好、最后写 Order，中途失败时启动顺序表还是旧值，不会出现
/// 「Order 指向一条还没写成功的项」。
#[cfg(target_os = "macos")]
fn apply_nvram(ops: &[(String, String, Vec<u8>)]) -> Result<String, String> {
    // 测试用：把信箱指到临时目录时允许 dry-run，这样成功路径也能在非 root 下验证。
    // 生产安装的 launchd 作业不设 ANYBOOT_HELPER_SPOOL，这条分支进不来。
    let dry_run = std::env::var(crate::helper_proto::SPOOL_ENV).is_ok()
        && std::env::var("ANYBOOT_HELPER_DRY_RUN").as_deref() == Ok("1");

    for (name, guid, value) in ops {
        if dry_run {
            continue;
        }
        let arg = nvram_arg(name, guid, value);
        let output = std::process::Command::new("/usr/sbin/nvram")
            .arg(&arg)
            .output()
            .map_err(|error| format!("无法执行 nvram: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!("写 {name} 失败（nvram 退出码非 0）")
            } else {
                format!("写 {name} 失败：{stderr}")
            });
        }
    }
    Ok(if dry_run {
        format!("已处理 {} 条变量（dry-run，未真写）", ops.len())
    } else {
        format!("已写 {} 条变量：{}", ops.len(), join_names(ops))
    })
}

#[cfg(not(target_os = "macos"))]
fn apply_nvram(_ops: &[(String, String, Vec<u8>)]) -> Result<String, String> {
    Err("这个助手只在 macOS 上使用".to_string())
}

fn join_names(ops: &[(String, String, Vec<u8>)]) -> String {
    ops.iter()
        .map(|(name, _, _)| name.as_str())
        .collect::<Vec<_>>()
        .join("、")
}

/// `nvram` 写入参数的唯一正确形态：`guid:name=%XX%XX…` **一个**参数。
///
/// 拆成两个参数时 `nvram` 会把它当「读取 + 设置空名变量」：退出码 0、什么都没写。
/// （这条坑在 `channels::nvram` 里已经踩过一次，这里必须用同一套拼法。）
#[cfg(target_os = "macos")]
fn nvram_arg(name: &str, guid: &str, value: &[u8]) -> String {
    let mut encoded = String::with_capacity(value.len() * 3);
    for byte in value {
        encoded.push_str(&format!("%{byte:02X}"));
    }
    format!("{guid}:{name}={encoded}")
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn nvram_arg_is_a_single_argument() {
        // 这条断言是防回归的：拆成两个参数会「退出码 0 但什么都没写」
        assert_eq!(
            super::nvram_arg(
                "OCBtOrder",
                crate::helper_proto::OC_GUID,
                &[0x82, 0x00, 0x81, 0x00]
            ),
            "4D1FDA02-38C7-4A6A-9CC6-4BCCA8B30102:OCBtOrder=%82%00%81%00"
        );
        assert!(!super::nvram_arg("OCBtOrder", crate::helper_proto::OC_GUID, &[0x82]).contains(' '));
    }

    #[test]
    fn join_names_reads_like_a_sentence() {
        let ops = vec![
            ("OCBt0082".to_string(), String::new(), Vec::new()),
            ("OCBtOrder".to_string(), String::new(), Vec::new()),
        ];
        assert_eq!(super::join_names(&ops), "OCBt0082、OCBtOrder");
    }
}
