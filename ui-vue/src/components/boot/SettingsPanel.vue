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
  commit("open_web_page", { url: `http://127.0.0.1:${httpPort.value}` });
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
        @change="commit('set_main_always_on_top', { on: !s?.mainAlwaysOnTop })"
      />
    </label>

    <!-- 贴边自动隐藏 -->
    <label class="setting">
      <span class="setting-text">
        <strong>贴边自动隐藏</strong>
        <small>窗口拖到屏幕边缘时隐藏成一条缝隙，鼠标移上去再展开</small>
        <small class="setting-state" id="main-auto-hide-state">{{ s?.mainAutoHide ? "已开启" : "已关闭" }}</small>
      </span>
      <input id="main-auto-hide"
        type="checkbox"
        :checked="Boolean(s?.mainAutoHide)"
        @change="commit('set_main_auto_hide', { on: !s?.mainAutoHide })"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>贴边触发距离</strong>
        <small>鼠标离屏幕边缘多少像素内算「贴上去」</small>
      </span>
      <input id="main-auto-hide-edge-distance"
        type="range"
        min="0"
        max="60"
        :value="val('mainAutoHideEdgeDistance', 24)"
        @input="setLocal('mainAutoHideEdgeDistance', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_auto_hide_edge_distance', { px: val('mainAutoHideEdgeDistance', 24) })"
      />
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
        @change="commit('set_main_dock_strip_hover_ms', { ms: Number(($event.target as HTMLInputElement).value) })"
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
        @change="commit('set_main_dock_strip_edge_gap', { px: Number(($event.target as HTMLInputElement).value) })"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>空隙白条厚度</strong>
        <small>贴边隐藏时那条缝的粗细</small>
        <small class="setting-state" id="main-dock-strip-thickness-state">{{ val("mainDockStripThickness", 3) }} px</small>
      </span>
      <input id="main-dock-strip-thickness"
        type="range"
        min="2"
        max="12"
        :value="val('mainDockStripThickness', 3)"
        @input="setLocal('mainDockStripThickness', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_dock_strip_thickness', { px: val('mainDockStripThickness', 3) })"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>白条长度</strong>
        <small>白条占屏幕边缘的比例</small>
        <small class="setting-state" id="main-dock-strip-length-state">{{ fmtPct(val("mainDockStripLength", 100)) }}</small>
      </span>
      <input id="main-dock-strip-length"
        type="range"
        min="10"
        max="100"
        :value="val('mainDockStripLength', 100)"
        @input="setLocal('mainDockStripLength', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_dock_strip_length', { percent: val('mainDockStripLength', 100) })"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>白条不透明度</strong>
        <small>越低越透明</small>
        <small class="setting-state" id="main-dock-strip-opacity-state">{{ fmtPct(val("mainDockStripOpacity", 100)) }}</small>
      </span>
      <input id="main-dock-strip-opacity"
        type="range"
        min="0"
        max="100"
        :value="val('mainDockStripOpacity', 100)"
        @input="setLocal('mainDockStripOpacity', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_dock_strip_opacity', { percent: val('mainDockStripOpacity', 100) })"
      />
    </label>

    <label class="setting">
      <span class="setting-text">
        <strong>白条颜色</strong>
        <small>贴边缝隙的颜色</small>
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
        <strong>热区厚度</strong>
        <small>鼠标移到贴边处多宽的范围内会呼出窗口</small>
        <small class="setting-state" id="main-hotzone-thickness-state">{{ val("mainHotzoneThickness", 8) }} px</small>
      </span>
      <input id="main-hotzone-thickness"
        type="range"
        min="2"
        max="40"
        :value="val('mainHotzoneThickness', 8)"
        @input="setLocal('mainHotzoneThickness', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_main_hotzone_thickness', { px: val('mainHotzoneThickness', 8) })"
      />
    </label>

    <div class="setting">
      <span class="setting-text">
        <strong>热区调试底色与透明度</strong>
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
          @change="commit('set_main_hotzone_opacity', { percent: val('mainHotzoneOpacity', 0) })"
        />
      </span>
    </div>

    <label class="setting">
      <span class="setting-text">
        <strong>热区可视边框</strong>
        <small>调试用：把「鼠标移入即呼出」的触发带画出来</small>
      </span>
      <input id="main-hotzone-border"        type="checkbox"
        :checked="Boolean(s?.mainHotzoneBorder)"
        @change="commit('set_main_hotzone_border', { on: !s?.mainHotzoneBorder })"
      />
      <input id="main-hotzone-border-color" type="color" :value="hotzoneBorderColor" @input="setHotzoneBorderColor(($event.target as HTMLInputElement).value)" />
    </label>

    <!-- 窗口位置 -->
    <label class="setting">
      <span class="setting-text">
        <strong>窗口位置</strong>
        <small>记住上次位置 / 固定到某块屏幕</small>
      </span>
      <select id="main-window-position"
        :value="s?.mainWindowPosition || 'last'"
        @change="commit('set_main_window_position', { value: ($event.target as HTMLSelectElement).value })"
      >
        <option value="last">记住上次位置</option>
        <option value="center">每次居中</option>
        <option value="fixed">固定屏幕</option>
      </select>
    </label>

    <!-- 固定显示器（旧版 main-fixed-monitor） -->
    <label class="setting">
      <span class="setting-text">
        <strong>固定显示器</strong>
        <small>固定到某块屏幕（多显示器时）</small>
      </span>
      <input id="main-fixed-monitor" type="checkbox" :checked="Boolean(s?.mainFixedMonitor)" @change="commit('set_main_fixed_monitor', { on: !s?.mainFixedMonitor })" />
    </label>

    <!-- 窗口宽度 / 高度（旧版 window-size-*） -->
    <div class="setting">
      <span class="setting-text">
        <strong>窗口宽度</strong>
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
          @change="commit('set_window_size_width', { px: val('windowSizeWidth', 460) })"
        />
      </span>
    </div>

    <div class="setting">
      <span class="setting-text">
        <strong>窗口高度</strong>
        <small class="setting-state" id="window-size-height-state">{{ val("windowSizeHeight", 720) }} px</small>
      </span>
      <span class="remote-inline">
        <input
          id="window-size-height"
          type="range"
          min="320"
          max="1200"
          step="4"
          :value="val('windowSizeHeight', 720)"
          @input="setLocal('windowSizeHeight', Number(($event.target as HTMLInputElement).value))"
          @change="commit('set_window_size_height', { px: val('windowSizeHeight', 720) })"
        />
      </span>
    </div>

    <div class="setting">
      <span class="setting-text">
        <strong>窗口尺寸</strong>
        <small>取消「锁定尺寸」后可直接拖拽窗口边缘调整</small>
      </span>
      <div class="setting-actions">
        <button type="button" id="reset-main-window-position" class="secondary" @click="commit('reset_main_window_position', {})">
          重置位置
        </button>
        <button type="button" id="reset-main-window-size" class="secondary" @click="commit('reset_main_window_size', {})">
          重置尺寸
        </button>
      </div>
    </div>

    <label class="setting">
      <span class="setting-text">
        <strong>锁定窗口尺寸</strong>
        <small>禁止拖拽改变窗口大小</small>
      </span>
      <input id="window-size-locked"
        type="checkbox"
        :checked="Boolean(s?.windowSizeLocked)"
        @change="commit('set_window_size_locked', { on: !s?.windowSizeLocked })"
      />
    </label>

    <!-- Dock / 任务栏 -->
    <label class="setting">
      <span class="setting-text">
        <strong>从 Dock / 任务栏隐藏</strong>
        <small>只在托盘显示图标，不占 Dock / 任务栏</small>
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
        <strong>关闭窗口时</strong>
        <small>点窗口关闭按钮之后的动作</small>
      </span>
      <select id="close-action"
        :value="s?.closeAction || 'tray'"
        @change="setCloseAction(($event.target as HTMLSelectElement).value as 'tray' | 'quit')"
      >
        <option value="tray">收进托盘（继续后台运行）</option>
        <option value="quit">退出程序</option>
      </select>
    </label>

    <!-- 托盘菜单样式 -->
    <label class="setting">
      <span class="setting-text">
        <strong>托盘菜单样式</strong>
        <small>原生菜单 / 自绘卡片</small>
      </span>
      <select id="tray-menu-style"
        :value="s?.trayMenuStyle || 'native'"
        @change="commit('set_tray_menu_style', { value: ($event.target as HTMLSelectElement).value })"
      >
        <option value="native">原生菜单</option>
        <option value="custom">自绘卡片</option>
      </select>
    </label>

    <!-- 主题 -->
    <label class="setting">
      <span class="setting-text">
        <strong>主题</strong>
        <small>暗色 / 亮色 / 跟随系统</small>
      </span>
      <select id="theme"
        :value="s?.theme || 'dark'"
        @change="commit('set_theme', { value: ($event.target as HTMLSelectElement).value })"
      >
        <option value="dark">暗色</option>
        <option value="light">亮色</option>
        <option value="system">跟随系统</option>
      </select>
    </label>

    <!-- 强调色 -->
    <label class="setting">
      <span class="setting-text">
        <strong>强调色</strong>
        <small>按钮、选中态、链接的基色</small>
      </span>
      <input id="accent-color"
        type="color"
        :value="s?.accentColor || '#4f46e5'"
        @change="commit('set_accent_color', { color: ($event.target as HTMLInputElement).value })"
      />
    </label>

    <!-- 不透明度 -->
    <label class="setting">
      <span class="setting-text">
        <strong>窗口不透明度</strong>
        <small>整窗背景的透明程度</small>
        <small class="setting-state" id="opacity-state">{{ fmtPct(val("opacityPercent", 90)) }}</small>
      </span>
      <input id="opacity"
        type="range"
        min="20"
        max="100"
        :value="val('opacityPercent', 90)"
        @input="setLocal('opacityPercent', Number(($event.target as HTMLInputElement).value))"
        @change="commit('set_opacity_percent', { percent: val('opacityPercent', 90) })"
      />
    </label>

    <!-- 远程控制（旧版 section#remote） -->
    <section aria-label="远程控制" class="settings" id="remote">
      <h2>远程控制</h2>

      <div class="setting">
        <span class="setting-text">
          <strong>HTTP 服务</strong>
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
          <strong>访问令牌</strong>
          <small class="setting-state mono" id="token-hint">{{ ui.http?.tokenHint || "（未设置）" }}</small>
        </span>
        <span class="remote-inline">
          <input id="token-input" v-model="tokenInput" spellcheck="false" type="text" placeholder="新 token（≥8 位），留空不改动" />
          <button type="button" class="secondary" @click="regenerateToken">重新生成</button>
        </span>
      </div>

      <small class="setting-hint" id="http-hint">
        手机 / 浏览器访问 http://&lt;本机 IP&gt;:端口/?token=令牌 —— <span id="http-requests">远程请求会转本机确认</span>
        <button type="button" id="http-hint-copy" class="link-btn" @click="copyToken">复制访问地址</button>
      </small>

      <div v-if="(ui.http as any)?.occupiedBy" class="port-occupant" id="port-occupant">
        <span class="setting-text">
          <strong>端口占用</strong>
          <small class="setting-state" id="port-occupant-message">{{ (ui.http as any)?.occupiedBy || "" }}</small>
          <small id="port-occupant-hint">端口被别的程序占用时可以重试或结束占用进程</small>
        </span>
        <span class="remote-inline">
          <button type="button" id="retry-http" class="secondary" @click="retryHttp">重试</button>
          <button type="button" id="kill-occupant" class="secondary danger" @click="killOccupant">结束占用进程</button>
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>令牌操作</strong>
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
          <small>远程发起授权请求时在本机弹通知</small>
        </span>
        <input id="notify-enabled" type="checkbox" :checked="Boolean(s?.notifyEnabled)" @change="commit('set_notify_enabled', { on: !s?.notifyEnabled })" />
      </label>

      <div class="setting">
        <span class="setting-text">
          <strong>远程请求触发本机授权框</strong>
          <small>远程请求转本机确认时，是否弹出授权框</small>
        </span>
        <input id="remote-allow-gui-auth" type="checkbox" :checked="Boolean(s?.remoteAllowGuiAuth)" @change="commit('set_remote_allow_gui_auth', { on: !s?.remoteAllowGuiAuth })" />
      </div>

      <!-- 通知渠道（旧版 http-channel-* / cli-channel-*） -->
      <div class="setting">
        <span class="setting-text">
          <strong>HTTP 渠道通知</strong>
          <small>通过 HTTP 服务发来的请求是否通知</small>
        </span>
        <span class="remote-inline">
          <input id="http-channel-notify" type="checkbox" :checked="Boolean(s?.httpChannelNotify)" @change="commit('set_http_channel_notify', { on: !s?.httpChannelNotify })" />
          <input id="http-channel-delay" type="number" min="0" max="60" :value="val('httpChannelDelay', 0)" @change="commit('set_http_channel_delay', { sec: Number(($event.target as HTMLInputElement).value) })" />
        </span>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>CLI 渠道通知</strong>
          <small>命令行发来的请求是否通知</small>
        </span>
        <span class="remote-inline">
          <input id="cli-channel-notify" type="checkbox" :checked="Boolean(s?.cliChannelNotify)" @change="commit('set_cli_channel_notify', { on: !s?.cliChannelNotify })" />
          <input id="cli-channel-delay" type="number" min="0" max="60" :value="val('cliChannelDelay', 0)" @change="commit('set_cli_channel_delay', { sec: Number(($event.target as HTMLInputElement).value) })" />
        </span>
      </div>

      <!-- 提示音（旧版 setting-sound / play-sound / sound-file-path / sound-apply） -->
      <div class="setting">
        <span class="setting-text">
          <strong>提示音</strong>
          <small>远程请求到达时播放的声音</small>
        </span>
        <span class="remote-inline">
          <label class="setting" style="border: 0; padding: 0">
            <input id="setting-sound" type="checkbox" :checked="Boolean(s?.soundEnabled)" @change="commit('set_sound_enabled', { on: !s?.soundEnabled })" />
          </label>
          <button type="button" id="play-sound" class="secondary" @click="commit('play_sound', {})">试听</button>
          <input id="sound-file-path" v-model="soundPath" spellcheck="false" type="text" placeholder="自定义音频文件路径" />
          <button type="button" id="sound-apply" class="secondary" @click="commit('set_sound_file', { path: soundPath })">应用</button>
        </span>
      </div>

      <!-- 打开配置目录（旧版 open-dir-actions / open-config-dir） -->
      <div class="setting" id="open-dir-actions">
        <span class="setting-text">
          <strong>配置目录</strong>
          <small>设置、密钥、备份都存在这里</small>
        </span>
        <button type="button" id="open-config-dir" class="secondary" @click="commit('open_config_dir', {})">打开</button>
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
          <strong>在浏览器打开</strong>
          <small>用系统默认浏览器打开本机远程管理页</small>
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
