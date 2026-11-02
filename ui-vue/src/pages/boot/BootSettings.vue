<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useBootAdvStore, type LoaderId } from "@/stores/bootAdv";
import { loaderMeta } from "@/pages/boot/themes/loaders";
import { THEME_LABELS, THEME_LAYOUT_MODES, type LayoutMode } from "@/pages/boot/themes/registry";
import { setAutostart, setCloseAction, setHideFromDockTaskbar, setStartInTray, setVarsDir, unmountTemporary } from "@/api/tauri";

/**
 * 引导高级设置面板（原 `ui/boot-adv.js` 的设置页部分 + `index.html` 的设置区）。
 *
 * 三块：
 *   ① **版式主题**：K / A / B / C / G / I / J + 布局档位（部分主题只有加宽）
 *   ② **启用与排序**：勾选哪些引导器、上下移动
 *   ③ **应用设置**：vars 目录、开机自启、收进托盘、关闭行为、临时挂载
 *
 * 主题与排序存在 localStorage（`stores/bootAdv`），应用设置走后端命令。
 */

const ui = useUiStateStore();
const adv = useBootAdvStore();

const busy = ref("");
const message = ref("");
const errorText = ref("");

const varsDirInput = ref("");

onMounted(() => {
  varsDirInput.value = ui.status?.varsDir ?? "";
});

/** 当前主题可用的布局档位 */
const layouts = computed<LayoutMode[]>(
  () => THEME_LAYOUT_MODES[adv.effectiveTheme] ?? ["default"],
);

const layoutLabels: Record<LayoutMode, string> = {
  default: "默认",
  wide: "加宽",
  lr: "左右布局",
};

async function run(label: string, action: () => Promise<unknown>, done: string) {
  if (busy.value) return;
  busy.value = label;
  message.value = "";
  errorText.value = "";
  try {
    await action();
    message.value = done;
  } catch (error) {
    errorText.value = String(error);
  } finally {
    busy.value = "";
  }
}

function applyVarsDir() {
  const dir = varsDirInput.value.trim();
  void run("保存 vars 目录", () => setVarsDir(dir), "vars 目录已保存");
}

/** 引导器 ESP 展示值（旧版 l.esp）：refind/grub 固定，三通道取 channel.detail */
function loaderEsp(id: string): string {
  if (id === "refind") return "ESP · refind.conf";
  if (id === "grub") return "vars/PreviousBoot 快照";
  const ch = (ui.channels || []).find((c) => c.id === id);
  return ch?.detail || "";
}

/** 切引导器左右列（旧版 k-rg） */
function moveSide(id: string) {
  const cur = adv.state.side[id] === "r" ? "r" : "l";
  adv.setSide(id, cur === "r" ? "l" : "r");
}

function toggleVisible(id: LoaderId) {
  adv.setVisible(id, !adv.state.visible[id]);
}


function setClose(action: "tray" | "quit") {
  void run("保存关闭行为", () => setCloseAction(action), `关闭窗口 = ${action === "tray" ? "收进托盘" : "退出"}`);
}
</script>

