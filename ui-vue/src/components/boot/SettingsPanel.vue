<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { SUPPORTED_LOCALES, LOCALE_LABELS, currentPref, setLocale, t, type LangPref } from "@/i18n";
import {
  call,
  setAutostart,
  setCloseAction,
  setHideFromDockTaskbar,
  setStartInTray,
} from "@/api/tauri";

/**
 * 基本设置页（原 `ui/index.html` 的 `#tab-settings`，469~834 行）。
 *
 * ## 1:1 结构
 * ```
 * section.settings
 *   h2「应用设置」
 *   label.setting × N
 *     .setting-text > strong（标题）+ small（说明）+ small.setting-state（状态）
 *     input[type=checkbox] / select / input[type=range]
 *   h2「远程控制」
 *   ...
 * ```
 *
 * 旧版共 35 个设置项，分「应用设置」（外观/窗口/托盘）与「远程控制」（HTTP 服务）两节。
 */

const ui = useUiStateStore();
/** 热区边框颜色（旧版 #main-hotzone-border-color，默认 #ff0000） */
const hotzoneBorderColor = computed(() => (s.value as any)?.mainHotzoneBorderColor || "#ff0000");
function setHotzoneBorderColor(color: string) {
  void commit("set_main_hotzone_border_color", { color });
}

function openLoginItems() {
  void call("open_login_items_settings").catch(() => {});
}

/** HTTP 请求日志（旧版 http.requests，每项「时间 来源 [通道] 摘要 — 状态」） */
const httpRequests = computed<any[]>(() => {
  const reqs = (ui.http as any)?.requests;
  return Array.isArray(reqs) ? reqs : [];
});
function fmtRequest(e: any): string {
  const time = e?.time ? new Date(Number(e.time)).toLocaleTimeString("zh-CN", { hour12: false }) : "";
  const msg = e?.message ? `：${e.message}` : "";
  return `${time} ${e?.source ?? ""} [${e?.channel ?? ""}] ${e?.summary ?? ""} — ${e?.status ?? ""}${msg}`;
}

const s = computed(() => ui.settings);

/** 本地即时值（拖动滑块时先更新 UI，松手再落盘） */
const local = ref<Record<string, number | string | boolean>>({});

function val<T>(key: string, fallback: T): T {
  const v = local.value[key];
  if (v !== undefined) return v as T;
  const fromSettings = (s.value as Record<string, unknown> | null)?.[key];
  return (fromSettings as T) ?? fallback;
}

function setLocal(key: string, value: number | string | boolean) {
  local.value = { ...local.value, [key]: value };
}

/** 远程控制表单（旧版 #http-bind / #http-port / #token-input） */
const httpBind = ref("0.0.0.0");
const httpPort = ref("12300");
const tokenInput = ref("");
const soundPath = ref("");

watch(
  () => ui.http,
  (http) => {
    if (!http) return;
    const parts = String(http.addr || "").split(":");
    if (parts.length === 2) {
      httpBind.value = parts[0] ?? "0.0.0.0";
      httpPort.value = parts[1] ?? "12300";
    }
  },
  { immediate: true },
);

function toggleHttp() {
  // 旧版：set_http_enabled { enabled }（参数名 enabled，非 on）
  commit("set_http_enabled", { enabled: !ui.http?.listening });
}

function applyHttp() {
  // 旧版：端口校验 1-65535 → set_http_bind { bind } 链 set_http_port { port }
  const port = Number(httpPort.value);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    window.dispatchEvent(new CustomEvent("ab-status-bar", { detail: { text: t("settings.portRange"), error: true } }));
    return;
  }
  void call("set_http_bind", { bind: httpBind.value.trim() }).then(() =>
    call("set_http_port", { port }),
  );
}

function regenerateToken() {
  commit("regenerate_http_token", { value: tokenInput.value.trim() });
}

async function copyToken() {
  try {
    const text = await call<string>("reveal_http_token", {});
    await navigator.clipboard.writeText(text);
  } catch {
    /* 剪贴板不可用 */
  }
}

function saveToken() {
  commit("set_http_token", { value: tokenInput.value.trim() });
}

function retryHttp() {
  commit("retry_http", {});
}

function killOccupant() {
  commit("kill_port_occupant", {});
}

function openWeb() {
  // open_web_page 无参：后端自己算当前控制页 URL 并打开
  void call("open_web_page").catch(() => {});
}

