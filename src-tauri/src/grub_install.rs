//! grub 安装/卸载脚本（mkconfig、09_anyboot）

use super::*;

/// Linux 侧数据收集脚本（把 grubenv 迁到 ESP），装到 `/etc/grub.d/09_anyboot`。
///
/// 内容与 `linux/09_anyboot` 一致，只有 `@@ESP_UUID@@` 是占位符 —— 安装时替换成
/// 用户填的 fs-uuid，其余一字不改（用户定案 1）。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) const GRUB_SCRIPT_TEMPLATE: &str = r##"#!/bin/sh
exec tail -n +3 "$0"
# ============================================================================
#  AnyBoot · /etc/grub.d/09_anyboot
#  安装位置： /etc/grub.d/09_anyboot        权限： 0755（必须可执行）
#
#  ★ 开头两行 `#!/bin/sh` 与 `exec tail -n +3 "$0"` 是 shell 包装。
#    grub-mkconfig 会「执行」本文件并读取它的 stdout；包装行负责把下面这段
#    GRUB 脚本原样打印出去。删掉它们，grub-mkconfig 就会拿 sh 去跑 GRUB 命令，
#    于是报 `search: 未找到命令` / `未预期的记号 "("`。**不要删！**
#
#  作用：把 GRUB 的"环境块"(grubenv) 从 ext4 的 /boot/grub/ 迁到 ESP(FAT32)：
#          <ESP>/EFI/grubenv
#  这样 Windows / macOS 无需 ext4 支持、无需提权，即可读写"上次/下次启动项"。
#  AnyBoot 同步时也会把 grub.cfg 副本放到 <ESP>/EFI/grub.cfg（同一个父目录）。
#
#  ── 本机取值（安装时由 AnyBoot 填入）──────────────────────────────────────
#          ESP 的 fs-uuid  = @@ESP_UUID@@
#          ESP 上的环境块  = /EFI/grubenv
#
#  排序：09 —— 在 00_header 之后、10_linux 之前。GRUB 在渲染菜单前会把整份
#    grub.cfg 执行一遍，所以这里重设 default、重定义 savedefault 会覆盖
#    00_header 里指向 ext4 的旧逻辑（后写入者胜出）。
#
#  模块依赖：search / load_env / save_env 已内置在 grubx64.efi 里；fat 未内置
#    （生成的 grub.cfg 要到 40_custom 那段才 insmod fat）→ 读 ESP 前必须显式
#    insmod fat，否则 search --fs-uuid 探测不到 ESP。
# ============================================================================

insmod part_gpt
insmod part_msdos
insmod fat

if search --no-floppy --fs-uuid --set=anyboot_esp @@ESP_UUID@@ ; then
  set anyboot_env="($anyboot_esp)/EFI/grubenv"
  export anyboot_esp
  export anyboot_env

  # 1) 从 ESP 上的环境块读取状态（saved_entry / next_entry / 任意自定义键）
  if [ -s "${anyboot_env}" ] ; then
    load_env -f "${anyboot_env}"
  fi

  # 2) 用 ESP 的状态决定 default（覆盖 00_header 基于内建 grubenv 的结论）。
  #    若 00_header 已取到一次性项（Linux 侧 grub-reboot / grub-editenv），
  #    它会置 boot_once=true —— 此时不再覆盖，尊重那一次的选择。
  if [ -z "${boot_once}" ] ; then
    if [ "${next_entry}" ] ; then
      set default="${next_entry}"
      set next_entry=
      save_env -f "${anyboot_env}" next_entry
      set boot_once=true
    elif [ "${saved_entry}" ] ; then
      set default="${saved_entry}"
    fi
  fi
fi

# 3) 覆盖 00_header 的 savedefault：把"记住上次选择"写回 ESP。
#    定义在最外层（不嵌在 if 里），ESP 若找不到会自动降级回内建 grubenv。
function savedefault {
  if [ -z "${boot_once}" ] ; then
    saved_entry="${chosen}"
    if [ "${anyboot_env}" ] ; then
      save_env -f "${anyboot_env}" saved_entry
    else
      save_env saved_entry
    fi
  fi
}
"##;

