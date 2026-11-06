<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { useBootAdvStore, type LoaderId } from "@/stores/bootAdv";
import { loaderMeta } from "@/pages/boot/themes/loaders";
import { simRows, refindVariableRows, grubRows } from "@/pages/boot/bootRows";
import type { EntryData } from "@/components/boot/types";
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
const settings = useSettingsStore();
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

/** 引导器默认项（旧版 def = l.nextBoot || entries.find(def)） */
function loaderDefault(id: string): string {
  if (id === "refind") {
    const hit = (ui.status?.variables ?? []).find((v) => v.active);
    const lb = (ui.refindSim?.entries ?? []).find((e) => e.lastBoot);
    return (lb?.title || hit?.displayName || "未知").toString();
  }
  if (id === "grub") {
    const g = settings.grubMode === "manual" ? ui.grub : ui.linuxGrub;
    return (g?.nextEntry || g?.savedEntry || "未知").toString();
  }
  const ch = (ui.channels ?? []).find((c) => c.id === id);
  // 旧版 channelEntries：currentDefault 可能是 id 或标题，命中 channel.entries 取标题
  const cur = String(ch?.currentDefault || "");
  const list = ((ch?.entries ?? []) as { id?: string; title?: string }[]);
  const hit = cur
    ? list.find((e) => cur === e.id || cur === e.title || cur.includes(e.title || "\u0000"))
    : undefined;
  return (hit?.title || cur || (ch?.detected ? "未选择" : "未知")).toString();
}
/** 引导器图标里的两个字母（旧版 ldr 图标文字） */
function loaderMono(id: string): string {
  return loaderMeta(id).mono;
}
/** 引导器权限类型（file/nvram）——旧版免提权/需提权判定 */
function loaderPerm(id: string): string {
  return loaderMeta(id).perm;
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
/** 引导器 ESP 副标题（与 BootApp/旧版适配器一致：按模式零回落） */
function loaderEsp(id: string): string {
  if (id === "refind") return settings.refindMode === "manual" ? "vars/PreviousBoot 快照" : "ESP · refind.conf";
  if (id === "grub") {
    if (settings.grubMode === "manual") return ui.grub?.cfgPath || "grub.cfg";
    const t = (ui.linuxGrub as unknown as { target?: { cfgRel?: string } } | null)?.target;
    return t?.cfgRel || "";
  }
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



/** 预演面板用的真实引导器（ESP/权限/默认项），供各布局预览卡片显示设置 */
const previewLoaders = computed(() =>
  adv.state.order
    .filter((id) => adv.state.visible[id])
    .map((id) => {
      const perm = loaderMeta(id).perm;
      const defTitle = loaderDefault(id);
      const count = (previewRows.value[id] ?? []).length;
      const entries: { t?: string; def?: boolean }[] = [];
      if (defTitle && defTitle !== "未知") entries.push({ t: defTitle, def: true });
      while (entries.length < count) entries.push({ t: "", def: false });
      return {
        id,
        name: loaderMeta(id).name,
        state: (perm === "nvram" ? "warn" : "ok") as "ok" | "warn" | "off",
        entries,
        esp: loaderEsp(id),
        perm,
        detected: true,
        nextBoot: entries.find((e) => e.def) ?? null,
      };
    }),
);
/** 预演面板用的真实行（与系统引导页同一份行源） */
const previewRows = computed<Record<string, EntryData[]>>(() => {
  const out: Record<string, EntryData[]> = {};
  for (const id of adv.state.order.filter((x) => adv.state.visible[x])) {
    if (id === "refind") {
      out[id] = settings.refindMode === "manual"
        ? refindVariableRows(ui.status?.variables)
        : simRows(ui.refindSim);
    } else if (id === "grub") {
      out[id] = grubRows({
        grubMode: settings.grubMode,
        linuxGrubEntries: (ui.linuxGrub?.entries ?? []) as never,
        grubEntries: (ui.grub?.entries ?? []) as never,
        variables: ui.status?.variables,
      });
    } else {
      out[id] = ((ui.channels.find((c) => c.id === id)?.entries ?? []) as never[]).map((e: never) => {
        const row = e as { title?: string };
        return { selector: row.title ?? "", name: row.title ?? "" } as EntryData;
      });
    }
  }
  return out;
});

/** 旧版 applyVwClasses：按容器宽度挂 .vw-900/.vw-760/.vw-520 断点类（屏幕适配） */
const rootEl = ref<HTMLElement | null>(null);
const kPrevEl = ref<HTMLElement | null>(null);
let vwRo: ResizeObserver | null = null;
function applyVwClasses(el: HTMLElement, width: number): void {
  el.classList.toggle("vw-g", width >= 900);
  el.classList.toggle("vw-900", width <= 900);
  el.classList.toggle("vw-760", width <= 760);
  el.classList.toggle("vw-520", width <= 520);
}
onMounted(() => {
  const el = rootEl.value;
  if (!el || typeof ResizeObserver === "undefined") return;
  vwRo = new ResizeObserver((entries) => {
    for (const en of entries) {
      applyVwClasses(el, en.contentRect.width);
      // 预演卡片按自身宽度挂断点类（宽窗口里预演区仍可能很窄 → 需要移动端堆叠）
      if (kPrevEl.value) applyVwClasses(kPrevEl.value, kPrevEl.value.getBoundingClientRect().width);
    }
  });
  vwRo.observe(el);
  applyVwClasses(el, el.getBoundingClientRect().width);
  if (kPrevEl.value) {
    vwRo.observe(kPrevEl.value);
    applyVwClasses(kPrevEl.value, kPrevEl.value.getBoundingClientRect().width);
  }
});
onUnmounted(() => { vwRo?.disconnect(); vwRo = null; });
</script>

<template>
  <div class="settings-page" ref="rootEl">
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
        <!-- row5：排序条（设置动作收在设置区，预演区保持纯观看） -->
        <div class="k-row">
          <span class="k-sortwrap" style="display: contents">
            <span class="k-sortbar">
              <span class="k-sort-lbl">上下排序</span>
              <span v-for="(id, i) in adv.state.order" :key="id" class="k-sortchip">
                <span class="k-sort-nm">{{ loaderMeta(id).name }}</span>
                <button type="button" class="k-zop" title="上移一位" :disabled="i === 0" @click="moveLoader(id, -1)">↑</button>
                <button type="button" class="k-zop" title="下移一位" :disabled="i === adv.state.order.length - 1" @click="moveLoader(id, 1)">↓</button>
              </span>
            </span>
          </span>
          <span class="grow" />
          <span class="hint">预演与「系统引导」页同一份真实数据（只读示意）· 界面效果以「系统引导」页实际为准</span>
        </div>
        <!-- 引导器设置（k-setlist 折叠条） -->
        <button
          type="button"
          class="k-setbar"
          :class="{ 'is-on': setListOpen }"
          title="展开/收起 引导器设置（ESP 路径 / 权限 / 默认项 / 脚本 / 免密模式）"
          @click="setListOpen = !setListOpen"
        >
          <span class="gear" aria-hidden="true">⚙</span>
          <span class="k-setbar-t">引导器设置</span>
          <span class="hint">ESP 路径 · 权限 · 默认项 · 脚本 · 免密模式</span>
          <span class="grow" />
          <span class="chevron car" :class="{ open: setListOpen }">⌄</span>
        </button>
        <div class="k-setlist" :class="{ 'is-on': setListOpen }" :hidden="!setListOpen">
          <template v-if="adv.state.order.some((x) => adv.state.visible[x])">
            <div v-for="id in adv.state.order.filter((x) => adv.state.visible[x])" :key="id" class="k-set-row">
              <span class="ldr" aria-hidden="true">{{ loaderMono(id) }}</span>
              <span class="k-set-nm">{{ loaderMeta(id).name }}</span>
              <span class="chip">ESP · {{ loaderEsp(id) || '未知' }}</span>
              <span v-if="loaderPerm(id) === 'nvram'" class="chip elev">需提权</span>
              <span v-else class="chip ok">免提权</span>
              <span class="chip">默认项 · {{ loaderDefault(id) }}</span>
              <span class="chip">脚本 · {{ id === 'grub' ? '09_anyboot' : '不适用' }}</span>
              <span class="chip">免密 · {{ id === 'opencore' ? '未开启' : '不适用' }}</span>
              <span class="grow" />
              <button type="button" class="btn xs ghost" :title="'打开 ' + loaderMeta(id).name + ' 详细设置'">⚙</button>
            </div>
          </template>
          <div v-else class="k-set-row">没有启用的引导器 —— 点上方标签开启后再设置</div>
        </div>
      </div>
    </div>
    <!-- 预演面板（旧版：bootThemeStage → .k-stage + lg-side 选中项；真实区块由 ThemeHost slot 承接） -->
    <section class="card k-prev" ref="kPrevEl">
      <header class="k-prev-head">
        <h2>预演 · {{ adv.effectiveLayout === 'lr' ? '左右' : '单列' }}</h2>
        <small>{{ adv.state.order.filter((x) => adv.state.visible[x]).length }} 个引导器</small>
      </header>
      <!-- K 不套用主题：旧版 k-zone-cards + kLoaderCard 自绘卡片 -->
      <div v-if="adv.effectiveTheme === 'K'" class="k-zone-cards">
        <div v-for="loader in previewLoaders" :key="loader.id" class="k-card">
          <div class="k-card-head">
            <span class="ldr" aria-hidden="true">{{ loaderMono(loader.id) }}</span>
            <span class="nm">{{ loader.name }}</span>
            <span class="grow" />
            <span class="hint">共 {{ (previewRows[loader.id] ?? []).length }} 条</span>
          </div>
          <div class="k-entries">
            <div
              v-for="(row, i) in previewRows[loader.id] ?? []"
              :key="i"
              class="grub-entry"
              :class="{ 'is-pinned': i === 0 }"
            >
              <span class="grub-entry-name">{{ row.name }}</span>
            </div>
            <div v-if="!(previewRows[loader.id] ?? []).length" class="hint">没有条目</div>
          </div>
        </div>
      </div>
      <ThemeHost
        v-else
        :rows="previewRows"
        :theme="adv.effectiveTheme"
        :layout="adv.effectiveLayout"
        :loaders="previewLoaders"
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
