<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { applyAccent, shadeHex } from "@/composables/useAccent";
import { useSettingsStore } from "@/stores/settings";
import { useBootAdvStore, type LoaderId } from "@/stores/bootAdv";
import { loaderMeta } from "@/pages/boot/themes/loaders";
import { call } from "@/api/tauri";
import BootList from "@/components/boot/BootList.vue";
import RefindSection from "@/components/boot/RefindSection.vue";
import GrubSection from "@/components/boot/GrubSection.vue";
import { simRows, grubRows, refindVariableRows } from "@/pages/boot/bootRows";
import type { EntryData } from "@/components/boot/types";
import BootSettings from "./BootSettings.vue";
import SettingsPanel from "@/components/boot/SettingsPanel.vue";
import SolPanel from "@/components/boot/SolPanel.vue";
import ThemeHost from "./themes/index.vue";
import type { LoaderInfo } from "./themes/types";

/**
 * 主窗（原 `ui/index.html`，1092 行）。
 *
 * ## 1:1 结构
 * ```
 * #dock-strip     贴边隐藏时的小白条
 * #hotzone        热区可视化层
 * .app-columns > main.pane.pane-main
 *   header > .header-row > h1 + .system-actions(6 个 .ico-btn) + #status
 *   nav.tabs#tabs > 4 × button.tab[data-tab]
 *   .tab-panel#tab-boot（系统引导）
 *     section#next-boot   .nb-head + #next-boot-name + .next-boot-rows
 *     section#refind      rEFInd 启动项（自动/手动两套）
 *     section#grub        GRUB 启动项（自动/手动两套）
 *     #boot-channels-mount 三引导器通道
 *     #privCard           root 助手卡片
 *   .tab-panel#tab-adv-boot / #tab-settings / #tab-sol
 * ```
 */

const ui = useUiStateStore();
const settings = useSettingsStore();
const adv = useBootAdvStore();

type TabId = "boot" | "adv-boot" | "settings" | "sol";

const TABS = [
  { id: "boot", label: "系统引导" },
  { id: "adv-boot", label: "引导高级设置" },
  { id: "settings", label: "基本设置" },
  { id: "sol", label: "SOL 远程唤醒" },
] as const;

const TAB_KEY = "anyboot.main.tab";

const currentTab = ref<TabId>(
  (() => {
    try {
      const saved = localStorage.getItem(TAB_KEY) as TabId | null;
      if (saved && TABS.some((t) => t.id === saved)) return saved;
    } catch {
      /* 忽略 */
    }
    return "boot" as TabId;
  })(),
);

watch(currentTab, (tab) => {
  try {
    localStorage.setItem(TAB_KEY, tab);
  } catch {
    /* 忽略 */
  }
});

/** 下次启动区：是否需要折叠按钮（旧版 #nb-toggle） */
const nextBootRowsOpen = ref(false);

/** 免密助手切换中的忙碌标志（旧版 helperPending） */
const helperBusy = ref(false);
/** 切换免密助手（旧版 toggleBootHelper → boot_helper {action}） */
async function toggleBootHelper(action: "install" | "uninstall") {
  if (helperBusy.value) return;
  helperBusy.value = true;
  try {
    const { call } = await import("@/api/tauri");
    await call("boot_helper", { action });
  } catch {
    /* 失败静默，状态由下次刷新带回 */
  } finally {
    helperBusy.value = false;
  }
}

/** 跟随主题切换（旧版 set_theme） */
function setTheme(theme: string) {
  void call("set_theme", { theme }).catch(() => {});
}

/** 页头主题按钮：暗 ↔ 亮 */
function toggleTheme() {
  const current = document.documentElement.classList.contains("dark") ? "dark" : "light";
  const next = current === "dark" ? "light" : "dark";
  const html = document.documentElement;
  html.setAttribute("data-theme", next);
  html.classList.toggle("light", next === "light");
  html.classList.toggle("dark", next === "dark");
  applyAccent(ui.settings?.accentColor, next);
  setTheme(next);
}

/** 菜单密度（旧版 applyStyleDensity）：body.dataset.bootStyle = compact|min */
function applyStyleDensity(): void {
  const st = adv.state.style;
  if (st === "full") delete document.body.dataset.bootStyle;
  else document.body.dataset.bootStyle = st;
}

