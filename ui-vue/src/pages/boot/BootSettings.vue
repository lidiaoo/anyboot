<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useBootAdvStore, type LoaderId } from "@/stores/bootAdv";
import { loaderMeta } from "@/pages/boot/themes/loaders";
import ThemeHost from "@/pages/boot/themes/index.vue";
import { THEME_LABELS, THEME_LAYOUT_MODES, type LayoutMode } from "@/pages/boot/themes/registry";

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

const setListOpen = ref(false);
/** 旧版 moveLoader（boot-adv.js）：order 内上/下移一位 */
function moveLoader(id: LoaderId, delta: number) {
  const order = [...adv.state.order];
  const i = order.indexOf(id);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= order.length) return;
  order.splice(j, 0, ...order.splice(i, 1));
  adv.setOrder(order);
}
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


</script>

<template>
  <div class="settings-page">
    <!-- 旧版 renderAdvTab：div.k > div.k-bar（行式：k-row × N + k-tabs + sortbar + setlist） -->
    <div class="k">
      <div class="k-bar">
        <!-- row1：启用引导器（.k-tabs 芯片行 + 重置） -->
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
              title="点击挪到另一列（没开左右布局时，点它自动切过去）"
              @click.stop="moveSide(id)"
            >{{ adv.state.side[id] === 'r' ? '右' : '左' }}</span>
          </button>
          <span class="grow" />
          <button type="button" class="btn xs ghost" title="回到默认编排" @click="adv.resetOrder()">重置</button>
        </div>
        <!-- row2：版式主题 -->
        <div class="k-row">
          <span class="k-lbl">版式主题</span>
          <div class="seg">
            <button
              v-for="item in THEME_LABELS"
              :key="item.key"
              type="button"
              :class="{ 'is-on': adv.effectiveTheme === item.key }"
              :title="item.key === 'K' ? '不套用 A–J 版式：预演区用本页的引导器卡片（区域编排：启用/左右分栏/排序），仅此页有这种形态' : '引导菜单整体用方案 ' + item.key + ' 的版式；选好后列内排序照常生效'"
              @click="adv.setTheme(item.key)"
            >
              {{ item.label }}
            </button>
          </div>
          <span class="hint">主题决定每张卡怎么画；B / G / J 是加宽版面，选中即把窗口加宽到 932。</span>
        </div>
        <!-- row3：布局 -->
        <div class="k-row">
          <span class="k-lbl">布局</span>
          <div class="seg">
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
          <span v-if="layouts.length > 1" class="hint">默认 = 单列竖排（460）；加宽 = 窗口 932、列表仍单列；左右布局 = 两列各自滚动，点标签上的「左/右」分列、拖分隔条定占比（中间圆钮或双击回正中）——仅「不套用主题」和 A / C；B / G / J 只有加宽，I 没有左右布局（置灰的档位选择仍会记着，换回支持的主题自动生效）。</span>
          <span v-else class="hint">这个主题只有「{{ layoutLabels[layouts[0] ?? "default"] }}」一种布局</span>
        </div>
        <!-- row4：菜单样式 -->
        <div class="k-row">
          <span class="k-lbl">菜单样式</span>
          <div class="seg">
            <button type="button" :class="{ 'is-on': adv.state.style === 'full' }" title="完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮" @click="adv.setStyle('full')">完整</button>
            <button type="button" :class="{ 'is-on': adv.state.style === 'compact' }" title="完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮" @click="adv.setStyle('compact')">紧凑</button>
            <button type="button" :class="{ 'is-on': adv.state.style === 'min' }" title="完整：路径 + 徽章 + 全部动作；紧凑：行距减半、隐去路径；极简：只剩图标 + 名称 + 默认高亮" @click="adv.setStyle('min')">极简</button>
          </div>
          <span class="hint">三档对任何版式主题都生效；行内动作按钮在紧凑/极简下悬停才出现。</span>
        </div>
        <!-- row4b：下次启动 -->
        <div class="k-row">
          <span class="k-lbl">下次启动</span>
          <div class="seg">
            <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'off' }" title="不显示：整块从界面移除（不是留个空壳），下面引导列表照常可用 —— 出厂默认就是这一档" @click="adv.setNextBoot('off')">不显示</button>
            <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'fold' }" @click="adv.setNextBoot('fold')">折叠框</button>
            <button type="button" :class="{ 'is-on': adv.state.nextBoot === 'open' }" @click="adv.setNextBoot('open')">展开</button>
          </div>
          <span class="hint">「系统引导」页顶部那块：不显示 = 整块移除、不占任何高度（默认）；折叠框 = 默认收起，标题旁小按钮切换；展开 = 打开就看到全部条目，同样可收起。</span>
        </div>
        <!-- row5：上下排序（k-sortbar） -->
        <div class="k-sortbar">
          <span class="k-sort-lbl">上下排序</span>
          <span v-for="(id, i) in adv.state.order" :key="id" class="k-sortchip">
            <span class="k-sort-nm">{{ loaderMeta(id).name }}</span>
            <button type="button" class="k-zop" title="上移一位" :disabled="i === 0" @click="moveLoader(id, -1)">↑</button>
            <button type="button" class="k-zop" title="下移一位" :disabled="i === adv.state.order.length - 1" @click="moveLoader(id, 1)">↓</button>
          </span>
        </div>
        <!-- 引导器设置（k-setlist 折叠条） -->
        <div class="k-setbar" @click="setListOpen = !setListOpen">
          <span class="k-setbar-t">※ 引导器设置 <small>ESP 路径 · 权限 · 默认项 · 脚本 · 免密模式</small></span>
          <span class="chevron" :class="{ open: setListOpen }">⌄</span>
        </div>
        <div v-show="setListOpen" class="k-setlist">
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
        </div>
      </div>
    </div>
    <!-- 预演面板（旧版：bootThemeStage → .k-stage + lg-side 选中项；真实区块由 ThemeHost slot 承接） -->
    <section class="card k-prev">
      <header class="k-prev-head">
        <h2>预演 · {{ adv.effectiveLayout === 'lr' ? '左右' : '单列' }}</h2>
        <small>{{ adv.state.order.filter((x) => adv.state.visible[x]).length }} 个引导器</small>
      </header>
      <ThemeHost
        :rows="{}"
        :theme="adv.effectiveTheme"
        :layout="adv.effectiveLayout"
        :loaders="adv.state.order.filter((x) => adv.state.visible[x]).map((id) => ({ id, name: loaderMeta(id).name, state: 'off' as const, entries: [] }))"
        :side="adv.state?.side"
      />
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
  /* 旧版无限宽（boot-adv-mount 全宽） */
  max-width: none;
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