#[cfg(target_os = "linux")]
pub(crate) fn binary_path(program: &str) -> Option<PathBuf> {
    ["/usr/bin", "/usr/sbin", "/bin", "/sbin"]
        .iter()
        .map(|dir| Path::new(dir).join(program))
        .find(|path| path.is_file())
}

/// 重新生成 grub.cfg：发行版之间命令名与输出路径都不一样（用户定案 8）。
#[cfg(target_os = "linux")]
pub(crate) fn grub_run_mkconfig(log: &mut Vec<String>) -> Result<(), String> {
    let candidates = [
        ("grub-mkconfig", "/boot/grub/grub.cfg"),
        ("grub2-mkconfig", "/boot/grub2/grub.cfg"),
    ];
    let mut last_error = "没找到 grub-mkconfig / grub2-mkconfig".to_string();
    for (program, output) in candidates {
        if binary_path(program).is_none() {
            script_log_push(log, &format!("跳过 {program}：系统里没有这个命令"));
            continue;
        }
        script_log_push(
            log,
            &format!("运行 {program} -o {output}（非 root 会经 pkexec 提权）"),
        );
        let result = mount_command(program).args(["-o", output]).output();
        match &result {
            Ok(result) if result.status.success() => {
                script_log_output(log, program, result);
                script_log_push(log, &format!("→ 已重新生成 {output}"));
                return Ok(());
            }
            Ok(result) => {
                script_log_output(log, program, result);
                last_error = format!(
                    "{program} 失败: {}",
                    String::from_utf8_lossy(&result.stderr).trim()
                )
            }
            Err(error) => {
                script_log_error(log, program, error);
                last_error = format!("无法执行 {program}: {error}");
            }
        }
    }
    Err(last_error)
}