/** 界面外观：主题 + 不透明度（旧版 applyAppearance） */
function applyAppearance(): void {
  const st = ui.settings as Record<string, unknown> | undefined;
  // 主题应用（旧版 applyAppearance：GUEST_UI_THEME || resolvedTheme(theme)）
  let guestUi = "";
  try {
    const v = (new URLSearchParams(location.search).get("abUi") || "").toLowerCase();
    if (v === "light" || v === "dark") guestUi = v;
  } catch { /* 忽略 */ }
  const rawTheme = String(st?.theme ?? "dark");
  const resolved = guestUi || (rawTheme === "system"
    ? (window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark")
    : (rawTheme === "light" ? "light" : "dark"));
  const html = document.documentElement;
  html.setAttribute("data-theme", resolved);
  html.classList.toggle("light", resolved === "light");
  html.classList.toggle("dark", resolved === "dark");
  applyAccent(ui.settings?.accentColor, resolved);
  const percent = Number(st?.opacityPercent ?? 90);
  const clamped = Math.min(100, Math.max(0, Number.isFinite(percent) ? percent : 90));
  document.documentElement.style.setProperty("--ui-alpha", String(clamped / 100));
  // 同步设置里的透明度滑条状态文字（旧版 #opacity-state）
  const el = document.getElementById("opacity-state");
  if (el) el.textContent = `${clamped}%（拖动即时预览）`;
  // 镜像到 localStorage 供下次首帧
  try {
    localStorage.setItem("anyboot.ui-alpha", String(clamped / 100));
  } catch {
    /* 忽略 */
  }
}

/** 贴边隐藏白条 + 热区（旧版 applyDockStrip，逐字对齐） */
function clampInt(v: unknown, min: number, max: number, fallback: number): number {
  const n = Math.round(Number(v));
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}
function applyDockStrip(): void {
  const body = document.body;
  body.classList.remove("dock-top", "dock-bottom", "dock-left", "dock-right");
  const st = ui.settings as Record<string, unknown> | undefined;
  if (!st) return;
  const thickness = clampInt(st.mainDockStripThickness, 2, 40, 6);
  const length = clampInt(st.mainDockStripLength, 10, 100, 100);
  const opacity = clampInt(st.mainDockStripOpacity, 10, 100, 100);
  const root = document.documentElement.style;
  root.setProperty("--dock-strip-thickness", `${thickness}px`);
  root.setProperty("--dock-strip-length", `${length}%`);
  root.setProperty("--dock-strip-opacity", String(opacity / 100));
  root.setProperty("--dock-strip-color", shadeHex(String(st.mainDockStripColor ?? "")) || "#6366f1");
  root.setProperty("--dock-strip-edge-gap", `${clampInt(st.mainDockStripEdgeGap, 0, 100, 0)}px`);
  root.setProperty("--hotzone-thickness", `${clampInt(st.mainHotzoneThickness, 1, 100, 40)}px`);
  root.setProperty("--hotzone-color", shadeHex(String(st.mainHotzoneColor ?? "")) || "#6366f1");
  root.setProperty("--hotzone-opacity", String(clampInt(st.mainHotzoneOpacity, 0, 100, 0) / 100));
  root.setProperty("--hotzone-border-color", shadeHex(String(st.mainHotzoneBorderColor ?? "")) || "#ff0000");
  root.setProperty("--hotzone-border-width", st.mainHotzoneBorder ? "1px" : "0px");
  // 贴边：body 加 dock-<edge> class（CSS 据此画白条方向）+ 上报白条矩形
  const state = ui.state as unknown as { docked?: boolean; dockEdge?: string } | null;
  if (state?.docked && state.dockEdge) {
    body.classList.add("dock-" + state.dockEdge);
    requestAnimationFrame(() => {
      const strip = document.getElementById("dock-strip");
      if (!strip) return;
      const rect = strip.getBoundingClientRect();
      void import("@/api/tauri").then(({ call }) =>
        call("report_dock_strip_rect", {
          x: rect.x, y: rect.y, width: rect.width, height: rect.height,
          viewportWidth: window.innerWidth, viewportHeight: window.innerHeight,
          dpr: window.devicePixelRatio,
        }).catch(() => {}),
      );
    });
  }
}

watch(() => ui.settings?.accentColor, () => {
  const curTheme = document.documentElement.classList.contains("light") ? "light" : "dark";
  applyAccent(ui.settings?.accentColor, curTheme);
});
// 贴边/热区设置变化或 docked 状态变化时应用
watch(() => ui.settings, () => { applyDockStrip(); applyAppearance(); }, { deep: true });
watch(() => adv.state.style, applyStyleDensity);
watch(() => (ui.state as unknown as { docked?: boolean; dockEdge?: string } | null)?.docked, applyDockStrip);

onMounted(async () => {
  await ui.start();
  // 强调色初始应用（旧版 applyAccent）
  const curTheme = document.documentElement.classList.contains("light") ? "light" : "dark";
  applyAccent(ui.settings?.accentColor, curTheme);
  // 贴边白条/热区初始应用（旧版 applyDockStrip）
  applyDockStrip();
  // 界面外观（透明度）初始应用（旧版 applyAppearance）
  applyAppearance();
  // 菜单密度初始应用（旧版 applyStyleDensity）
  applyStyleDensity();
  // 旧版启动时还会拉一次设置、探一下本地服务、取默认下载目录
  void call("get_settings").catch(() => {});
  void call("local_service_health").catch(() => {});
  void call("hub_download_dir_default").catch(() => {});
  void call("update_take_focus_pending").catch(() => {});

  // 旧版监听的事件（补齐）
  const { listen } = await import("@tauri-apps/api/event");
  unlisteners.push(
    await listen("update-state", () => {
      void call("update_state").catch(() => {});
    }),
  );
  unlisteners.push(
    await listen<{ theme?: string }>("theme-changed", (event) => {
      const theme = event.payload?.theme;
      if (!theme) return;
      const html = document.documentElement;
      html.setAttribute("data-theme", theme);
      html.classList.toggle("light", theme === "light");
      html.classList.toggle("dark", theme === "dark");
      applyAccent(ui.settings?.accentColor, theme === "light" ? "light" : "dark");
    }),
  );
  unlisteners.push(await listen("hub-focus-update", () => void focusWindowState()));;
  window.setTimeout(() => {
    const line =
      `[vue-boot] ready=${ui.ready ? 1 : 0} rev=${ui.revision} tab=${currentTab.value} ` +
      `refindMode=${settings.refindMode} refindSim=${ui.refindSim?.entries?.length ?? 0} ` +
      `grubMode=${settings.grubMode} channels=${ui.channels.length}`;
    void call<void>("hub_report_error", { source: "vue-boot", message: line, stack: "" }).catch(() => {});
  }, 3000);
});

const loaders = computed<LoaderInfo[]>(() => {
  const list: LoaderInfo[] = [];
  for (const id of adv.enabledLoaders) {
    const meta = loaderMeta(id);
    const channel = ui.channels.find((c) => c.id === id);
    const detected = id === "refind" || id === "grub" ? true : Boolean(channel?.detected);
    list.push({
      id,
      name: meta.name,
      state: !detected ? "off" : meta.perm === "nvram" ? "warn" : "ok",
      entries: new Array(countEntries(id)).fill(0),
      esp: id === "grub" ? ui.grub?.efiDir : undefined,
      perm: meta.perm,
      detected,
    });
  }
  return list;
});

function countEntries(id: LoaderId | string): number {
  if (id === "refind") {
    return settings.refindMode === "manual"
      ? (ui.status?.variables ?? []).filter((v) => v.exists).length
      : (ui.refindSim?.entries ?? []).filter((e) => !e.hidden).length;
  }
  if (id === "grub") {
    const source = settings.grubMode === "manual" ? ui.grub : (ui.linuxGrub ?? ui.grub);
    return source?.entries?.length ?? 0;
  }
  const ch = ui.channels.find((c) => c.id === id);
  return (ch?.menuPreview?.entries ?? ch?.entries ?? []).length;
}

/** 上次启动项（旧版 #next-boot-last；该 commit 未实现数据源，保持空） */
const nextBootLast = ref("");

/** 下次启动：当前默认项 */
const nextBootName = computed(() => {
  if (settings.refindMode !== "manual") {
    const hit = ui.refindSim?.entries?.find((e) => e.lastBoot);
    if (hit) return hit.title.trim();
  }
  const active = ui.status?.variables?.find((v) => v.active);
  return active?.displayName || "未知";
});

/** 下次启动：每行 = 一个引导器 + 它选中的条目 */
const nextBootRows = computed(() =>
  loaders.value.map((loader) => {
    let value = "—";
    if (loader.id === "refind") {
      value =
        settings.refindMode === "manual"
          ? (ui.status?.variables?.find((v) => v.active)?.displayName ?? "—")
          : (ui.refindSim?.entries?.find((e) => e.lastBoot)?.title.trim() ?? "—");
    } else if (loader.id === "grub") {
      const source = settings.grubMode === "manual" ? ui.grub : (ui.linuxGrub ?? ui.grub);
      value = source?.nextEntry || source?.savedEntry || "—";
    } else {
      const ch = ui.channels.find((c) => c.id === loader.id);
      value = ch?.currentDefault || ch?.lastBoot || "—";
    }
    return { id: loader.id, name: loader.name, value };
  }),
);

/** 底部提示条（旧版 #status-bar，所有操作回执都落这里） */
const statusBar = ref<{ icon: string; text: string; error: boolean }>({ icon: "", text: "", error: false });
/** 用户手动收起的文案（旧版 statusBarDismissed：同一文案不重复弹） */
let statusBarDismissed = "";

/** 旧版 actionMessage：失败优先，其次 lastAction.message */
function actionMessage(state: { status?: { error?: string | null }; lastAction?: { message?: string } | null } | null): string {
  if (!state) return "";
  if (state.status?.error) return state.status.error;
  if (state.lastAction && state.lastAction.message) return state.lastAction.message;
  return "";
}

/** 旧版 syncStatusBarFromState：快照带回执时更新，除非用户已收起同一条 */
function syncStatusBarFromState(): void {
  const text = actionMessage(ui.state as never);
  if (!text) return;
  if (text === statusBarDismissed || text === statusBar.value.text) return;
  const error = Boolean(ui.status?.error);
  statusBar.value = { icon: error ? "✕" : "✓", text, error };
}

/** 手动收起（旧版 status-bar-close） */
function dismissStatusBar(): void {
  statusBarDismissed = statusBar.value.text;
  statusBar.value = { icon: "", text: "", error: false };
}

// 快照变化时同步
watch(() => ui.state, syncStatusBarFromState);

/* ============ Linux 免密提权（旧版 index.html:985–1100 的 priv* 逻辑） ============ */
interface PrivStatus {
  platform: boolean;
  state: string;
  detail: string;
  sudoersExists: boolean;
  sudoersPath: string;
  user: string;
  helperExists: boolean;
  hasPkexec: boolean;
  hasSudo: boolean;
}
const privStatus = ref<PrivStatus | null>(null);
const privBusy = ref(false);
const privPwOpen = ref(false);
const privPw = ref("");
let privPending: "install" | "uninstall" | "" = "";

const privState = computed(() => privStatus.value?.detail ?? "正在读取状态…");
const privPaths = computed(() => {
  const s = privStatus.value;
  if (!s) return "";
  return (
    (s.sudoersExists ? `授权：${s.sudoersPath}${s.user ? " → " + s.user : ""}` : "授权：未写入") +
    " · " +
    (s.helperExists ? "助手已就位" : "助手未安装")
  );
});

async function privLoad() {
  try {
    privStatus.value = (await call<PrivStatus>("linux_priv_status")) ?? null;
  } catch {
    privStatus.value = null;
  }
}

async function privRun(action: "install" | "uninstall", password?: string) {
  if (privBusy.value) return;
  privBusy.value = true;
  try {
    const st = (await call<PrivStatus>(
      action === "install" ? "linux_priv_install" : "linux_priv_uninstall",
      { password: password || null },
    )) ?? null;
    privPwOpen.value = false;
    privPw.value = "";
    privPending = "";
    privStatus.value = st;
  } catch {
    /* 忽略 */
  } finally {
    privBusy.value = false;
  }
}

function privInstall() {
  void privRun("install");
}
function privRemove() {
  void privRun("uninstall");
}
function privPwGo() {
  if (privPending) void privRun(privPending, privPw.value);
}

onMounted(() => {
  void privLoad();
});
/** 事件解绑句柄 */
const unlisteners: (() => void)[] = [];

/** 主窗焦点状态（旧版 hub-focus-update 用来刷新标题栏） */
function focusWindowState() {
  void call("hub_focus_window", { label: "main" }).catch(() => {});
}

onUnmounted(() => {
  for (const off of unlisteners) off();
});

// 真实区块行（旧版 G 从真实 DOM 行派生；此处共享派生供区块与 G 主题同源）
const defaultEntrySelector = computed<string>(() => {
  if (settings.refindMode === "manual") {
    const hit = (ui.status?.variables ?? []).find((v) => v.active);
    return hit?.source ?? "";
  }
  const hit = (ui.refindSim?.entries ?? []).find((e) => e.lastBoot);
  return hit?.title?.trim() ?? "";
});
const bootRowsByLoader = computed<Record<string, EntryData[]>>(() => ({
  refind:
    settings.refindMode === "manual"
      ? refindVariableRows(ui.status?.variables)
      : simRows(ui.refindSim),
  grub: grubRows({
    grubMode: settings.grubMode,
    linuxGrubEntries: ui.linuxGrub?.entries,
    grubEntries: ui.grub?.entries,
    variables: ui.status?.variables,
  }),
  sdboot: channelRows("sdboot"),
  opencore: channelRows("opencore"),
  clover: channelRows("clover"),
}));
function channelRows(id: string): EntryData[] {
  const ch = ui.channels.find((c) => c.id === id);
  if (!ch) return [];
  const preview = ch.menuPreview?.entries ?? [];
  if (preview.length) {
    return preview.map((row) => ({ selector: row.spec, name: row.title, class: row.kind }));
  }
  return (ch.entries ?? []).map((row) => ({
    selector: String(row.id ?? row.title ?? ""),
    name: String(row.title ?? row.id ?? ""),
    class: String(row.kind ?? ""),
  }));
}

/** 旧版：非 K 主题整页 fill 链（body[data-ba-fill] + #tab-boot.ba-fill） */
function syncBodyBaFill(): void {
  if (adv.effectiveTheme !== "K") document.body.dataset.baFill = "1";
  else delete document.body.dataset.baFill;
}
watch(() => adv.effectiveTheme, syncBodyBaFill, { immediate: true });

// ── 布局窗口落宽（旧版 boot-adv.js ensureLayoutWidth）──────────────
// wide / lr = 窗口至少 932（两栏 460+12+460，后端「窄了才加宽」）；default 回 460。
// 只在本机真实窗口里做「还窄就补发」，远端（浏览器/手机看本机 UI）不判宽度，
// 否则会反复去改主机窗口。
let layoutWidthSent = "";
let widenTries = 0;
function ensureLayoutWidth(): void {
  const layout = adv.effectiveLayout;
  const inTauri = location.protocol === "tauri:" || location.hostname === "tauri.localhost";
  const narrow = inTauri && layout !== "default" && window.innerWidth > 0 && window.innerWidth < 900;
  if (layoutWidthSent === layout && !(narrow && widenTries < 3)) return;
  if (narrow) widenTries += 1;
  else widenTries = 0;
  layoutWidthSent = layout;
  void call("set_boot_layout", { mode: layout }).catch(() => {});
}
watch([() => adv.effectiveLayout, () => adv.effectiveTheme], ensureLayoutWidth, { immediate: true });
/** 旧版 busyLabel（app.js:753）：忙态优先级 power > switching > unmounting > scanning */
function busyLabel(busy: { power?: string | null; switching?: boolean; unmounting?: boolean; scanning?: boolean } | null | undefined): string {
  if (!busy) return "";
  if (busy.power) return `${busy.power}处理中…`;
  if (busy.switching) return "正在切换下次启动系统…";
  if (busy.unmounting) return "正在卸载临时挂载…";
  if (busy.scanning) return "正在扫描…";
  return "";
}
const statusText = computed(() => {
  if (!ui.ready) return "正在扫描 rEFInd 变量目录…";
  const raw = (ui.status?.message || "").trim();
  const duplicatesNextBoot = !raw || raw.startsWith("下次启动：");
  const label = busyLabel(ui.busy);
  return label || (duplicatesNextBoot ? "" : raw);
});
/** 旧版：statusElement.classList.toggle("hidden", !label && duplicatesNextBoot) */
const statusHidden = computed(() => {
  if (!ui.ready) return false;
  const raw = (ui.status?.message || "").trim();
  const duplicatesNextBoot = !raw || raw.startsWith("下次启动：");
  return !busyLabel(ui.busy) && duplicatesNextBoot;
});
</script>

<template>
  <!-- 贴边隐藏时的小白条 -->
  <div id="dock-strip" :hidden="!ui.state?.docked" />
  <div id="hotzone" />

  <div class="app-columns">
    <main class="pane pane-main">
      <header>
        <div class="header-row">
          <h1>AnyBoot</h1>
          <div class="system-actions">
            <button
              type="button"
              id="theme-btn"
              class="secondary ico-btn"
              aria-label="切换主题"
              title="切换主题"
              @click="toggleTheme"
            >
              <svg aria-hidden="true" class="i" focusable="false" viewBox="0 0 16 16">
                <circle cx="8" cy="8" r="6" />
                <path d="M8 2a6 6 0 0 1 0 12z" fill="currentColor" stroke="none" />
              </svg>
            </button>
            <button type="button" id="open-hub" class="secondary ico-btn" aria-label="远程管理" title="远程管理" @click="call('hub_open_window', {})">
              <svg aria-hidden="true" class="i" focusable="false" viewBox="0 0 16 16">
                <rect height="8.2" rx="1.4" width="11.6" x="2.2" y="3" />
                <path d="M5.4 6.2 7 7.6 5.4 9" />
                <path d="M8.4 9h2.4" />
              </svg>
            </button>
            <button type="button" id="power-reboot" class="secondary ico-btn" aria-label="重启系统" title="重启系统" @click="call('power_action', { action: 'reboot' })">
              <svg aria-hidden="true" class="i" focusable="false" viewBox="0 0 16 16">
                <path d="M12.8 8a4.8 4.8 0 1 1-1.5-3.5" />
                <path d="M13 3.1v3h-3" />
              </svg>
            </button>
            <button type="button" id="power-shutdown" class="secondary ico-btn" aria-label="关机" title="关机" @click="call('power_action', { action: 'shutdown' })">
              <svg aria-hidden="true" class="i" focusable="false" viewBox="0 0 16 16">
                <path d="M8 2.6v5" />
                <path d="M11.7 4.8a5 5 0 1 1-7.4 0" />
              </svg>
            </button>
            <button type="button" id="power-sleep" class="secondary ico-btn" aria-label="睡眠" title="睡眠" @click="call('power_action', { action: 'sleep' })">
              <svg aria-hidden="true" class="i" focusable="false" viewBox="0 0 16 16">
                <path d="M10.6 2.5a5.6 5.6 0 1 0 2.9 8.3 4.7 4.7 0 0 1-2.9-8.3Z" />
              </svg>
            </button>
            <button type="button" id="restart-app" class="secondary ico-btn" aria-label="重启程序" title="重启程序" @click="call('restart_app')">
              <svg aria-hidden="true" class="i" focusable="false" viewBox="0 0 16 16">
                <rect height="10.4" rx="2.2" width="10.4" x="2.8" y="2.8" />
                <path d="M9.6 6.4a2.3 2.3 0 1 0 .3 3.2" />
                <path d="M9.8 4.9v1.7H8.1" />
              </svg>
            </button>
            <button type="button" id="quit-app" class="secondary ico-btn" aria-label="退出程序" title="退出程序" @click="call('quit_app')">
              <svg aria-hidden="true" class="i" focusable="false" viewBox="0 0 16 16">
                <path d="M9.7 2.7h2a1.4 1.4 0 0 1 1.4 1.4v7.8a1.4 1.4 0 0 1-1.4 1.4h-2" />
                <path d="M3 8h5.5" />
                <path d="M6.3 5.8 4.1 8l2.2 2.2" />
              </svg>
            </button>
          </div>
        </div>
        <p id="status" :class="{ hidden: statusHidden }">{{ statusText }}</p>
      </header>

      <nav aria-label="界面栏目" class="tabs" id="tabs">
        <button
          v-for="tab in TABS"
          :key="tab.id"
          type="button"
          :class="['tab', { active: currentTab === tab.id }]"
          :data-tab="tab.id"
          @click="currentTab = tab.id"
        >
          {{ tab.label }}
        </button>
      </nav>

      <!-- 栏目一：系统引导 -->
      <div
        :class="[
          'tab-panel',
          { active: currentTab === 'boot', 'ba-arranged ba-fill': adv.effectiveTheme !== 'K' },
        ]"
        id="tab-boot"
      >
        <!-- 下次启动 -->
        <section v-if="adv.state.nextBoot !== 'off'" aria-label="下次启动" class="next-boot" id="next-boot">
          <div class="nb-head">
            <small>下次启动</small>
            <button
              type="button"
              id="nb-toggle"
              class="nb-toggle"
              :aria-expanded="nextBootRowsOpen"
              :hidden="!nextBootRows.length"
              @click="nextBootRowsOpen = !nextBootRowsOpen"
            >
              {{ nextBootRowsOpen ? "收起" : "展开" }}
            </button>
          </div>
          <strong id="next-boot-name">{{ nextBootName }}</strong>
          <small class="next-boot-last" id="next-boot-last" :hidden="!nextBootLast">上次启动 <b>{{ nextBootLast }}</b></small>
          <div
            v-show="(nextBootRowsOpen || adv.state.nextBoot === 'open') && nextBootRows.length"
            class="next-boot-rows"
            id="next-boot-rows"
          >
            <div v-for="row in nextBootRows" :key="row.id" class="nb-row">
              <span class="k">{{ row.name }}</span>
              <span class="v">{{ row.value }}</span>
            </div>
          </div>
        </section>

        <!--
          引导器区：**K = 不套用主题**时显示旧版完整卡片（rEFInd / GRUB / 三通道）；
          套用主题时由 ThemeHost 渲染。这与旧版 `bootAdvApplyToBootPage()` 的分支一致：
              if (effectiveTheme() === "K") applyKArrangement();
              else bootThemeStage(list);
        -->
        <ThemeHost
          :rows="bootRowsByLoader"
          :default-entry-selector="defaultEntrySelector"
          :theme="adv.effectiveTheme"
          :layout="adv.effectiveLayout"
          :loaders="loaders"
          :side="adv.state?.side"
          :active-id="loaders[0]?.id ?? ''"
          :ratio="adv.state?.ratio ?? 0.5"
          :on-select="(s: string) => call('switch_grub', { selector: s })"
          @update:ratio="adv.setRatio"
          @reorder="(order: string[]) => adv.setOrder(order as any)"
        >
          <!-- K 原生态：旧版的完整区块结构 -->
          <div v-if="adv.effectiveTheme !== 'K'" class="lg-group" data-loader-id="refind">
            <LoaderIcon size="sm" />
            <b>rEFInd</b>
            <span>· {{ bootRowsByLoader.refind.length }} 项</span>
          </div>
          <RefindSection data-loader-section="refind" :class="adv.effectiveTheme !== 'K' ? 'ba-theme-sec ba-split ba-g-card' : ''" />

          <hr aria-hidden="true" class="boot-divider" id="boot-divider" :style="adv.effectiveTheme !== 'K' ? { display: 'none' } : undefined" />

          <div v-if="adv.effectiveTheme !== 'K'" class="lg-group" data-loader-id="grub">
            <LoaderIcon size="sm" />
            <b>GRUB</b>
            <span>· {{ bootRowsByLoader.grub.length }} 项</span>
          </div>
          <GrubSection data-loader-section="grub" :class="adv.effectiveTheme !== 'K' ? 'ba-theme-sec ba-split ba-g-card' : ''" />

          <!-- 三引导器通道（旧版 #boot-channels-mount） -->
          <div id="boot-channels-mount">
            <template v-for="ch in ui.channels.filter((c) => c.detected)" :key="ch.id">
            <div v-if="adv.effectiveTheme !== 'K'" class="lg-group" :data-loader-id="ch.id">
              <LoaderIcon size="sm" />
              <b>{{ ch.label }}</b>
              <span>· {{ bootRowsByLoader[ch.id]?.length ?? 0 }} 项</span>
            </div>
            <section
              :data-loader-section="ch.id"
              class="grub"
            >
              <div class="grub-head">
                <div class="grub-title-row">
                  <span class="zone-tag">{{ ch.label }} 区</span>
                  <h2>{{ ch.label }} 启动项</h2>
                </div>
                <small class="grub-pane-status">{{ ch.chip === "ok" ? "已检测 · 免提权" : "已检测 · 需提权" }}</small>
              </div>
              <!-- 免密模式 helper（旧版 renderHelperLine） -->
              <div
                v-if="ch.id === 'opencore' && ui.state?.helper && ui.state.helper.supported"
                class="channel-helper"
                :data-state="ui.state.helper.installed ? (ui.state.helper.stale ? 'stale' : 'on') : 'off'"
              >
                <span class="channel-helper-label">免密模式</span>
                <small class="channel-helper-text">{{ ui.state.helper.text }}</small>
                <button
                  type="button"
                  class="channel-helper-action needs-elev"
                  :disabled="helperBusy"
                  :title="ui.state.helper.installed && ui.state.helper.stale ? '把 /Library 里那份旧助手换成当前版本（一次管理员密码）；启动项设置不受影响' : ui.state.helper.installed ? '删掉 /Library 里的 root 助手与信箱：之后「设为默认」恢复成每次输入一次密码' : '安装时弹一次管理员密码；之后由 root 助手直接写 OCBt*，不再询问。助手只在收到请求时被 launchd 唤起，不常驻、不开端口'"
                  @click="toggleBootHelper(ui.state.helper.installed && !ui.state.helper.stale ? 'uninstall' : 'install')"
                >{{ ui.state.helper.installed && ui.state.helper.stale ? '重装 ⤴' : ui.state.helper.installed ? '关闭' : '开启 ⤴' }}</button>
              </div>
              <div class="grub-list">
                <BootList :loader="ch.id" />
              </div>
            </section>
            </template>
            <div v-if="!ui.channels.some((c) => c.detected)" class="boot-channel-collapsed" id="boot-channel-collapsed">
              其他引导器
              <span class="boot-channel-collapsed-names" id="boot-channel-collapsed-names">
                {{ ui.channels.map((c) => c.label).join("、") }}（未检测到）
              </span>
            </div>
          </div>

          <!-- 免密提权卡（旧版 #privCard，逐字结构） -->
          <section :hidden="!privStatus?.platform" aria-label="免密提权" class="priv-card" id="privCard">
            <div class="priv-head">
              <h3>免密提权</h3>
              <span class="priv-badge" id="privBadge">{{ privStatus?.state === "installed" ? "已安装" : privStatus?.state === "stale" || privStatus?.state === "outdated" ? "需更新" : "未安装" }}</span>
            </div>
            <p class="priv-state" id="privState">{{ privState }}</p>
            <p class="priv-paths mono" id="privPaths">{{ privPaths }}</p>
            <div class="actions">
              <button type="button" id="privInstall" @click="privInstall">安装免密组件</button>
              <button type="button" class="secondary" id="privRemove" @click="privRemove">移除授权</button>
            </div>
            <div class="priv-pw" id="privPwBox" :hidden="!privPwOpen">
              <label for="privPw">本机登录密码（只在本次运行的内存里，用于 sudo 兜底）</label>
              <input id="privPw" v-model="privPw" type="password" autocomplete="off" spellcheck="false" placeholder="登录密码" />
              <div class="actions">
                <button type="button" id="privPwGo" @click="privPwGo">用密码继续</button>
                <button type="button" class="secondary" id="privPwCancel" @click="privPwOpen = false">取消</button>
              </div>
            </div>
            <p class="priv-note" id="privNote">{{ (privStatus?.detail || "") }}</p>
          </section>
        </ThemeHost>
      </div>

      <!-- 栏目二：引导高级设置 -->
      <div :class="['tab-panel', { active: currentTab === 'adv-boot' }]" id="tab-adv-boot">
        <div id="boot-adv-mount"><BootSettings /></div>
      </div>

      <!-- 栏目三：基本设置 -->
      <div :class="['tab-panel', { active: currentTab === 'settings' }]" id="tab-settings">
        <SettingsPanel />
      </div>

      <!-- 栏目四：SOL 远程唤醒 -->
      <div :class="['tab-panel', { active: currentTab === 'sol' }]" id="tab-sol">
        <SolPanel />
      </div>
    </main>
  </div>

  <!-- 窗口自己的底部提示条（旧版 #status-bar，与 .app-columns 平级） -->
  <div v-if="statusBar.text" id="status-bar" aria-live="polite" :class="['status-bar', statusBar.error ? 'error' : 'ok']" role="status">
    <span id="status-bar-icon" class="status-bar-icon" aria-hidden="true">{{ statusBar.icon }}</span>
    <span id="status-bar-text" class="status-bar-text">{{ statusBar.text }}</span>
    <button type="button" id="status-bar-close" class="status-bar-close" title="收起" @click="dismissStatusBar">×</button>
  </div>
</template>
