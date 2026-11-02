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

    <label class="setting">
      <span class="setting-text">
        <strong>热区可视边框</strong>
        <small>调试用：把「鼠标移入即呼出」的触发带画出来</small>
      </span>
      <input id="main-hotzone-border"
        type="checkbox"
        :checked="Boolean(s?.mainHotzoneBorder)"
        @change="commit('set_main_hotzone_border', { on: !s?.mainHotzoneBorder })"
      />
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

    <div class="setting">
      <span class="setting-text">
        <strong>窗口尺寸</strong>
        <small>取消「锁定尺寸」后可直接拖拽窗口边缘调整</small>
      </span>
      <div class="setting-actions">
        <button type="button" class="secondary" @click="commit('reset_main_window_position', {})">
          重置位置
        </button>
        <button type="button" class="secondary" @click="commit('reset_main_window_size', {})">
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

      <div class="setting">
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