/// Linux：按分区 UUID 查那块分区的 fs-uuid（lsblk 的 UUID 列，udev 数据库，
/// 普通用户可读）。GRUB 的 `search --fs-uuid` 只认 fs-uuid，所以 09_anyboot
/// 写盘前要从识别键（分区 UUID）现场解析一次。
#[cfg(target_os = "linux")]
pub(crate) fn linux_fsuuid_of_partuuid(partuuid: &str) -> Option<String> {
    let output = command("lsblk")
        .args(["--json", "--output", "UUID,PARTUUID"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let devices: LsblkOutput = serde_json::from_slice(&output.stdout).ok()?;
    devices
        .blockdevices
        .into_iter()
        .find(|device| {
            device
                .partuuid
                .as_deref()
                .is_some_and(|value| grub::uuid_matches(value, partuuid))
        })?
        .uuid
        .filter(|value| grub::uuid_digits(value).is_some())
}

#[cfg(target_os = "linux")]
pub(crate) fn grub_install_script(app: &AppHandle, log: &mut Vec<String>) -> Result<String, String> {
    let settings = stored_settings(app);
    let Some(partuuid) = grub::partuuid_digits(&settings.grub_efi_partuuid) else {
        script_log_push(log, "缺少 ESP 的分区 UUID，安装中止");
        return Err("请先填写并保存 ESP 的分区 UUID".to_string());
    };
    let Some(fsuuid) = linux_fsuuid_of_partuuid(&partuuid) else {
        script_log_push(log, "从分区 UUID 解析不出 fs-uuid（lsblk 没报这块分区的 UUID），安装中止");
        return Err(
            "从分区 UUID 解析不出 fs-uuid —— GRUB 的 search --fs-uuid 只认 fs-uuid；\
             确认这块分区还在（lsblk 里 PARTUUID 对得上且 UUID 列有值）"
                .to_string(),
        );
    };
    let content = grub_script_content(grub_template(&settings), &grub::uuid_pretty(&fsuuid));
    script_log_push(
        log,
        &format!(
            "准备安装 {GRUB_SCRIPT_PATH}（把 @@ESP_UUID@@ 换成 {}，\
             由分区 UUID {} 解析而来）",
            grub::uuid_pretty(&fsuuid),
            grub::uuid_pretty(&partuuid)
        ),
    );

    // 用 install(1) 落盘：/etc/grub.d 需要 root，非 root 时 mount_command 会套 pkexec
    let staging = std::env::temp_dir().join("anyboot-09_anyboot");
    if let Err(error) = fs::write(&staging, content.as_bytes()) {
        script_log_push(log, &format!("暂存脚本失败: {error}"));
        return Err(format!("暂存脚本失败: {error}"));
    }
    script_log_push(log, &format!("写入临时文件 {}", staging.display()));
    script_log_push(
        log,
        &format!("运行 install -m 755 <暂存文件> {GRUB_SCRIPT_PATH}（非 root 会经 pkexec 提权）"),
    );
    let result = mount_command("install")
        .args(["-m", "755"])
        .arg(&staging)
        .arg(GRUB_SCRIPT_PATH)
        .output();
    match &result {
        Ok(output) => script_log_output(log, "install", output),
        Err(error) => script_log_error(log, "install", error),
    }
    let _ = fs::remove_file(&staging);
    match result {
        Ok(output) if output.status.success() => {
            script_log_push(log, &format!("→ 已写入 {GRUB_SCRIPT_PATH}"));
        }
        Ok(_) => {
            return Err("安装 09_anyboot 失败：见上方执行过程".to_string());
        }
        Err(error) => return Err(format!("无法执行 install: {error}")),
    }
    grub_run_mkconfig(log)?;
    Ok(format!(
        "已安装 {GRUB_SCRIPT_PATH} 并重新生成 grub.cfg（fs-uuid = {}）",
        grub::uuid_pretty(&fsuuid)
    ))
}

#[cfg(target_os = "linux")]
pub(crate) fn grub_uninstall_script(log: &mut Vec<String>) -> Result<String, String> {
    let mut removed = false;
    if Path::new(GRUB_SCRIPT_PATH).exists() {
        script_log_push(
            log,
            &format!("运行 rm -f {GRUB_SCRIPT_PATH}（非 root 会经 pkexec 提权）"),
        );
        let output = mount_command("rm").args(["-f", GRUB_SCRIPT_PATH]).output();
        match &output {
            Ok(result) => script_log_output(log, "rm", result),
            Err(error) => script_log_error(log, "rm", error),
        }
        let output = output.map_err(|error| format!("无法执行 rm: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "删除 09_anyboot 失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        script_log_push(log, &format!("→ 已删除 {GRUB_SCRIPT_PATH}"));
        removed = true;
    } else {
        script_log_push(log, &format!("{GRUB_SCRIPT_PATH} 不存在，跳过删除"));
    }
    grub_run_mkconfig(log)?;
    Ok(if removed {
        format!("已删除 {GRUB_SCRIPT_PATH} 并重新生成 grub.cfg")
    } else {
        "09_anyboot 本来就没装；已重新生成 grub.cfg".to_string()
    })
}

/// 非 Linux 上也保留同样的签名（`log` 只用来记一句「不支持」）。
#[cfg(not(target_os = "linux"))]
pub(crate) fn grub_install_script(_app: &AppHandle, log: &mut Vec<String>) -> Result<String, String> {
    script_log_push(log, "安装 09_anyboot 只能在 Linux 上进行");
    Err("安装 09_anyboot 只能在 Linux 上进行".to_string())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn grub_uninstall_script(log: &mut Vec<String>) -> Result<String, String> {
    script_log_push(log, "卸载 09_anyboot 只能在 Linux 上进行");
    Err("卸载 09_anyboot 只能在 Linux 上进行".to_string())
}