<template>
  <div class="settings-page">
    <!-- ① 版式主题 -->
    <section class="card">
      <header><h2>版式主题</h2></header>
      <div class="themes">
        <button
          v-for="item in THEME_LABELS"
          :key="item.key"
          type="button"
          :class="['theme-btn', { 'is-on': adv.effectiveTheme === item.key }]"
          @click="adv.setTheme(item.key)"
        >
          {{ item.label }}
        </button>
      </div>
      <div v-if="layouts.length > 1" class="layouts">
        <span class="lbl">布局</span>
        <button
          v-for="mode in layouts"
          :key="mode"
          type="button"
          :class="['theme-btn', { 'is-on': adv.effectiveLayout === mode }]"
          @click="adv.setLayout(mode)"
        >
          {{ layoutLabels[mode] }}
        </button>
      </div>
      <p v-else class="hint">这个主题只有「{{ layoutLabels[layouts[0] ?? "default"] }}」一种布局</p>
    </section>

    <!-- ② 启用与排序 -->
    <section class="card">
      <header><h2>启用引导器</h2></header>
      <div class="k-tabs">
        <button
          v-for="id in adv.state.order"
          :key="id"
          type="button"
          :class="['k-tab', { 'is-on': adv.state.visible[id] }]"
          title="点击 开/关 展示该引导器"
          @click="toggleVisible(id)"
        >
          <span>{{ loaderMeta(id).name }}</span>
          <span
            class="k-rg"
            :title="'点击挪到另一列（没开左右布局时，点它自动切过去）'"
            @click.stop="moveSide(id)"
          >{{ adv.state.side[id] === 'r' ? '右' : '左' }}</span>
        </button>
        <span class="grow" />
        <button type="button" class="btn xs ghost" title="回到默认编排" @click="adv.resetOrder()">重置</button>
      </div>
      <p class="hint">顺序决定主题里卡片的排列；左右布局时点标签上的「左/右」分列。</p>
    </section>

    <!-- 菜单样式（旧版 boot-adv row4） -->
    <section class="card">
      <header><h2>菜单样式</h2></header>
      <div class="seg">
        <button type="button" :class="{ 'is-on': adv.state.style === 'full' }" title="完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮" @click="adv.setStyle('full')">完整</button>
        <button type="button" :class="{ 'is-on': adv.state.style === 'compact' }" title="完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮" @click="adv.setStyle('compact')">紧凑</button>
        <button type="button" :class="{ 'is-on': adv.state.style === 'min' }" title="完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮" @click="adv.setStyle('min')">极简</button>
      </div>
      <p class="hint">三档对任何版式主题都生效；行内动作按钮在紧凑/极简下悬停才出现。</p>
    </section>

    <!-- 下次启动区域显示方式（旧版 boot-adv row4b） -->
    <section class="card">
      <header><h2>下次启动</h2></header>
      <div class="seg">
        <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'off' }" title="不显示：整块从界面移除（不是留个空壳），下面引导列表照常可用 —— 出厂默认就是这一档" @click="adv.setNextBoot('off')">不显示</button>
        <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'fold' }" @click="adv.setNextBoot('fold')">折叠框</button>
        <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'open' }" @click="adv.setNextBoot('open')">展开</button>
      </div>
      <p class="hint">「系统引导」页顶部那块：不显示 = 整块移除、不占任何高度（默认）；折叠框 = 默认收起，标题旁小按钮切换；展开 = 打开就看到全部条目，同样可收起。</p>
    </section>

    <!-- 引导器设置（旧版 boot-adv k-setlist 汇总） -->
    <section class="card">
      <header><h2>引导器设置</h2></header>
      <ul class="loader-set-list">
        <li v-for="id in adv.state.order.filter((x) => adv.state.visible[x])" :key="id" class="loader-set-row">
          <span class="nm">{{ loaderMeta(id).name }}</span>
          <span class="chip">ESP · {{ loaderEsp(id) || '未知' }}</span>
          <span class="chip">默认项 · 未知</span>
          <span class="chip">脚本 · {{ id === 'grub' ? '09_anyboot' : '不适用' }}</span>
          <span class="chip">免密 · {{ id === 'opencore' ? '未开启' : '不适用' }}</span>
          <span class="spacer" />
          <button type="button" class="btn icon ghost sm" :title="'打开 ' + loaderMeta(id).name + ' 详细设置'">⚙</button>
        </li>
        <li v-if="!adv.state.order.some((x) => adv.state.visible[x])" class="hint">没有启用的引导器 —— 点上方标签开启后再设置</li>
      </ul>
    </section>

    <!-- ④ 应用设置 -->
    <section class="card">
      <header><h2>应用设置</h2></header>

      <div class="row">
        <label>变量目录</label>
        <input v-model="varsDirInput" class="mono" placeholder="留空 = 自动探测" />
        <button type="button" class="btn sm" :disabled="!!busy" @click="applyVarsDir">保存</button>
      </div>

      <div class="row">
        <label>开机启动</label>
        <input
          type="checkbox"
          :checked="Boolean(ui.settings?.autostart)"
          @change="run('保存自启', () => setAutostart(!ui.settings?.autostart), '已保存')"
        />
      </div>

      <label class="setting">
        <span class="setting-text">
          <strong>启动时隐藏主界面</strong>
          <small>启动后不直接打开主菜单，仅显示在托盘</small>
        </span>
        <input
          id="start-in-tray"
          type="checkbox"
          :checked="Boolean(ui.settings?.startInTray)"
          @change="run('保存托盘', () => setStartInTray(!ui.settings?.startInTray), '已保存')"
        />
      </label>

      <label class="setting">
        <span class="setting-text">
          <strong>隐藏程序（任务栏/Dock）</strong>
          <small>Linux/Windows 不显示任务栏图标，macOS 不显示 Dock 图标</small>
        </span>
        <input
          id="hide-from-dock"
          type="checkbox"
          :checked="Boolean(ui.settings?.hideFromDockTaskbar)"
          @change="
            run('保存 Dock', () => setHideFromDockTaskbar(!ui.settings?.hideFromDockTaskbar), '已保存')
          "
        />
      </label>

      <div class="setting">
        <span class="setting-text">
          <strong>点击窗口 ❌ 时</strong>
          <small>可在托盘菜单「显示主窗口」中重新打开</small>
        </span>
        <button type="button" class="btn sm" @click="setClose('tray')">最小化到托盘</button>
        <button type="button" class="btn sm" @click="setClose('quit')">退出程序</button>
      </div>

      <div v-if="ui.status?.temporaryMount" class="row">
        <label>临时挂载</label>
        <button
          type="button"
          class="btn sm"
          :disabled="!!busy"
          @click="run('卸载临时挂载', () => unmountTemporary(), '已卸载')"
        >
          卸载
        </button>
      </div>
    </section>

    <p v-if="message" class="ok">{{ message }}</p>
    <p v-if="errorText" class="err">{{ errorText }}</p>
  </div>
