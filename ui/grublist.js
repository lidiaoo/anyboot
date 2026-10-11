// GRUB 启动项独立窗口：展示逻辑与主窗口内嵌列表共用 grub-list-view.js。
//
// 数据来源与主窗口完全一致：
// - 进场先 invoke("get_ui_state") 拿一帧完整快照；
// - 之后后端每次 publish_ui 都会 app.emit("ui-state-changed") 广播到**所有**窗口，
//   这里监听同一个事件即可，不需要单独的推送通道。
// 点条目切系统走同一个 switch_grub 命令；后端写完 grubenv 会 publish，
// 这里的列表随事件自动刷新（选中高亮、busy 置灰都跟主窗口同步）。
"use strict";

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

// 后端统一记 UTC ISO 串（…T…Z）。Webview 的时区 = 系统时区，用 toLocaleString
// 转成本地时间显示，与 Windows 任务栏 / macOS 菜单栏 / Linux 桌面时钟一致。
// 解析失败时原样返回，别把界面搞出 "Invalid Date"。
function formatLocalTime(iso) {
  if (!iso || typeof iso !== "string") return iso || "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString([], { hour12: false });
}

const listElement = document.querySelector("#grub-window-list");
const metaElement = document.querySelector("#grub-window-meta");
const statusElement = document.querySelector("#grub-window-status");
const activeElement = document.querySelector("#grub-window-active");

let busy = false;
let lastSelector = "";
// 最近一次快照里的主题：ui-alpha-preview 预览事件只带不透明度，
// 主题沿用当前值（主题变化走 ui-state-changed 的完整快照）。
let currentTheme = "dark";
// 与主窗口同步的强调色（accent-preview 预览 / 快照里的 accentColor）
let currentAccent = "#2f7fe0";

function resolvedTheme(theme) {
  if (theme === "system") {
    return window.matchMedia("(prefers-color-scheme: light)").matches
      ? "light"
      : "dark";
  }
  return theme === "light" ? "light" : "dark";
}

/// amount > 0 往白混、< 0 往黑混（与 app.js 里的 shadeHex 保持一致）
function shadeHex(hex, amount = 0) {
  const match = /^#?([0-9a-f]{6})$/i.exec(String(hex || "").trim());
  if (!match) return null;
  const value = Number.parseInt(match[1], 16);
  const channels = [(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff].map(
    (c) => Math.round(amount >= 0 ? c + (255 - c) * amount : c * (1 + amount)),
  );
  if (channels.some((c) => !Number.isFinite(c))) return null;
  return `#${channels.map((c) => c.toString(16).padStart(2, "0")).join("")}`;
}

/// 强调色落到 CSS 变量（按钮/链接派生），方向与主窗口一致：
/// 暗色主题悬停/链接提亮，亮色主题压暗。
function applyAccent(accent, theme) {
  const base = shadeHex(accent) || "#2f7fe0";
  const dark = resolvedTheme(theme) === "dark";
  const root = document.documentElement.style;
  root.setProperty("--accent", base);
  root.setProperty("--accent-hover", shadeHex(base, dark ? 0.08 : -0.12) || base);
  root.setProperty("--accent-link", shadeHex(base, dark ? 0.18 : -0.18) || base);
  root.setProperty("--accent-link-hover", shadeHex(base, dark ? 0.3 : -0.3) || base);
}

/// 主题与不透明度跟主窗口保持一致（同一份 CSS 变量）。
function applyAppearance(theme, percent, accent) {
  document.documentElement.dataset.theme = resolvedTheme(theme);
  const value = Number(percent);
  const clamped = Math.min(
    100,
    Math.max(0, Number.isFinite(value) ? value : 100),
  );
  document.documentElement.style.setProperty(
    "--ui-alpha",
    String(clamped / 100),
  );
  if (accent !== undefined) {
    currentAccent = shadeHex(accent) || currentAccent;
  }
  applyAccent(currentAccent, theme);
}

function renderMeta(grub) {
  const entries = grub.entries || [];
  const parts = [];
  if (grub.efiDir) {
    parts.push(`EFI 目录 ${grub.efiDir}`);
  }
  parts.push(`${entries.length} 个启动项`);
  if (grub.syncedAt) {
    parts.push(`同步于 ${formatLocalTime(grub.syncedAt)}`);
  }
  metaElement.textContent = parts.join(" · ");
}

function apply(state) {
  if (!state) {
    return;
  }
  const settings = state.settings || {};
  currentTheme = settings.theme || "dark";
  applyAppearance(currentTheme, settings.opacityPercent, settings.accentColor);
  const grub = state.grub || {};
  busy = Boolean(
    state.busy && Object.values(state.busy).some(Boolean),
  );
  renderMeta(grub);
  // 顶部横幅：当前选中的启动项（父级小字 › 子级大字）
  renderGrubActive(activeElement, grub);
  renderGrubEntries({
    container: listElement,
    grub,
    busy,
    onSwitch: switchTo,
  });
}

async function switchTo(selector) {
  if (!selector) {
    return;
  }
  lastSelector = selector;
  statusElement.textContent = "正在写入 grubenv…";
  statusElement.classList.remove("fail");
  try {
    await invoke("switch_grub", { selector });
    // 成功的回执由后端 publish 的快照带回来（grub.nextEntry 变了），
    // 这里只把中间态清掉。
    statusElement.textContent = "";
  } catch (error) {
    statusElement.textContent = String(error);
    statusElement.classList.add("fail");
  }
}

async function main() {
  try {
    apply(await invoke("get_ui_state"));
  } catch (error) {
    metaElement.textContent = `读取状态失败: ${error}`;
  }
  await listen("ui-state-changed", (event) => {
    // 正在切换的中间态别被事件覆盖：切换完成前 statusElement 保持原样
    apply(event.payload);
    if (lastSelector && !statusElement.classList.contains("fail")) {
      statusElement.textContent = "";
      lastSelector = "";
    }
  });
  // 主窗口拖不透明度滑条的实时预览（change 才写后端，拖动期间快照还是旧值）
  await listen("ui-alpha-preview", (event) => {
    applyAppearance(currentTheme, event.payload);
  });
  // 主窗口调色板选强调色的实时预览（change 才写后端）
  await listen("accent-preview", (event) => {
    currentAccent = shadeHex(event.payload) || currentAccent;
    applyAccent(currentAccent, currentTheme);
  });
  // 首帧渲染回执：后端在收到这个之前不会 show 窗口（修「闪一下才稳定」）
  try {
    await invoke("grub_list_window_ready");
  } catch (error) {
    void error; // 旧版后端没有这个命令，忽略
  }
}

main();
