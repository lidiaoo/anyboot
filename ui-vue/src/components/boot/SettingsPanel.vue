<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
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
  commit("set_http_enabled", { on: !ui.http?.listening });
}

function applyHttp() {
  commit("set_http_listen", { bind: httpBind.value.trim(), port: Number(httpPort.value) || 12300 });
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
</script>

<template>
  <section aria-label="应用设置" class="settings">
    <h2>应用设置</h2>

    <!-- 开机启动 -->
    <label class="setting">
      <span class="setting-text">
        <strong>开机启动</strong>
        <small>登录系统后自动启动，常驻托盘待命</small>
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
      打开「登录项与扩展」设置
    </button>

    <!-- 启动时隐藏主界面 -->
    <label class="setting">
      <span class="setting-text">
        <strong>启动时隐藏主界面</strong>
        <small>启动后不直接打开主菜单，仅显示在托盘</small>
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
        <strong>窗口始终置顶</strong>
        <small>主窗口任何情况下都不被别的窗口盖住；取消勾选后可被正常覆盖</small>
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
        <strong>贴边自动隐藏</strong>
        <small>鼠标离开后主窗口贴到最近的屏幕边，只露一条边条；悬停边条再展开</small>
        <small class="setting-state" id="main-auto-hide-state">{{ s?.mainAutoHide ? "已开启" : "已关闭" }}</small>
      </span>
      <input id="main-auto-hide"
        type="checkbox"
        :checked="Boolean(s?.mainAutoHide)"
        @change="commit('set_main_auto_hide', { enabled: !s?.mainAutoHide })"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>贴边触发距离</strong>
        <small>主窗口距屏幕边框不超过该像素值、且鼠标离开窗口时才隐藏（0–200，越大越灵敏）</small>
      </span>
      <input id="main-auto-hide-edge-distance" type="number" min="0" max="200" step="1" :value="val('mainAutoHideEdgeDistance', 24)" @change="commit('set_main_auto_hide_edge_distance', { distance: Number(($event.target as HTMLInputElement).value) })" />
    </label>

    <!-- 贴边白条 -->
    <label class="setting">
      <span class="setting-text">
        <strong>悬浮呼出延时</strong>
        <small>鼠标压住白条持续这么久才展开主窗（0–2000 毫秒，0 = 立即）</small>
      </span>
      <input
        id="main-dock-strip-hover-ms"
        type="number"
        min="0"
        max="2000"
        step="50"
        :value="val('mainDockStripHoverMs', 120)"
        @change="commit('set_main_dock_strip_hover_ms', { value: Number(($event.target as HTMLInputElement).value) })"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>白条距屏幕边界</strong>
        <small>贴边隐藏时白条离屏幕边框留出的距离（0–100px，0 = 紧贴边缘）</small>
      </span>
      <input
        id="main-dock-strip-edge-gap"
        type="number"
        min="0"
        max="100"
        :value="val('mainDockStripEdgeGap', 0)"
        @change="commit('set_main_dock_strip_edge_gap', { value: Number(($event.target as HTMLInputElement).value) })"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>贴边白条厚度</strong>
        <small>6px（拖动即时预览）</small>
        <small class="setting-state" id="main-dock-strip-thickness-state">{{ val("mainDockStripThickness", 6) }} px</small>
      </span>
      <input id="main-dock-strip-thickness"
        type="range"
        min="2"
        max="40"
        :value="val('mainDockStripThickness', 6)"
        @input="setLocal('mainDockStripThickness', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_dock_strip_thickness', { value: val('mainDockStripThickness', 6) })"
      step="1"
    />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>贴边白条长度</strong>
        <small>100%（主窗那条边长的百分比）</small>
        <small class="setting-state" id="main-dock-strip-length-state">{{ fmtPct(val("mainDockStripLength", 100)) }}</small>
      </span>
      <input id="main-dock-strip-length"
        type="range"
        min="10"
        max="100"
        :value="val('mainDockStripLength', 100)"
        @input="setLocal('mainDockStripLength', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_dock_strip_length', { value: val('mainDockStripLength', 100) })"
      step="5"
    />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>贴边白条不透明度</strong>
        <small>100%（拖动即时预览）</small>
        <small class="setting-state" id="main-dock-strip-opacity-state">{{ fmtPct(val("mainDockStripOpacity", 100)) }}</small>
      </span>
      <input id="main-dock-strip-opacity"
        type="range"
        min="10"
        max="100"
        :value="val('mainDockStripOpacity', 100)"
        @input="setLocal('mainDockStripOpacity', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_dock_strip_opacity', { value: val('mainDockStripOpacity', 100) })"
      step="5"
    />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>贴边白条颜色</strong>
        <small>点开调色板选一个色调，即时预览</small>
      </span>
      <input id="main-dock-strip-color"
        type="color"
        :value="val('mainDockStripColor', '#ffffff')"
        @change="commit('set_main_dock_strip_color', { color: ($event.target as HTMLInputElement).value })"
      />
    </label>

    <!-- 热区 -->
    <label class="setting">
      <span class="setting-text">
        <strong>热区触发厚度</strong>
        <small>40px（鼠标移入屏幕边往内这一段就呼出主窗）</small>
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
        <strong>热区调试底色与透明度</strong>
        <small>0%（默认全透明看不见；调出来可核对热区边界）</small>
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
        <strong>热区边框</strong>
        <small>给热区画一圈边框，调试用（默认关）</small>
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
        <strong>固定显示位置</strong>
        <small>屏幕中间=每次显示都居中；上次位置=记住你最后一次摆放的位置；自由=可随意拖动</small>
      </span>
      <select id="main-window-position"
        :value="s?.mainWindowPosition || 'last'"
        @change="commit('set_main_window_position', { mode: ($event.target as HTMLSelectElement).value })"
      >
        <option value="free">自由拖动</option>
        <option value="center">屏幕中间</option>
        <option value="last">上次位置</option>
      </select>
    </label>

    <!-- 固定显示器（旧版 main-fixed-monitor） -->
    <label class="setting">
      <span class="setting-text">
        <strong>固定显示器</strong>
        <small>选了某台显示器后主窗口只在这台显示器里显示：居中、呼出、贴边都落在它上面，拖到别的显示器会被夹回来；自动 = 不限制</small>
      </span>
      <select
        id="main-fixed-monitor"
        :value="s?.mainFixedMonitor || 'auto'"
        @change="commit('set_main_fixed_monitor', { name: ($event.target as HTMLSelectElement).value })"
      >
        <option value="auto">自动（不固定）</option>
        <option v-for="m in (s?.mainMonitorOptions ?? [])" :key="m" :value="m">{{ m }}</option>
      </select>
    </label>

    <!-- 窗口宽度 / 高度（旧版 window-size-*） -->
    <div class="setting">
      <span class="setting-text">
        <strong>窗口宽度</strong>
        <small>460px（拖动实时调整窗口）</small>
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
        <strong>窗口高度</strong>
        <small>560px（拖动实时调整窗口）</small>
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
        <strong>重置位置</strong>
        <small>把主窗口立即放回屏幕正中央（在「上次位置」模式下也会记忆这个中心点为新的落点）</small>
      </span>
      <button type="button" id="reset-main-window-position" class="secondary" @click="commit('reset_main_window_position', {})">
        重置到屏幕中央
      </button>
    </div>

    <div class="setting">
      <span class="setting-text">
        <strong>重置窗口大小</strong>
        <small>宽回到内嵌单栏的标称值（460）、高回到整屏高度的 2/3（与托盘「重置窗口大小」同源）</small>
      </span>
      <button type="button" id="reset-main-window-size" class="secondary" @click="commit('reset_main_window_size', {})">
        重置窗口大小
      </button>
    </div>

    <label class="setting">
      <span class="setting-text">
        <strong>锁定窗口大小</strong>
        <small>锁定后不能拖拽窗口边框改变大小、也不能最大化；上面的宽高滑条同时停用（双向实时生效）</small>
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
        <strong>隐藏程序（任务栏/Dock）</strong>
        <small>Linux/Windows 不显示任务栏图标，macOS 不显示 Dock 图标</small>
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
        <strong>点击窗口 ❌ 时</strong>
        <small>可在托盘菜单「显示主窗口」中重新打开</small>
      </span>
      <select id="close-action"
        :value="s?.closeAction || 'tray'"
        @change="setCloseAction(($event.target as HTMLSelectElement).value as 'tray' | 'quit')"
      >
        <option value="tray">最小化到托盘</option>
        <option value="quit">退出程序</option>
      </select>
    </label>

    <!-- 托盘菜单样式 -->
    <label class="setting">
      <span class="setting-text">
        <strong>托盘菜单方式</strong>
        <small>各自方式=三端各用系统原生托盘菜单（默认）；全局统一=所有平台右键直接展示同一份菜单内容（Windows/macOS 弹自绘菜单窗，Linux 由桌面环境渲染）。切换后重启应用生效</small>
      </span>
      <select id="tray-menu-style"
        :value="s?.trayMenuStyle === 'unified' ? 'unified' : 'classic'"
        @change="commit('set_tray_menu_style', { mode: ($event.target as HTMLSelectElement).value })"
      >
        <option value="classic">各自方式（默认）</option>
        <option value="unified">全局统一</option>
      </select>
    </label>

    <!-- 主题 -->
    <label class="setting">
      <span class="setting-text">
        <strong>界面主题</strong>
        <small>暗色 / 亮色 / 跟随系统</small>
      </span>
      <select id="theme"
        :value="s?.theme || 'dark'"
        @change="commit('set_theme', { theme: ($event.target as HTMLSelectElement).value })"
      >
        <option value="dark">暗色</option>
        <option value="light">亮色</option>
        <option value="system">跟随系统</option>
      </select>
    </label>

    <!-- 强调色 -->
    <label class="setting">
      <span class="setting-text">
        <strong>主题色调</strong>
        <small>点开调色板选强调色，按钮/链接跟着变色（主窗口和 GRUB 窗口同步）</small>
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
        <strong>窗口不透明度</strong>
        <small>90%（拖动即时预览）</small>
        <small class="setting-state" id="opacity-state">{{ fmtPct(val("opacityPercent", 90)) }}</small>
      </span>
      <input id="opacity"
        type="range"
        min="0"
        max="100"
        :value="val('opacityPercent', 90)"
        @input="setLocal('opacityPercent', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_opacity_percent', { percent: val('opacityPercent', 90) })"
      step="5"
    />
    </label>

    <!-- 远程控制（旧版 section#remote） -->
    <section aria-label="远程控制" class="settings" id="remote">
      <h2>远程控制</h2>

      <div class="setting">
        <span class="setting-text">
          <strong>HTTP 服务监听</strong>
          <small class="setting-state" id="http-state">{{ ui.http?.stateText || "正在读取…" }}</small>
        </span>
        <input id="http-enabled" type="checkbox" :checked="Boolean(ui.http?.listening)" @change="toggleHttp" />
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>监听地址 / 端口</strong>
          <small>默认 0.0.0.0:12300；改成 127.0.0.1 则只能本机或 SSH 隧道访问</small>
        </span>
        <span class="remote-inline">
          <input id="http-bind" v-model="httpBind" spellcheck="false" type="text" />
          <input id="http-port" v-model="httpPort" inputmode="numeric" spellcheck="false" type="text" />
          <button type="button" id="http-apply" class="secondary" @click="applyHttp">应用</button>
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>访问令牌（token）</strong>
          <small class="setting-state mono" id="token-hint">{{ ui.http?.tokenHint || "（未设置）" }}</small>
          <small>自己填一个也可以：4–128 位可见 ASCII（字母/数字/符号），不能有空格和中文；清空则服务不启动</small>
        </span>
        <span class="remote-inline">
          <input id="token-input" v-model="tokenInput" spellcheck="false" type="text" placeholder="自定义 token…" />
          <button type="button" class="secondary" @click="regenerateToken">重新生成</button>
        </span>
      </div>

      <div class="remote-hint">
        <strong>远程怎么调</strong>
        <small>下面这段默认就展开、含明文 token（注意旁人视线）；上面的令牌 / 地址 / 端口一改，这里立刻同步</small>
        <pre id="http-hint">点「复制命令」拿含 token 的可直接执行示例（含 ssh 用法）</pre>
        <div class="actions">
          <button type="button" class="secondary" id="http-hint-copy" @click="copyToken">复制命令</button>
        </div>
      </div>
      <div class="remote-log">
        <strong>最近请求</strong>
        <ol id="http-requests">
          <li v-for="(entry, i) in httpRequests" :key="i">{{ fmtRequest(entry) }}</li>
        </ol>
      </div>

      <div v-if="(ui.http as any)?.occupiedBy" class="port-occupant" id="port-occupant">
        <span class="setting-text">
          <strong>端口被占用</strong>
          <small class="setting-state" id="port-occupant-message">{{ (ui.http as any)?.occupiedBy || "" }}</small>
          <small id="port-occupant-hint">端口被别的程序占用时可以重试或结束占用进程</small>
        </span>
        <span class="remote-inline">
          <button type="button" id="retry-http" class="secondary" @click="retryHttp">重试监听</button>
          <button type="button" id="kill-occupant" class="secondary danger" @click="killOccupant">结束占用进程</button>
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>Token 设置</strong>
          <small>显示 / 复制 / 重新生成访问令牌</small>
        </span>
        <span class="remote-inline">
          <button type="button" id="token-reveal" class="secondary" @click="commit('reveal_http_token', {})">显示</button>
          <button type="button" id="token-copy" class="secondary" @click="copyToken">复制</button>
          <button type="button" id="token-regenerate" class="secondary" @click="regenerateToken">重新生成</button>
          <button type="button" id="token-save" class="secondary" @click="saveToken">保存</button>
        </span>
      </div>

      <!-- 远程请求通知（旧版 notify-enabled 起） -->
      <label class="setting">
        <span class="setting-text">
          <strong>远程请求通知</strong>
          <small>总开关；关掉后不再弹窗，只在主窗口显示待执行请求</small>
        </span>
        <input id="notify-enabled" type="checkbox" :checked="Boolean(s?.notifyEnabled)" @change="commit('set_notify_enabled', { on: !s?.notifyEnabled })" />
      </label>

      <div class="setting">
        <span class="setting-text">
          <strong>远程可触发本机授权框</strong>
          <small>默认关：远程遇到需要本机授权的操作会直接说明原因，不会挂着等人点确认</small>
        </span>
        <input id="remote-allow-gui-auth" type="checkbox" :checked="Boolean(s?.remoteAllowGuiAuth)" @change="commit('set_remote_allow_gui_auth', { on: !s?.remoteAllowGuiAuth })" />
      </div>

      <!-- 通知渠道（旧版 http-channel-* / cli-channel-*） -->
      <div class="setting">
        <span class="setting-text">
          <strong>HTTP 渠道（webhook / curl / 面板）</strong>
          <small>弹窗通知 + 延迟秒数（0 = 立即执行）</small>
        </span>
        <span class="remote-inline">
          <input id="http-channel-notify" type="checkbox" :checked="Boolean(s?.httpChannelNotify)" @change="commit('set_http_channel_notify', { on: !s?.httpChannelNotify })" />
          <input id="http-channel-delay" type="text" inputmode="numeric" spellcheck="false" :value="val('httpChannelDelay', 0)" @change="commit('set_http_channel_delay', { sec: Number(($event.target as HTMLInputElement).value) })" />
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>SSH / CLI 渠道</strong>
          <small>ssh 远程调用的通知与延迟（与 webhook 分开配置）</small>
        </span>
        <span class="remote-inline">
          <input id="cli-channel-notify" type="checkbox" :checked="Boolean(s?.cliChannelNotify)" @change="commit('set_cli_channel_notify', { on: !s?.cliChannelNotify })" />
          <input id="cli-channel-delay" type="text" inputmode="numeric" spellcheck="false" :value="val('cliChannelDelay', 0)" @change="commit('set_cli_channel_delay', { sec: Number(($event.target as HTMLInputElement).value) })" />
        </span>
      </div>

      <!-- 提示音（旧版 setting-sound / play-sound / sound-file-path / sound-apply） -->
      <div class="setting">
        <span class="setting-text">
          <strong>提示音</strong>
          <small>收到远程请求时播放提示音（系统默认或自定义文件）</small>
        </span>
        <span class="remote-inline">
          <label class="setting" style="border: 0; padding: 0">
            <input id="setting-sound" type="checkbox" :checked="Boolean(s?.soundEnabled)" @change="commit('set_sound_enabled', { on: !s?.soundEnabled })" />
          </label>
          <button type="button" id="play-sound" class="secondary" @click="commit('play_sound', {})">试听</button>
          <input id="sound-file-path" v-model="soundPath" spellcheck="false" type="text" placeholder="自定义路径（留空用内置音）" />
          <button type="button" id="sound-apply" class="secondary" @click="commit('set_sound_file', { path: soundPath })">应用</button>
        </span>
      </div>

      <!-- 打开配置目录（旧版 open-dir-actions / open-config-dir） -->
      <div class="setting" id="open-dir-actions">
        <button type="button" id="open-config-dir" class="secondary" @click="commit('open_config_dir', {})">打开配置目录（含 trace.log / settings.json）</button>
      </div>

      <!-- 待确认卡（旧版 .pending#pending-card） -->
      <div v-if="(ui.state as any)?.pending" class="pending" id="pending-card">
        <strong id="pending-title">远程请求待执行</strong>
        <p id="pending-detail">{{ (ui.state as any).pending.text }}</p>
        <span class="remote-inline">
          <button type="button" id="pending-run" class="primary-sol" @click="commit('pending_confirm', {})">执行</button>
          <button type="button" id="pending-cancel" class="secondary" @click="commit('pending_cancel', {})">取消</button>
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>打开 Web 页面</strong>
          <small>用系统默认浏览器打开控制页，自动带上访问令牌，不用手输</small>
        </span>
        <button type="button" id="open-web" class="secondary" @click="openWeb">打开</button>
      </div>

      <label class="setting">
        <span class="setting-text">
          <strong>允许远程图形授权</strong>
          <small>远程请求转本机确认时，是否允许在远程界面上点确认</small>
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
