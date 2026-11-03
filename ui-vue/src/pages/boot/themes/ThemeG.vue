<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import type { EntryData } from "@/components/boot/types";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import LoaderIcon from "./LoaderIcon.vue";
import type { LoaderInfo } from "./types";

/**
 * 主题 G：命令面板（`.lg`）。
 *
 * 对齐旧 `boot-adv.js` 的 `THEME_RENDERERS.G`（设计稿 viewG）：
 *
 * ```
 * .lg
 *   .lg-main
 *     .lg-top
 *       .lg-search   ← 搜索框（⌘K）
 *       .lg-filters  ← 筛选 chips（全部 N / rEFInd N / GRUB N …）
 *     .lg-results    ← 滚动容器，按引导器分组
 *       .lg-group    ← 组头（字章 + 名称 · N 项）
 *       .lg-item     ← 条目行（复用引导列表的条目样式）
 *   .lg-side       ← 右侧预览（选中项的路径 / 状态 / 动作）
 *   .lg-foot       ← 快捷键提示
 * ```
 *
 * 与旧实现的差别：旧版把真实 DOM 的条目行「压成」.lg-item；
 * 这里直接从 store 取数据渲染，样式类名保持一致。
 */

const props = defineProps<{
  /** 真实区块行（旧版 G 从真实 DOM 行派生；这里由 BootApp 传共享行） */
  rows?: Record<string, EntryData[]>;
  loaders: LoaderInfo[];
  activeId?: string;
  onSelect?: (id: string) => void;
  onArrange?: () => void;
}>();

const ui = useUiStateStore();
const settings = useSettingsStore();

const query = ref("");
/** 搜索框（⌘K / Ctrl+K 聚焦） */
const searchInputRef = ref<HTMLInputElement | null>(null);

/**
 * ⌘K / Ctrl+K 聚焦搜索框（对齐旧版 `onCmdK`，boot-adv.js:1694）。
 * 底部写着 ⌘K 就得真能用；Windows 上没有 ⌘ 所以 Ctrl+K 同效。
 */
function onCmdK(event: KeyboardEvent) {
  if (!(event.metaKey || event.ctrlKey)) return;
  if (event.key !== "k" && event.key !== "K") return;
  event.preventDefault();
  searchInputRef.value?.focus();
  searchInputRef.value?.select();
}

onMounted(() => window.addEventListener("keydown", onCmdK));
onUnmounted(() => window.removeEventListener("keydown", onCmdK));
const filter = ref("all");

/** 每个引导器的条目（按自动/手动模式取数据源） */
interface GroupedEntry {
  loader: LoaderInfo;
  entries: EntryData[];
}

const groups = computed<GroupedEntry[]>(() => {
  const out: GroupedEntry[] = [];
  for (const loader of props.loaders) {
    out.push({ loader, entries: entriesOf(loader.id) });
  }
  return out;
});

