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

function toggleVisible(id: LoaderId) {
  adv.setVisible(id, !adv.state.visible[id]);
}

function move(id: LoaderId, delta: number) {
  const order = [...adv.state.order];
  const index = order.indexOf(id);
  const next = index + delta;
  if (index < 0 || next < 0 || next >= order.length) return;
  [order[index], order[next]] = [order[next]!, order[index]!];
  adv.setOrder(order);
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
      <header><h2>显示哪些引导器</h2></header>
      <ul class="loader-list">
        <li v-for="id in adv.state.order" :key="id">
          <input
            type="checkbox"
            :checked="adv.state.visible[id]"
            @change="toggleVisible(id)"
          />
          <span class="nm">{{ loaderMeta(id).name }}</span>
          <span class="spacer" />
          <button type="button" class="btn icon ghost sm" title="上移" @click="move(id, -1)">▴</button>
          <button type="button" class="btn icon ghost sm" title="下移" @click="move(id, 1)">▾</button>
        </li>
      </ul>
      <p class="hint">顺序决定主题里卡片的排列；左右布局时还能在主题里拖动分配列。</p>
    </section>

    <!-- ③ 应用设置 -->
    <section class="card">
      <header><h2>应用设置</h2></header>

      <div class="row">
        <label>rEFInd vars 目录</label>
        <input v-model="varsDirInput" class="mono" placeholder="留空 = 自动探测" />
        <button type="button" class="btn sm" :disabled="!!busy" @click="applyVarsDir">保存</button>
      </div>

      <div class="row">
        <label>开机自启</label>
        <input
          type="checkbox"
          :checked="Boolean(ui.settings?.autostart)"
          @change="run('保存自启', () => setAutostart(!ui.settings?.autostart), '已保存')"
        />
      </div>

      <div class="row">
        <label>启动时收进托盘</label>
        <input
          type="checkbox"
          :checked="Boolean(ui.settings?.startInTray)"
          @change="run('保存托盘', () => setStartInTray(!ui.settings?.startInTray), '已保存')"
        />
      </div>

      <div class="row">
        <label>从 Dock 隐藏</label>
        <input
          type="checkbox"
          :checked="Boolean(ui.settings?.hideFromDockTaskbar)"
          @change="
            run('保存 Dock', () => setHideFromDockTaskbar(!ui.settings?.hideFromDockTaskbar), '已保存')
          "
        />
      </div>

      <div class="row">
        <label>关闭窗口时</label>
        <button type="button" class="btn sm" @click="setClose('tray')">收进托盘</button>
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