</template>

<style scoped>
.settings-page {
  display: flex;
  flex-direction: column;
  gap: 14px;
  max-width: 760px;
}
.card {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  background: var(--panel, #fff);
  padding: 16px 18px;
}
header {
  margin-bottom: 12px;
}
h2 {
  font-size: 14px;
  margin: 0;
}
.themes,
.layouts {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  align-items: center;
  margin-bottom: 8px;
}
.theme-btn {
  border: 1px solid var(--line, #d1d5db);
  background: var(--panel, #fff);
  border-radius: 999px;
  padding: 5px 13px;
  font-size: 12px;
  cursor: pointer;
  color: var(--fg-dim, #4b5563);
}
.theme-btn.is-on {
  background: var(--accent, #2563eb);
  border-color: var(--accent, #2563eb);
  color: #fff;
}
.lbl {
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
  margin-right: 4px;
}
.loader-list {
  list-style: none;
  padding: 0;
  margin: 0 0 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.loader-list li {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 7px 10px;
  border: 1px solid var(--line, #f1f5f9);
  border-radius: 8px;
  font-size: 13px;
}
.spacer {
  flex: 1;
}
.row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
  font-size: 12.5px;
}
.row label {
  width: 110px;
  color: var(--fg-faint, #9ca3af);
  flex: none;
}
.row input[type="text"],
.row input:not([type]) {
  flex: 1;
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
  flex: 1;
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 7px;
  padding: 6px 10px;
}
.hint {
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
  margin: 0;
}
.ok {
  font-size: 12.5px;
  color: #15803d;
  margin: 0;
}
.err {
  font-size: 12.5px;
  color: #dc2626;
  margin: 0;
}
</style>