/** 与 BootList 同口径的数据源选择 */
function entriesOf(id: string): EntryData[] {
  if (props.rows && props.rows[id]) return props.rows[id];
  if (id === "refind") {
    if (settings.refindMode === "manual") {
      return (ui.status?.variables ?? [])
        .filter((v) => v.exists)
        .map((v) => ({ selector: v.source, name: v.displayName, class: v.system }));
    }
    return (ui.refindSim?.entries ?? [])
      .filter((e) => !e.hidden)
      .map((e) => ({ selector: e.title.trim(), name: e.title.trim(), class: e.kind }));
  }
  if (id === "grub") {
    const source = settings.grubMode === "manual" ? ui.grub : (ui.linuxGrub ?? ui.grub);
    return (source?.entries ?? []).map((e) => ({
      selector: e.selector,
      name: e.name,
      class: e.class,
      kernels: e.kernels,
      variants: e.variants,
      chainloader: e.chainloader,
      submenu: e.submenu,
    }));
  }
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

/** 筛选 chips */
const filters = computed(() => {
  const total = groups.value.reduce((sum, g) => sum + g.entries.length, 0);
  return [
    { id: "all", label: `全部 ${total}` },
    ...groups.value.map((g) => ({ id: g.loader.id, label: `${g.loader.name} ${g.entries.length}` })),
  ];
});

/** 过滤后的结果 */
const results = computed(() => {
  const keyword = query.value.trim().toLowerCase();
  return groups.value
    .filter((g) => filter.value === "all" || g.loader.id === filter.value)
    .map((g) => ({
      ...g,
      entries: keyword
        ? g.entries.filter((e) => e.name.toLowerCase().includes(keyword))
        : g.entries,
    }))
    .filter((g) => g.entries.length > 0);
});

/** 当前高亮的条目（右侧预览用） */
const selected = ref<{ loader: string; selector: string } | null>(null);
// 旧版语义：默认 selected=0，右侧永远有一份预览，不是空窗
watch(results, (groups) => {
  if (!selected.value && groups.length && groups[0].entries.length) {
    const first = groups[0].entries.find((e) => e.selector); 
    if (first?.selector) selected.value = { loader: groups[0].loader.id, selector: first.selector };
  }
});

const selectedEntry = computed(() => {
  if (!selected.value) return null;
  const group = results.value.find((g) => g.loader.id === selected.value?.loader);
  return group?.entries.find((e) => e.selector === selected.value?.selector) ?? null;
});

watch(
  () => results.value.map((g) => `${g.loader.id}:${g.entries.length}`).join(","),
  () => {
    // 结果集变了，原来的选中项可能已经不在了
    if (selected.value && !selectedEntry.value) selected.value = null;
  },
);

function pick(loaderId: string, selector: string) {
  selected.value = { loader: loaderId, selector };
}

function commit() {
  if (!selected.value) return;
  props.onSelect?.(selected.value.selector);
}

const selectedLoaderState = computed(
  () => props.loaders.find((l) => l.id === selected.value?.loader)?.state ?? "ok",
);
</script>

<template>
  <div class="lg">
    <div class="lg-main">
      <div class="lg-top">
        <div class="lg-search">
          <span aria-hidden="true">🔍</span>
          <input ref="searchInputRef" v-model="query" placeholder="搜索系统、引导器或动作…" />
          <span class="kbd">⌘K</span>
        </div>
        <div class="lg-filters">
          <button
            v-for="f in filters"
            :key="f.id"
            type="button"
            :class="['lg-fchip', { 'is-on': filter === f.id }]"
            @click="filter = f.id"
          >
            {{ f.label }}
          </button>
        </div>
      </div>

      <div class="lg-results">
        <template v-for="group in results" :key="group.loader.id">
          <div class="lg-group">
            <LoaderIcon :id="group.loader.id" size="sm" />
            <b>{{ group.loader.name }}</b>
            <span>· {{ group.entries.length }} 项</span>
          </div>
          <button
            v-for="entry in group.entries"
            :key="`${group.loader.id}:${entry.selector}`"
            type="button"
            :class="[
              'lg-item',
              {
                'is-active':
                  selected?.loader === group.loader.id &&
                  selected?.selector === entry.selector,
              },
            ]"
            @click="pick(group.loader.id, entry.selector ?? '')"
            @dblclick="commit"
          >
            <span class="t">
              <b>{{ entry.name }}</b>
              <small v-if="entry.kernels?.[0]">{{ entry.kernels[0] }}</small>
            </span>
          </button>
        </template>
        <p v-if="!results.length" class="lg-hint">暂无条目 · 展开下面的「引导器设置」读取</p>
        <!-- 旧版：真实区块搬进命令面板列表（ba-theme-sec/ba-split/ba-g-card） -->
        <slot />
      </div>
    </div>

    <!--
      右侧「选中项」面板（旧版 boot-adv.js:1533 起）：
        h4「选中项」+ .lg-sel > .lg-sel-head(.row-ico + b) + .lg-sel-body > .lg-fact × N
    -->
    <aside class="lg-side">
      <template v-if="selectedEntry">
        <h4>选中项</h4>
        <div class="lg-sel">
          <div class="lg-sel-head">
            <LoaderIcon :id="selected?.loader ?? ''" size="sm" />
            <b>{{ selectedEntry.name }}</b>
          </div>
          <div class="lg-sel-body">
            <div class="lg-fact">
              <b>引导器</b>
              <span>{{ props.loaders.find((l) => l.id === selected?.loader)?.name ?? "—" }}</span>
            </div>
            <div class="lg-fact">
              <b>路径</b>
              <span class="mono">{{ selectedEntry.selector || "—" }}</span>
            </div>
            <div class="lg-fact">
              <b>状态</b>
              <span>{{ selectedLoaderState === "ok" ? "可用" : "需提权" }}</span>
            </div>
            <div v-if="selectedEntry.kernels?.length" class="lg-fact">
              <b>内核</b>
              <span class="mono">{{ selectedEntry.kernels[0] }}</span>
            </div>
          </div>
        </div>
        <button type="button" class="btn sm" @click="commit">设为下次启动</button>
      </template>
      <p v-else class="lg-hint">选中一条查看详情</p>
    </aside>

    <div class="lg-foot">
      <span><kbd>↑</kbd><kbd>↓</kbd> 移动</span>
      <span><kbd>⏎</kbd> 设为下次启动</span>
      <span><kbd>⌘K</kbd> 搜索</span>
    </div>
  </div>
</template>

<!-- 样式全部来自 styles/vendor/legacy.css（旧 styles.css 的 .l? 主题段），这里不写自造样式 -->
