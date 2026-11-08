<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { applyAccent, shadeHex } from "@/composables/useAccent";
import { useSettingsStore } from "@/stores/settings";
import { useBootAdvStore, type LoaderId } from "@/stores/bootAdv";
import { loaderMeta } from "@/pages/boot/themes/loaders";
import { call } from "@/api/tauri";
import { runStartupAdminScan, isTauri } from "@/api/tauri";
import HelperPrompt from "@/components/boot/HelperPrompt.vue";
import TokenPrompt from "@/components/boot/TokenPrompt.vue";
import BootList from "@/components/boot/BootList.vue";
import LoaderSettings from "@/components/boot/LoaderSettings.vue";
import { simRows, grubRows, refindVariableRows } from "@/pages/boot/bootRows";
import type { EntryData } from "@/components/boot/types";
import BootSettings from "./BootSettings.vue";
import SettingsPanel from "@/components/boot/SettingsPanel.vue";
import SolPanel from "@/components/boot/SolPanel.vue";
import ThemeHost from "./themes/index.vue";
import type { LoaderInfo } from "./themes/types";

import { useToastStore } from "@/stores/toast";
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

/** 启动静默扫 Linux grub 的重试标志（旧版 grubAutoBootRetried） */

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
  // 引导节目录用**本机后端**存的编排（visible/order/side/layout）覆盖本地 —— 跨端一致，
  // 移动端取消勾选后重开也不会又勾上（后端有值才覆盖，空则保持 localStorage）。
  adv.hydrateFromBackend((ui.settings as { bootAdvJson?: string } | null)?.bootAdvJson);
  // 启动扫描策略（用户定案修正 2026-11-07）：
  //   **勾选「启动时使用管理员扫描」= 启动一定用管理员权限扫描所有引导器**
  //   （GRUB 自动 + rEFInd 内核）。没免密就弹系统密码授权框扫；取消勾选才不用 root 扫。
  //   · 免密就绪 → 静默扫（Silent，绝不弹系统框）。
  //   · 免密未就绪 → 先弹【免密建议窗（带「取消提醒」）】引导开免密；
  //        - 用户开免密 → 装好后静默扫（不再弹系统框）；
  //        - 用户取消提醒 / 暂不 → 关闭建议窗后，**照常弹系统密码授权框扫**
  //          （因为勾了管理员扫描，必须扫）。等建议窗操作完毕再决定，绝不重复弹。
  //   · 免密未就绪且处于「取消提醒」期 → 不弹建议窗，**直接弹系统密码框扫**。
  void (async () => {
    try {
      const s = ui.settings;
      if (!s || s.startupScanWithAdmin === false) return; // 取消勾选：不用 root 扫
      if (ui.linuxGrub?.entries?.length) return; // 已有数据不再扫
      const h = ui.state?.helper;
      const helperReady = Boolean(h?.installed && !h?.stale);
      if (helperReady) {
        await runStartupAdminScan(false).catch(() => {}); // 免密就绪：静默扫
        return;
      }
      const now = Math.floor(Date.now() / 1000);
      const snoozed = Number(s.helperReminderSnoozeUntil ?? 0) > now;
      if (snoozed) {
        // 处于「取消提醒」期：不弹建议窗，直接走系统密码框做管理员扫描
        await runStartupAdminScan(true).catch(() => {});
        return;
      }
      // 免密建议弹窗只在**桌面端(Tauri)**弹：移动端(远程/浏览器)不弹，也不需要管理免密助手。
      if (!isTauri()) {
        // 移动端：不做 root 扫描（管理员扫描只在桌面本机跑），直接返回。
        return;
      }
      // 免密未就绪、未取消提醒：先弹免密建议窗（含取消提醒）；用户操作完再决定扫描方式
      helperPromptVisible.value = true;
    } catch { /* 忽略 */ }
  })();
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

/** 每引导器的真实设置数据（旧版 boot-adv.js 各 *_Loader 适配器精简）：
 *  esp 副标题、entries(t/def)、nextBoot（默认项）、healthText —— 供各主题「引导器设置」面板显示。 */