/** 落盘（旧版每个设置项都有对应的 set_* 命令） */
function commit(command: string, payload: Record<string, unknown>) {
  void call(command, payload).catch(() => {});
}

function fmtPct(v: number) {
  return `${v}%`;
}

/** 设置联动禁用（旧版 applySettingLinkage） */
const notifyOn = computed(() => Boolean((ui.settings as any)?.notifyEnabled));
const httpOn = computed(() => Boolean((ui.settings as any)?.httpEnabled));
const soundOn = computed(() => Boolean((ui.settings as any)?.playSound));
const centerLocked = computed(() => (ui.settings as any)?.mainWindowPosition === "center");
const autoHideOn = computed(() => Boolean((ui.settings as any)?.mainAutoHide));
/** 通道 notify/delay：通知总开关关闭时禁用 */
const channelDisabled = computed(() => !notifyOn.value);
/** HTTP 配置/token：HTTP 服务关闭时禁用 */
const httpDisabled = computed(() => !httpOn.value);
/** 提示音文件：提示音关闭时禁用 */
const soundDisabled = computed(() => !soundOn.value);
/** 贴边配置：居中模式禁用；auto-hide 关闭时 dock 配置也禁用 */
const autoHideDisabled = computed(() => centerLocked.value);
const dockStripDisabled = computed(() => centerLocked.value || !autoHideOn.value);

/** 访客 UI 模式（旧版 isGuestUiView）：URL 带 abUi=light|dark，远程查看主窗时指定亮暗 */
const guestUiTheme = (() => {
  try {
    const v = (new URLSearchParams(location.search).get("abUi") || "").toLowerCase();
    return v === "light" || v === "dark" ? v : "";
  } catch {
    return "";
  }
})();
const isGuestUi = computed(() => guestUiTheme !== "");
/** 语言偏好：'auto' = 跟随系统 */
const langPref = ref<LangPref>(currentPref());
function onLangChange(e: Event) {
  const v = (e.target as HTMLSelectElement).value as LangPref;
  langPref.value = v;
  setLocale(v);
  // 运行时切换：BootApp/多数组件用 import 的 t（不随 locale 重渲），整页刷新确保全站即时生效
  try { location.reload(); } catch { /* 忽略 */ }
}
const LANG_OPTIONS = [
  { v: "auto" as LangPref, label: t("settings.autoLabel") },
  ...SUPPORTED_LOCALES.map((loc) => ({ v: loc as LangPref, label: LOCALE_LABELS[loc] })),
];
</script>

