<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import type { EntryData } from "@/components/boot/types";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import LoaderIcon from "./LoaderIcon.vue";
import LoaderSettings from "@/components/boot/LoaderSettings.vue";
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
  /** 默认选中条目（旧版 gSelMemo：lastBoot / 活动变量） */
  defaultEntrySelector?: string;
  loaders: LoaderInfo[];
  activeId?: string;
  onSelect?: (id: string) => void;
  onArrange?: () => void;
}>();

const ui = useUiStateStore();
const settings = useSettingsStore();

const query = ref("");
/** .lg-results 根（真实区块所在，旧版对真实行做 ba-g-hide 显隐） */
const resultsRoot = ref<HTMLElement | null>(null);
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
    const entries = entriesOf(loader.id);
    // 旧版 G：带 submenu 的行 = 组的子行；首个子行处合成组头
    //（▾ 名称 + N 项），子行 ↳ 缩进（样式类 .lg-sub / .lg-group 同款语义）。
    const flat: (EntryData & { _sub?: boolean; _groupHead?: boolean; _count?: number; _pinned?: boolean })[] = [];
    // 旧版：当前「记住的启动项」置顶副本（与 GRUB 区置顶行同规则），原位照常显示
    const pinnedRow = props.defaultEntrySelector
      ? entries.find((e) => e.selector === props.defaultEntrySelector)
      : undefined;
    if (pinnedRow) flat.push({ ...pinnedRow, _pinned: true });
    const emitted = new Set<string>();
    for (const e of entries) {
      if (e.submenu) {
        if (!emitted.has(e.submenu)) {
          emitted.add(e.submenu);
          const count = entries.filter((x) => x.submenu === e.submenu).length;
          flat.push({ ...e, _sub: false, _groupHead: true, _count: count, name: e.submenu, selector: e.submenu });
        }
        flat.push({ ...e, _sub: true });
      } else {
        flat.push(e);
      }
    }
    out.push({ loader, entries: flat });
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


/** 旧版：侧栏动作 = 代理真实行自己的动作（点击真实行，不另写一套） */
function proxyGo() {
  if (!selected.value) return;
  const sel = selected.value.selector;
  const row = document.querySelector('[data-row-selector="' + CSS.escape(sel) + '"]');
  if (row) (row as HTMLElement).click();
}

/** 旧版 paintResults：搜索/筛选直接对真实行与分组做显隐（不重渲条目） */
watch([query, filter, () => props.rows], async () => {
  await new Promise((r) => requestAnimationFrame(r));
  const root = resultsRoot.value;
  if (!root) return;
  const q = query.value.trim().toLowerCase();
  // 每个引导器组：组头 + 区块
  const groups = root.querySelectorAll(".lg-group[data-loader-id]");
  groups.forEach((g) => {
    const id = (g as HTMLElement).dataset.loaderId;
    const section = root.querySelector(`[data-loader-section="${id}"]`);
    const show = filter.value === "all" || filter.value === id;
    (g as HTMLElement).style.display = show ? "" : "none";
    if (section) (section as HTMLElement).style.display = show ? "" : "none";
    if (!show || !section) return;
    let hits = 0;
    section.querySelectorAll(".grub-entry").forEach((row) => {
      const ok = !q || (row.textContent || "").toLowerCase().includes(q);
      row.classList.toggle("ba-g-hide", !ok);
      if (ok) hits++;
    });
    if (q && !hits) {
      (g as HTMLElement).style.display = "none";
      (section as HTMLElement).style.display = "none";
    }
  });
});
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

      <div class="lg-results" ref="resultsRoot">
        <!-- 系统引导页：真实区块由 slot 承接（保留原有 ba-g-hide 显隐逻辑）。
             引导高级设置预演：无 slot → 用 fallback 从 props.rows 自渲染列表，
             否则预演区列表为空。 -->
        <slot>
          <template v-for="g in results" :key="g.loader.id">
            <div v-if="g.entries.length" class="lg-group">{{ g.loader.name }} · {{ g.entries.length }} 项</div>
            <button
              v-for="e in g.entries"
              :key="g.loader.id + ':' + e.selector"
              type="button"
              class="lg-item grub-entry"
              :class="[{ active: selected && selected.loader === g.loader.id && selected.selector === e.selector }]"
              @click="selected = { loader: g.loader.id, selector: e.selector || '' }"
            >
              <span class="grub-entry-name">{{ e.name }}</span>
            </button>
          </template>
        </slot>
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
        <div class="toolbar">
          <button type="button" class="btn sm primary" @click="proxyGo">下次启动</button>
          <button
            type="button"
            class="btn sm"
            disabled
            title="这一行没有「设为默认」的动作：点条目本身就是把它设为下次启动"
          >设为默认</button>
        </div>
      </template>
      <p v-else class="lg-hint">选中一条查看详情</p>
      <!-- 折叠「引导器设置」（与 A/B 一致：收起时只看到一行） -->
      <!-- 跟随「当前选中项」所属引导器；无选中则留空提示，绝不回落到 refind -->
      <LoaderSettings v-if="selected?.loader" :loader="selected.loader" />
      <div v-else class="lg-hint" style="margin-top: 4px">选中上方某条引导项后，这里显示它所属引导器的设置</div>
    </aside>

    <div class="lg-foot">
      <span><kbd>↑</kbd><kbd>↓</kbd> 移动</span>
      <span><kbd>⏎</kbd> 设为下次启动</span>
      <span><kbd>⌘K</kbd> 搜索</span>
    </div>
  </div>
</template>

<!-- 样式全部来自 styles/vendor/legacy.css（旧 styles.css 的 .l? 主题段），这里不写自造样式 -->