function loaderEspText(id: LoaderId | string): string {
  if (id === "refind") return settings.refindMode === "manual" ? "vars/PreviousBoot 快照" : "ESP · refind.conf";
  if (id === "grub") {
    // 旧版定案：各显各的、零回落。手动报 ESP cfgPath，自动报分区 target.cfgRel
    if (settings.grubMode === "manual") return ui.grub?.cfgPath || "grub.cfg";
    const t = (ui.linuxGrub as unknown as { target?: { cfgRel?: string } } | null)?.target;
    return t?.cfgRel || "";
  }
  const ch = ui.channels.find((c) => c.id === id);
  return ch?.detail || "";
}
function loaderDefaultTitle(id: LoaderId | string): string {
  if (id === "refind") {
    if (settings.refindMode === "manual") {
      return (ui.status?.variables ?? []).find((v) => v.active)?.displayName ?? "未知";
    }
    return (ui.refindSim?.entries ?? []).find((e) => e.lastBoot)?.title?.trim() ?? "未知";
  }
  if (id === "grub") {
    const source = settings.grubMode === "manual" ? ui.grub : ui.linuxGrub;
    return source?.nextEntry || source?.savedEntry || "未知";
  }
  const ch = ui.channels.find((c) => c.id === id);
  const cur = String(ch?.currentDefault || "");
  const list = ((ch?.entries ?? []) as { id?: string; title?: string }[]);
  const hit = cur ? list.find((e) => cur === e.id || cur === e.title || cur.includes(e.title || "\u0000")) : undefined;
  return hit?.title || cur || ch?.lastBoot || "未知";
}
const loaders = computed<LoaderInfo[]>(() => {
  const list: LoaderInfo[] = [];
  for (const id of adv.enabledLoaders) {
    const meta = loaderMeta(id);
    const channel = ui.channels.find((c) => c.id === id);
    const detected = id === "refind" || id === "grub" ? true : Boolean(channel?.detected);
    const defTitle = loaderDefaultTitle(id);
    const count = countEntries(id);
    // 真实条目：设置面板只需要 length + 默认项（t/def）；用默认项造一个带 def 的条目
    const entries: { t?: string; def?: boolean }[] = [];
    if (defTitle && defTitle !== "未知") entries.push({ t: defTitle, def: true });
    // 补齐到 count（占位仅用于角标长度），首个为真实默认项
    while (entries.length < count) entries.push({ t: "", def: false });
    list.push({
      id,
      name: meta.name,
      state: !detected ? "off" : "ok",
      entries,
      esp: loaderEspText(id),
      perm: meta.perm,
      detected,
      nextBoot: entries.find((e) => e.def) ?? null,
      // 健康文案统一按条目数（OpenCore 不再显示「免密已开启/旧构建建议重装」等提示——
      // 免密提权入口已统一到「引导高级设置」总入口）。
      healthText: count ? `${loaderEspText(id) || "ESP"} · ${count} 个条目` : "还没读取",
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
    // 零回落（旧版 grubLoader 定案）：自动=linuxGrub，手动=ESP grub——与列表同源，
    // 否则 chips 计数 68 而列表空，看起来像坏了
    const source = settings.grubMode === "manual" ? ui.grub : ui.linuxGrub;
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
          : ((ui.refindSim?.entries?.find((e) => e.lastBoot)?.title ?? "").trim() || "—");
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

const toast = useToastStore();
/** 「需要开免密」的启动提示弹窗是否显示 */
const helperPromptVisible = ref(false);
/** 浏览器/手机端需要输入 token（无 token 或 401）：由 ui.remoteTokenMissing 驱动 */
const needToken = computed(() => !isTauri() && ui.remoteTokenMissing);
async function onTokenConfirmed() {
  // 填了 token：重启传输（ui.start 会按新 authToken 重连并拉快照）
  await ui.start().catch(() => {});
}
/** 弹窗用户操作完毕后：装了免密→静默扫；取消提醒/暂不→本次不扫 */
async function onHelperPromptResolved(action: "installed" | "snoozed" | "cancelled") {
  helperPromptVisible.value = false;
  // 勾选了「启动时使用管理员扫描」，所以扫描**一定做**；建议窗只是先问要不要开免密。
  if (action === "installed") {
    // 刚输了免密装了助手 → 静默扫，绝不再弹系统授权框（否则逻辑错）
    await runStartupAdminScan(false).catch(() => {});
  } else {
    // 取消提醒 / 暂不 → 免密未装，但仍要用管理员权限扫 → 弹系统密码授权框扫
    await runStartupAdminScan(true).catch(() => {});
  }
}
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
// 各引导器「重新读取」等操作把回执写到 ui.bootNotice → 显示到窗口底部 status-bar（与手动识别同一条）
watch(
  () => ui.bootNotice,
  (n) => {
    if (!n || !n.text) return;
    statusBar.value = { icon: n.error ? "✕" : "✓", text: n.text, error: n.error };
  },
  { deep: true },
);

/* ============ Linux 免密提权（旧版 index.html:985–1100 的 priv* 逻辑） ============ */
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
  // 无论 K 与否都挂：body[data-ba-fill] main 的 grid-template-rows + overflow:hidden
  // 是「#tabs 不被内容挤扁、#tab-boot 自带滚动」的前提。K 也在 .pane-main 网格里，
  // 之前 K 删掉它 → 内容 2200px 高把 #tabs 挤成 1px（用户：选不套用主题标签栏消失）。
  document.body.dataset.baFill = "1";
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
/** 屏幕适配：给 #tab-boot 挂 .vw-900/.vw-760/.vw-520 断点类（旧 applyVwClasses） */
function applyVw(el: HTMLElement, width: number) {
  el.classList.toggle("vw-g", width >= 900);
  el.classList.toggle("vw-900", width <= 900);
  el.classList.toggle("vw-760", width <= 760);
  el.classList.toggle("vw-520", width <= 520);
}
let vwObs: ResizeObserver | null = null;
onMounted(() => {
  const el = document.getElementById("tab-boot");
  if (!el || typeof ResizeObserver === "undefined") return;
  vwObs = new ResizeObserver((entries) => { for (const en of entries) applyVw(el, en.contentRect.width); });
  vwObs.observe(el);
  applyVw(el, el.getBoundingClientRect().width);
});
onUnmounted(() => { vwObs?.disconnect(); vwObs = null; });
// 兜底：mounted 后宽/左右布局再补发一次（防 setup 期过早、窗口尚未就绪）
onMounted(() => {
  window.setTimeout(() => {
    layoutWidthSent = "";
    ensureLayoutWidth();
  }, 800);
});

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
          @update:ratio="adv.setRatio"
          @reorder="(order: string[]) => adv.setOrder(order as any)"
        >
          <!-- G / 不套用主题(K)：与 A 一致——每个引导器只展示引导列表 +
               折叠的「引导器设置」（其余原完整区块内容收进折叠面板）。 -->
          <div
            v-for="id in adv.enabledLoaders"
            :key="id"
            :data-loader-section="id"
            class="boot-loader-block"
          >
            <!-- 仅「不套用主题(K)」显示每个引导器的头部标题（多引导器堆叠需区分）；
                 启用主题(A/B/C/G/I/J)不显示任何区块标题。 -->
            <div v-if="adv.effectiveTheme === 'K'" class="boot-loader-head">
              <span class="zone-tag">{{ loaders.find((l) => l.id === id)?.name ?? id }} 区</span>
              <b>{{ loaders.find((l) => l.id === id)?.name ?? id }} 启动项</b>
              <span class="cnt">· {{ bootRowsByLoader[id]?.length ?? 0 }} 项</span>
            </div>
            <BootList :loader="id" @select="(sel: string) => adv.setSelection(id, sel)" />
            <LoaderSettings :loader="id" />
          </div>
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
  <!-- 浮层 toast（GrubSection/RefindSection/ChannelSettings 等的「重新读取」回执落这里） -->
  <div v-if="toast.text" id="toast" :class="{ bad: toast.bad }" role="status">{{ toast.text }}</div>
  <div v-if="statusBar.text" id="status-bar" aria-live="polite" :class="['status-bar', statusBar.error ? 'error' : 'ok']" role="status">
    <span id="status-bar-icon" class="status-bar-icon" aria-hidden="true">{{ statusBar.icon }}</span>
    <span id="status-bar-text" class="status-bar-text">{{ statusBar.text }}</span>
    <button type="button" id="status-bar-close" class="status-bar-close" title="收起" @click="dismissStatusBar">×</button>
  </div>
  <HelperPrompt v-if="helperPromptVisible" @resolved="onHelperPromptResolved" />
  <TokenPrompt v-if="needToken" @confirmed="onTokenConfirmed" />
</template>