<template>
  <section :aria-label="$t('settings.sectionAria')" class="settings">
    <h2>{{ $t("settings.title") }}</h2>

    <!-- 语言 / Language（i18n：Tier0 + Tier1 共 9 种，'auto' 跟随系统） -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.lang") }}</strong>
        <small>{{ $t("settings.langNote") }}</small>
      </span>
      <select id="app-language" :value="langPref" @change="onLangChange">
        <option v-for="o in LANG_OPTIONS" :key="o.v" :value="o.v">{{ o.label }}</option>
      </select>
    </label>

    <!-- 开机启动 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.autostart") }}</strong>
        <small>{{ $t("settings.autostartNote") }}</small>
        <small class="setting-state" id="autostart-state">{{ s?.autostartState || "" }}</small>
      </span>
      <input id="autostart"
        type="checkbox"
        :checked="Boolean(s?.autostart)"
        @change="setAutostart(!s?.autostart)"
      />
    </label>

    <!-- 打开「登录项与扩展」设置（旧版 #login-items，仅 requires-approval 时显示） -->
    <button
      v-if="s?.autostartState === 'requires-approval'"
      type="button"
      id="login-items"
      class="link"
      @click="openLoginItems"
    >
      {{ $t("settings.loginItems") }}
    </button>

    <!-- 启动时隐藏主界面 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.hideOnStart") }}</strong>
        <small>{{ $t("settings.hideOnStartNote") }}</small>
      </span>
      <input id="start-in-tray"
        type="checkbox"
        :checked="Boolean(s?.startInTray)"
        @change="setStartInTray(!s?.startInTray)"
      />
    </label>

    <!-- 窗口始终置顶 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.alwaysOnTop") }}</strong>
        <small>{{ $t("settings.alwaysOnTopNote") }}</small>
      </span>
      <input id="main-always-on-top"
        type="checkbox"
        :checked="Boolean(s?.mainAlwaysOnTop)"
        @change="commit('set_main_always_on_top', { enabled: !s?.mainAlwaysOnTop })"
      />
    </label>

    <!-- 贴边自动隐藏 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.edgeHide") }}</strong>
        <small>{{ $t("settings.edgeHideNote") }}</small>
        <small class="setting-state" id="main-auto-hide-state">{{ s?.mainAutoHide ? $t("settings.on") : $t("settings.off") }}</small>
      </span>
      <input id="main-auto-hide"
        type="checkbox"
        :checked="Boolean(s?.mainAutoHide)"
        @change="commit('set_main_auto_hide', { enabled: !s?.mainAutoHide })" :disabled="autoHideDisabled"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.edgeTrigger") }}</strong>
        <small>{{ $t("settings.edgeTriggerNote") }}</small>
      </span>
      <input id="main-auto-hide-edge-distance" type="number" min="0" max="200" step="1" :value="val('mainAutoHideEdgeDistance', 24)" @change="commit('set_main_auto_hide_edge_distance', { distance: Number(($event.target as HTMLInputElement).value) })" />
    </label>

    <!-- 贴边白条 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.hoverDelay") }}</strong>
        <small>{{ $t("settings.hoverDelayNote") }}</small>
      </span>
      <input
        id="main-dock-strip-hover-ms"
        type="number"
        min="0"
        max="2000"
        step="50"
        :value="val('mainDockStripHoverMs', 120)"
        @change="commit('set_main_dock_strip_hover_ms', { value: Number(($event.target as HTMLInputElement).value) })" :disabled="dockStripDisabled"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.barInset") }}</strong>
        <small>{{ $t("settings.barInsetNote") }}</small>
      </span>
      <input
        id="main-dock-strip-edge-gap"
        type="number"
        min="0"
        max="100"
        :value="val('mainDockStripEdgeGap', 0)"
        @change="commit('set_main_dock_strip_edge_gap', { value: Number(($event.target as HTMLInputElement).value) })" :disabled="dockStripDisabled"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.barThickness") }}</strong>
        <small>{{ $t("settings.barThicknessNote") }}</small>
        <small class="setting-state" id="main-dock-strip-thickness-state">{{ val("mainDockStripThickness", 6) }} px</small>
      </span>
      <input id="main-dock-strip-thickness"
        type="range"
        min="2"
        max="40"
        :value="val('mainDockStripThickness', 6)"
        @input="setLocal('mainDockStripThickness', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_dock_strip_thickness', { value: val('mainDockStripThickness', 6) })"
      step="1" :disabled="dockStripDisabled"
    />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.barLength") }}</strong>
        <small>{{ $t("settings.barLengthNote") }}</small>
        <small class="setting-state" id="main-dock-strip-length-state">{{ fmtPct(val("mainDockStripLength", 100)) }}</small>
      </span>
      <input id="main-dock-strip-length"
        type="range"
        min="10"
        max="100"
        :value="val('mainDockStripLength', 100)"
        @input="setLocal('mainDockStripLength', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_dock_strip_length', { value: val('mainDockStripLength', 100) })"
      step="5" :disabled="dockStripDisabled"
    />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.barOpacity") }}</strong>
        <small>{{ $t("settings.barOpacityNote") }}</small>
        <small class="setting-state" id="main-dock-strip-opacity-state">{{ fmtPct(val("mainDockStripOpacity", 100)) }}</small>
      </span>
      <input id="main-dock-strip-opacity"
        type="range"
        min="10"
        max="100"
        :value="val('mainDockStripOpacity', 100)"
        @input="setLocal('mainDockStripOpacity', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_dock_strip_opacity', { value: val('mainDockStripOpacity', 100) })"
      step="5" :disabled="dockStripDisabled"
    />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.barColor") }}</strong>
        <small>{{ $t("settings.barColorNote") }}</small>
      </span>
      <input id="main-dock-strip-color"
        type="color"
        :value="val('mainDockStripColor', '#ffffff')"
        @change="commit('set_main_dock_strip_color', { color: ($event.target as HTMLInputElement).value })" :disabled="dockStripDisabled"
      />
    </label>

    <!-- 热区 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.hotzone") }}</strong>
        <small>{{ $t("settings.hotzoneNote") }}</small>
        <small class="setting-state" id="main-hotzone-thickness-state">{{ val("mainHotzoneThickness", 40) }} px</small>
      </span>
      <input id="main-hotzone-thickness"
        type="range"
        min="1"
        max="100"
        :value="val('mainHotzoneThickness', 40)"
        @input="setLocal('mainHotzoneThickness', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_hotzone_thickness', { value: val('mainHotzoneThickness', 40) })"
      step="1"
    />
    </label>

    <div class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.hotzoneDebug") }}</strong>
        <small>{{ $t("settings.hotzoneDebugNote") }}</small>
        <small class="setting-state" id="main-hotzone-opacity-state">{{ fmtPct(val("mainHotzoneOpacity", 0)) }}</small>
      </span>
      <span class="remote-inline">
        <input
          id="main-hotzone-color"
          type="color"
          :value="val('mainHotzoneColor', '#4f46e5')"
          @change="commit('set_main_hotzone_color', { color: ($event.target as HTMLInputElement).value })"
        />
        <input
          id="main-hotzone-opacity"
          type="range"
          min="0"
          max="100"
          :value="val('mainHotzoneOpacity', 0)"
          @input="setLocal('mainHotzoneOpacity', Number(($event.target as HTMLInputElement).value))"
          @change="commit('set_main_hotzone_opacity', { value: val('mainHotzoneOpacity', 0) })"
        step="5"
      />
      </span>
    </div>

    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.hotzoneBorder") }}</strong>
        <small>{{ $t("settings.hotzoneBorderNote") }}</small>
      </span>
      <input id="main-hotzone-border"        type="checkbox"
        :checked="Boolean(s?.mainHotzoneBorder)"
        @change="commit('set_main_hotzone_border', { enabled: !s?.mainHotzoneBorder })"
      />
      <input id="main-hotzone-border-color" type="color" :value="hotzoneBorderColor" @input="setHotzoneBorderColor(($event.target as HTMLInputElement).value)" />
    </label>

    <!-- 窗口位置 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.fixedPos") }}</strong>
        <small>{{ $t("settings.fixedPosNote") }}</small>
      </span>
      <select id="main-window-position"
        :value="s?.mainWindowPosition || 'last'"
        @change="commit('set_main_window_position', { mode: ($event.target as HTMLSelectElement).value })"
      >
        <option value="free">{{ $t("settings.optFree") }}</option>
        <option value="center">{{ $t("settings.optCenter") }}</option>
        <option value="last">{{ $t("settings.optLast") }}</option>
      </select>
    </label>

    <!-- 固定显示器（旧版 main-fixed-monitor） -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.fixedDisplay") }}</strong>
        <small>{{ $t("settings.fixedDisplayNote") }}</small>
      </span>
      <select
        id="main-fixed-monitor"
        :value="s?.mainFixedMonitor || 'auto'"
        @change="commit('set_main_fixed_monitor', { name: ($event.target as HTMLSelectElement).value })"
      >
        <option value="auto">{{ $t("settings.optAutoDisplay") }}</option>
        <option v-for="m in (s?.mainMonitorOptions ?? [])" :key="m" :value="m">{{ m }}</option>
      </select>
    </label>

    <!-- 窗口宽度 / 高度（旧版 window-size-*） -->
    <div class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.winWidth") }}</strong>
        <small>{{ $t("settings.winWidthNote") }}</small>
        <small class="setting-state" id="window-size-width-state">{{ val("windowSizeWidth", 460) }} px</small>
      </span>
      <span class="remote-inline">
        <input
          id="window-size-width"
          type="range"
          min="360"
          max="1600"
          step="4"
          :value="val('windowSizeWidth', 460)"
          @input="setLocal('windowSizeWidth', Number(($event.target as HTMLInputElement).value))"
          @change="commit('set_window_size', { width: val('windowSizeWidth', 460), height: val('windowSizeHeight', 560) })"
        />
      </span>
    </div>

    <div class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.winHeight") }}</strong>
        <small>{{ $t("settings.winHeightNote") }}</small>
        <small class="setting-state" id="window-size-height-state">{{ val("windowSizeHeight", 560) }} px</small>
      </span>
      <span class="remote-inline">
        <input
          id="window-size-height"
          type="range"
          min="560"
          max="1200"
          step="4"
          :value="val('windowSizeHeight', 560)"
          @input="setLocal('windowSizeHeight', Number(($event.target as HTMLInputElement).value))"
          @change="commit('set_window_size', { width: val('windowSizeWidth', 460), height: val('windowSizeHeight', 560) })"
        />
      </span>
    </div>

    <div class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.resetPos") }}</strong>
        <small>{{ $t("settings.resetPosNote") }}</small>
      </span>
      <button type="button" id="reset-main-window-position" class="secondary" @click="commit('reset_main_window_position', {})">
        {{ $t("settings.resetPosBtn") }}
      </button>
    </div>

    <div class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.resetSize") }}</strong>
        <small>{{ $t("settings.resetSizeNote") }}</small>
      </span>
      <button type="button" id="reset-main-window-size" class="secondary" @click="commit('reset_window_size', {})">
        {{ $t("settings.resetSizeBtn") }}
      </button>
    </div>

    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.lockSize") }}</strong>
        <small>{{ $t("settings.lockSizeNote") }}</small>
      </span>
      <input id="window-size-locked"
        type="checkbox"
        :checked="Boolean(s?.windowSizeLocked)"
        @change="commit('set_window_size_locked', { locked: !s?.windowSizeLocked })"
      />
    </label>

    <!-- Dock / 任务栏 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.hideTaskbar") }}</strong>
        <small>{{ $t("settings.hideTaskbarNote") }}</small>
      </span>
      <input id="hide-from-dock-taskbar"
        type="checkbox"
        :checked="Boolean(s?.hideFromDockTaskbar)"
        @change="setHideFromDockTaskbar(!s?.hideFromDockTaskbar)"
      />
    </label>

    <!-- 关闭行为 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.onClose") }}</strong>
        <small>{{ $t("settings.onCloseNote") }}</small>
      </span>
      <select id="close-action"
        :value="s?.closeAction || 'tray'"
        @change="setCloseAction(($event.target as HTMLSelectElement).value as 'tray' | 'quit')"
      >
        <option value="tray">{{ $t("settings.optTray") }}</option>
        <option value="quit">{{ $t("settings.optQuit") }}</option>
      </select>
    </label>

    <!-- 托盘菜单样式 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.trayMode") }}</strong>
        <small>{{ $t("settings.trayModeNote") }}</small>
      </span>
      <select id="tray-menu-style"
        :value="s?.trayMenuStyle === 'unified' ? 'unified' : 'classic'"
        @change="commit('set_tray_menu_style', { mode: ($event.target as HTMLSelectElement).value })"
      >
        <option value="classic">{{ $t("settings.optClassic") }}</option>
        <option value="unified">{{ $t("settings.optUnified") }}</option>
      </select>
    </label>

    <!-- 主题 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.uiTheme") }}</strong>
        <small v-if="isGuestUi">{{ $t("settings.uiThemeGuestNote") }}</small>
        <small v-else>{{ $t("settings.uiThemeNote") }}</small>
      </span>
      <select id="theme"
        :value="isGuestUi ? guestUiTheme : (s?.theme || 'dark')"
        :disabled="isGuestUi"
        @change="commit('set_theme', { theme: ($event.target as HTMLSelectElement).value })"
      >
        <option value="dark">{{ $t("settings.optDark") }}</option>
        <option value="light">{{ $t("settings.optLight") }}</option>
        <option value="system">{{ $t("settings.optSystem") }}</option>
      </select>
    </label>

    <!-- 强调色 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.accent") }}</strong>
        <small>{{ $t("settings.accentNote") }}</small>
      </span>
      <input id="accent-color"
        type="color"
        :value="s?.accentColor || '#6366f1'"
        @change="commit('set_accent_color', { color: ($event.target as HTMLInputElement).value })"
      />
    </label>

    <!-- 不透明度 -->
    <label class="setting">
      <span class="setting-text">
        <strong>{{ $t("settings.winOpacity") }}</strong>
        <small>{{ $t("settings.winOpacityNote") }}</small>
        <small class="setting-state" id="opacity-state">{{ fmtPct(val("opacityPercent", 90)) }}</small>
      </span>
      <input id="opacity"
        type="range"
        min="0"
        max="100"
        :value="val('opacityPercent', 90)"
        @input="setLocal('opacityPercent', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_opacity', { percent: val('opacityPercent', 90) })"
      step="5"
    />
    </label>

    <!-- 远程控制（旧版 section#remote） -->
    <section :aria-label="$t('settings.remoteAria')" class="settings" id="remote">
      <h2>{{ $t("settings.remoteControl") }}</h2>

      <div class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.httpListen") }}</strong>
          <small class="setting-state" id="http-state">{{ ui.http?.stateText || $t("settings.httpReading") }}</small>
        </span>
        <input id="http-enabled" type="checkbox" :checked="Boolean(ui.http?.listening)" @change="toggleHttp" />
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.listenAddr") }}</strong>
          <small>{{ $t("settings.listenAddrNote") }}</small>
        </span>
        <span class="remote-inline">
          <input id="http-bind" v-model="httpBind" spellcheck="false" type="text" :disabled="httpDisabled" />
          <input id="http-port" v-model="httpPort" inputmode="numeric" spellcheck="false" type="text" :disabled="httpDisabled" />
          <button type="button" id="http-apply" class="secondary" @click="applyHttp">{{ $t("settings.apply") }}</button>
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.tokenTitle") }}</strong>
          <small class="setting-state mono" id="token-hint">{{ ui.http?.tokenHint || $t("settings.tokenUnset") }}</small>
          <small>{{ $t("settings.tokenNote") }}</small>
        </span>
        <span class="remote-inline">
          <input id="token-input" v-model="tokenInput" spellcheck="false" type="text" :placeholder="t('settings.tokenInputPh')" :disabled="httpDisabled" />
          <button type="button" class="secondary" @click="regenerateToken">{{ $t("settings.regenerate") }}</button>
        </span>
      </div>

      <div class="remote-hint">
        <strong>{{ $t("settings.howToCall") }}</strong>
        <small>{{ $t("settings.howToCallNote") }}</small>
        <pre id="http-hint">{{ $t("settings.copyCmdHint") }}</pre>
        <div class="actions">
          <button type="button" class="secondary" id="http-hint-copy" @click="copyToken">{{ $t("settings.copyCmd") }}</button>
        </div>
      </div>
      <div class="remote-log">
        <strong>{{ $t("settings.recentReq") }}</strong>
        <ol id="http-requests">
          <li v-for="(entry, i) in httpRequests" :key="i">{{ fmtRequest(entry) }}</li>
        </ol>
      </div>

      <div v-if="(ui.http as any)?.occupiedBy" class="port-occupant" id="port-occupant">
        <span class="setting-text">
          <strong>{{ $t("settings.portBusy") }}</strong>
          <small class="setting-state" id="port-occupant-message">{{ (ui.http as any)?.occupiedBy || "" }}</small>
          <small id="port-occupant-hint">{{ $t("settings.portBusyNote") }}</small>
        </span>
        <span class="remote-inline">
          <button type="button" id="retry-http" class="secondary" @click="retryHttp">{{ $t("settings.retryListen") }}</button>
          <button type="button" id="kill-occupant" class="secondary danger" @click="killOccupant">{{ $t("settings.killOccupant") }}</button>
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.tokenSettings") }}</strong>
          <small>{{ $t("settings.tokenSettingsNote") }}</small>
        </span>
        <span class="remote-inline">
          <button type="button" id="token-reveal" class="secondary" @click="commit('reveal_http_token', {})">{{ $t("settings.show") }}</button>
          <button type="button" id="token-copy" class="secondary" @click="copyToken">{{ $t("settings.copy") }}</button>
          <button type="button" id="token-regenerate" class="secondary" @click="regenerateToken">{{ $t("settings.regenerate") }}</button>
          <button type="button" id="token-save" class="secondary" @click="saveToken" :disabled="httpDisabled">{{ $t("settings.save") }}</button>
        </span>
      </div>

      <!-- 远程请求通知（旧版 notify-enabled 起） -->
      <label class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.remoteNotify") }}</strong>
          <small>{{ $t("settings.remoteNotifyNote") }}</small>
        </span>
        <input id="notify-enabled" type="checkbox" :checked="Boolean(s?.notifyEnabled)" @change="commit('set_notify_enabled', { on: !s?.notifyEnabled })" />
      </label>

      <div class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.remoteAuthDialog") }}</strong>
          <small>{{ $t("settings.remoteAuthDialogNote") }}</small>
        </span>
        <input id="remote-allow-gui-auth" type="checkbox" :checked="Boolean(s?.remoteAllowGuiAuth)" @change="commit('set_remote_allow_gui_auth', { on: !s?.remoteAllowGuiAuth })" />
      </div>

      <!-- 通知渠道（旧版 http-channel-* / cli-channel-*） -->
      <div class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.httpChannel") }}</strong>
          <small>{{ $t("settings.httpChannelNote") }}</small>
        </span>
        <span class="remote-inline">
          <input id="http-channel-notify" type="checkbox" :checked="Boolean(s?.httpChannelNotify)" @change="commit('set_channel_notify', { channel: 'http', enabled: !s?.httpChannelNotify })" :disabled="channelDisabled" />
          <input id="http-channel-delay" type="text" inputmode="numeric" spellcheck="false" :value="val('httpChannelDelay', 0)" @change="commit('set_channel_delay', { channel: 'http', seconds: Number(($event.target as HTMLInputElement).value) })" :disabled="channelDisabled" />
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.sshChannel") }}</strong>
          <small>{{ $t("settings.sshChannelNote") }}</small>
        </span>
        <span class="remote-inline">
          <input id="cli-channel-notify" type="checkbox" :checked="Boolean(s?.cliChannelNotify)" @change="commit('set_channel_notify', { channel: 'cli', enabled: !s?.cliChannelNotify })" :disabled="channelDisabled" />
          <input id="cli-channel-delay" type="text" inputmode="numeric" spellcheck="false" :value="val('cliChannelDelay', 0)" @change="commit('set_channel_delay', { channel: 'cli', seconds: Number(($event.target as HTMLInputElement).value) })" :disabled="channelDisabled" />
        </span>
      </div>

      <!-- 提示音（旧版 setting-sound / play-sound / sound-file-path / sound-apply） -->
      <div class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.sound") }}</strong>
          <small>{{ $t("settings.soundNote") }}</small>
        </span>
        <span class="remote-inline">
          <label class="setting" style="border: 0; padding: 0">
            <input id="setting-sound" type="checkbox" :checked="Boolean(s?.playSound)" @change="commit('set_play_sound', { enabled: !s?.playSound })" />
          </label>
          <button type="button" id="play-sound" class="secondary" @click="commit('set_play_sound', { enabled: true })" :disabled="soundDisabled">{{ $t("settings.preview") }}</button>
          <input id="sound-file-path" v-model="soundPath" spellcheck="false" type="text" :placeholder="$t('settings.soundPathPh')" />
          <button type="button" id="sound-apply" class="secondary" @click="commit('set_sound_file_path', { path: soundPath })" :disabled="soundDisabled">{{ $t("settings.apply") }}</button>
        </span>
      </div>

      <!-- 打开配置目录（旧版 open-dir-actions / open-config-dir） -->
      <div class="setting" id="open-dir-actions">
        <button type="button" id="open-config-dir" class="secondary" @click="commit('open_config_dir', {})">{{ $t("settings.openConfigDir") }}</button>
      </div>

      <!-- 待确认卡（旧版 .pending#pending-card） -->
      <div v-if="(ui.state as any)?.pending" class="pending" id="pending-card">
        <strong id="pending-title">{{ $t("settings.pendingTitle") }}</strong>
        <p id="pending-detail">{{ (ui.state as any).pending.text }}</p>
        <span class="remote-inline">
          <button type="button" id="pending-run" class="primary-sol" @click="commit('confirm_pending', {})">{{ $t("settings.run") }}</button>
          <button type="button" id="pending-cancel" class="secondary" @click="commit('cancel_pending', {})">{{ $t("settings.cancel") }}</button>
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.openWeb") }}</strong>
          <small>{{ $t("settings.openWebNote") }}</small>
        </span>
        <button type="button" id="open-web" class="secondary" @click="openWeb">{{ $t("settings.open") }}</button>
      </div>

      <label class="setting">
        <span class="setting-text">
          <strong>{{ $t("settings.allowRemoteAuth") }}</strong>
          <small>{{ $t("settings.allowRemoteAuthNote") }}</small>
        </span>
        <input
          type="checkbox"
          :checked="Boolean(s?.remoteAllowGuiAuth)"
          @change="commit('set_remote_allow_gui_auth', { on: !s?.remoteAllowGuiAuth })"
        />
      </label>
    </section>
  </section>
</template>
