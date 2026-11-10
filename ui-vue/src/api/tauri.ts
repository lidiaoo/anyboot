/**
 * Tauri IPC 的类型化封装。
 *
 * 后端共 ~170 个命令；这里不做「每个命令一个函数」（维护成本太高、也容易漏），
 * 而是：
 *   ① 一个类型安全的通用 `call()` —— 调用方写明返回类型
 *   ② 高频 / 关键命令给具名快捷函数，避免命令名字符串散落各处
 *
 * 命令名必须与 `src-tauri/src/lib.rs` 的 `generate_handler![]` 一致；
 * Rust 侧有个测试（`frontend_uses_the_single_state_contract`）会校验前端源码里
 * 出现过这些命令名，所以阶段 5 迁移后要把它指向 `ui-vue/src`。
 */

import { invoke } from "@tauri-apps/api/core";

/** 是否跑在 Tauri 里（浏览器调试时为 false） */
export function isTauri(): boolean {
  return "__TAURI_INTERNALS__" in window || "__TAURI__" in window;
}

/**
 * 调一个 IPC 命令。
 *
 * @param command 命令名（与 Rust `generate_handler!` 里的一致）
 * @param args    参数对象；Rust 侧是 camelCase 反序列化
 */
export async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  // 非 Tauri（浏览器 / 手机 WebView / 内嵌 iframe）：走本机 HTTP 的 /api/ui-command
  // 桥接（旧 app.js 同一条通道）。此前没有此兜底 → 网页端打开 hub 页命令全报错 → 空数据。
  if (!isTauri()) {
    return await httpCall<T>(command, args);
  }
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    // 把失败写进 trace：macOS 上没有 devtools，这是唯一能事后排查的通道。
    // 只在 Tauri 里报，且**不吞异常** —— 调用方仍要自己 catch。
    void reportIpcFailure(command, args, error);
    throw error;
  }
}
/** 浏览器 / 网页端：POST /api/ui-command {cmd,args} → {ok,result}（沿用旧 app.js 语义） */
async function httpCall<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const token = (() => {
    try {
      return new URLSearchParams(location.search).get("token") || "";
    } catch {
      return "";
    }
  })();
  const resp = await fetch(`/api/ui-command?token=${encodeURIComponent(token)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ cmd: command, args: args ?? {} }),
  });
  const json = (await resp.json().catch(() => ({}))) as { ok?: boolean; result?: unknown; message?: string };
  if (!resp.ok || json.ok === false) {
    throw new Error(json.message || `命令失败：${command}`);
  }
  return json.result as T;
}

/** 失败命令的去重集合（避免刷屏：同一个命令只报一次） */
const reportedFailures = new Set<string>();

function reportIpcFailure(command: string, args: unknown, error: unknown): Promise<unknown> | undefined {
  if (!isTauri()) return undefined;
  if (reportedFailures.has(command)) return undefined;
  reportedFailures.add(command);
  const message =
    `[ipc-fail] ${command} args=${safeJson(args)} err=${String(error).slice(0, 300)}`;
  // 用底层的 invoke 直接上报，避免递归进本函数的 catch
  return invoke("hub_report_error", {
    source: "vue-ipc",
    message,
    stack: error instanceof Error ? (error.stack ?? "").slice(0, 800) : "",
  }).catch(() => undefined);
}

function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value ?? null).slice(0, 300);
  } catch {
    return "(unserializable)";
  }
}

// ---------------------------------------------------------------------------
// 快照
// ---------------------------------------------------------------------------

/** 取当前完整快照（首次挂载时用；之后靠 `ui-state-changed` 事件推送） */
export const getUiState = <T>() => call<T>("get_ui_state");

// ---------------------------------------------------------------------------
// 引导器读取方式（自动 / 手动）
// ---------------------------------------------------------------------------

/** `loader` 是 "refind" / "grub"；`mode` 是 "auto" / "manual" */
export const setLoaderMode = (loader: "refind" | "grub", mode: string) =>
  call<void>("set_loader_mode", { loader, mode });

/** 重新扫描 / 同步 GRUB */
export const refresh = () => call<unknown>("refresh");

// ---------------------------------------------------------------------------
// Linux 分区 GRUB（自动读取链路）
// ---------------------------------------------------------------------------

export const linuxGrubScan = (target: string | null = null) =>
  call<unknown>("linux_grub_scan", { target });

export const linuxGrubPick = (dev: string) => call<unknown>("linux_grub_pick", { dev });

export const linuxGrubSwitch = (payload: Record<string, unknown>) =>
  call<unknown>("linux_grub_switch", payload);

export const linuxGrubHelperReady = () => call<boolean>("linux_grub_helper_ready");

export const linuxPrivStatus = () => call<unknown>("linux_priv_status");

// ---------------------------------------------------------------------------
// 三引导器通道
// ---------------------------------------------------------------------------

export const switchBootChannel = (payload: Record<string, unknown>) =>
  call<unknown>("switch_boot_channel", payload);

// ---------------------------------------------------------------------------
// GRUB ESP 配置
// ---------------------------------------------------------------------------

export const switchGrub = (selector: string) => call<unknown>("switch_grub", { selector });
export const detectEsp = (value: string) => call<unknown>("detect_esp", { value });
export const setGrubMount = (keep: boolean) => call<void>("set_grub_mount", { keep });
export const pickGrubDir = () => call<string>("pick_grub_dir");

// ---------------------------------------------------------------------------
// HTTP 服务
// ---------------------------------------------------------------------------

export const retryHttp = () => call<unknown>("retry_http");
export const setHttpToken = (token: string) => call<void>("set_http_token", { token });
export const regenerateHttpToken = () => call<string>("regenerate_http_token");
export const revealHttpToken = () => call<string>("reveal_http_token");
export const httpAccessHint = () => call<unknown>("http_access_hint");
export const killPortOccupant = () => call<unknown>("kill_port_occupant");

// ---------------------------------------------------------------------------
// 设置类命令（设置页用；名字被前端契约测试校验，别改）
// ---------------------------------------------------------------------------

/** 设定 rEFInd vars 目录 */
export const setVarsDir = (dir: string) => call<void>("set_vars_dir", { dir });
/** 切换下次启动的系统（rEFInd 的 PreviousBoot） */
export const switchSystem = (system: string) => call<void>("switch_system", { system });
/** 卸载临时挂载 */
export const unmountTemporary = () => call<void>("unmount_temporary");
/** 开机自启 */
export const setAutostart = (on: boolean) => call<void>("set_autostart", { enabled: on });
/** 启动时收进托盘 */
export const setStartInTray = (on: boolean) => call<void>("set_start_in_tray", { enabled: on });
/** 启动时使用管理员扫描（开启后启动主动扫所有引导器） */
export const setStartupScanWithAdmin = (enabled: boolean) =>
  call<void>("set_startup_scan_with_admin", { enabled });
/** Linux ext4 分区扫描策略：raw(仅裸读,默认) / prefer_mount(优先挂载) / mount_only(仅挂载) */
export const setLinuxExt4ScanMode = (mode: "raw" | "prefer_mount" | "mount_only") =>
  call<void>("set_linux_ext4_scan_mode", { mode });
/** 免密助手提醒的取消提醒到期时间（epoch 秒，0=清除） */
export const setHelperReminderSnooze = (until: number) =>
  call<void>("set_helper_reminder_snooze", { until });
/** 启动时的管理员扫描：触发一次全引导器扫描（allowPrompt 才允许弹系统授权框） */
export const runStartupAdminScan = (allowPrompt: boolean) =>
  call<void>("run_startup_admin_scan", { allowPrompt });
/** 从 Dock / 任务栏隐藏 */
export const setHideFromDockTaskbar = (on: boolean) =>
  call<void>("set_hide_from_dock_taskbar", { enabled: on });
/** 关闭窗口的行为：tray 收进托盘 / quit 退出 */
export const setCloseAction = (action: "tray" | "quit") =>
  call<void>("set_close_action", { action });
/** 电源动作：reboot / shutdown / sleep */
export const powerAction = (action: "reboot" | "shutdown" | "sleep") =>
  call<void>("power_action", { action });

// ---------------------------------------------------------------------------
// 应用 / 窗口
// ---------------------------------------------------------------------------

export const quitApp = () => call<void>("quit_app");
export const restartApp = () => call<void>("restart_app");
export const openWebPage = (url: string) => call<void>("open_web_page", { url });
export const openConfigDir = () => call<void>("open_config_dir");
export const openVarsDir = () => call<void>("open_vars_dir");
export const openGrubDir = () => call<void>("open_grub_dir");

// ---------------------------------------------------------------------------
// hub（服务器管理）
// ---------------------------------------------------------------------------

export const hubListServers = <T>() => call<T>("hub_list_servers");
export const hubSaveServer = (payload: Record<string, unknown>) =>
  call<unknown>("hub_save_server", { server: payload });
export const hubDeleteServer = (id: string) => call<void>("hub_delete_server", { id });
export const hubTestServer = (payload: Record<string, unknown>) =>
  call<unknown>("hub_test_server", { server: payload });
export const hubOpenWindow = (label: string, url?: string) =>
  call<void>("hub_open_window", { label, url });

// ---------------------------------------------------------------------------
// 诊断
// ---------------------------------------------------------------------------

export const logClientError = (message: string, stack = "") =>
  call<void>("sol_log_client_error", { message, stack });
