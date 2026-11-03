<script setup lang="ts">
import { computed } from "vue";
import GrubEntry from "./GrubEntry.vue";
import type { EntryData } from "./types";

/**
 * 引导条目列表（rEFInd / GRUB / 三通道共用的容器）。
 *
 * 对齐旧 `ui/grub-list-view.js` 的结构：
 *
 * ```
 * .grub-list            ← 滚动容器（sticky 钉在它顶部）
 *   .grub-entry.is-pinned   ← 选中项的置顶副本
 *   .grub-list-body
 *     .grub-entry ...       ← 正式列表（含组头 + 子项）
 * ```
 *
 * **两个历史坑，改动时务必保住**：
 *
 * 1. **sticky 必须脱离 grid 流**：grid 项的 sticky 只能在自己的格子里挪、
 *    出不了那一行，所以真正的 grid 放进 `.grub-list-body`，置顶行做
 *    `.grub-list`（block）的直接子块。
 * 2. **选中项只置顶、不从原位删除** —— 原位留着才找得到上下文，
 *    原位行照常带 `.active` 高亮。
 */

const props = withDefaults(
  defineProps<{
    entries: EntryData[];
    /** 当前选中项的 selector */
    activeSelector?: string;
    /** submenu id → 组标题 */
    groupTitles?: Record<string, string>;
    /** 是否显示组头与子项（极端格式下可能关掉） */
    showGroups?: boolean;
    /** 有动作在跑时整体禁用 */
    disabled?: boolean;
    /** 置顶行右侧小字（不给就按条目自己算） */
    pinnedTag?: string;
    /** 条目文本格式（旧版 grub.listFormat：minimal/simple/medium/detailed/raw） */
    format?: string;
  }>(),
  {
    activeSelector: "",
    groupTitles: () => ({}),
    showGroups: true,
    disabled: false,
    pinnedTag: "",
    format: "minimal",
  },
);

const emit = defineEmits<{
  (e: "select", selector: string): void;
}>();

/** 选中的条目（可以是顶层，也可以是组内子项） */
const activeEntry = computed(() =>
  props.activeSelector
    ? props.entries.find((entry) => entry.selector === props.activeSelector)
    : undefined,
);

/**
 * 正式列表要渲染的行序列。
 *
 * 顶层条目按原顺序铺开；走到第一条子项时，先把它的组头插进去，再补齐同组子项。
 */
interface Row {
  key: string;
  entry: EntryData;
  group: boolean;
  depth: number;
  childCount: number;
  groupTitle: string;
  pinned?: boolean;
  tag?: string;
}

/** 旧版 grub-list-view.js 的行文字规则：极简/简易=去前缀内核名；中等=完整内核名；复杂=原名 */
/** 旧版 grubKernelOf（grub-list-view.js:68）：优先第一个内核名，退回条目名 */
function grubKernelOf(entry: EntryData): string {
  return (entry.kernels && entry.kernels[0]) || entry.name || "";
}
/** 旧版 grubStripKernel：先去 vmlinuz- 前缀，再去 linux- 前缀 */
function grubStripKernel(name: string): string {
  return (name || "").replace(/^vmlinuz-/, "").replace(/^linux-/, "");
}
function rowLabel(entry: EntryData): string {
  const kernelName = grubKernelOf(entry);
  if (props.format === "minimal" || props.format === "simple") {
    return grubStripKernel(kernelName);
  }
  if (props.format === "medium") {
    return kernelName;
  }
  return entry.name || "";
}
const rows = computed<Row[]>(() => {
  const out: Row[] = [];
  const entries = props.entries;
  // 旧版：选中条目置顶副本（脱离组、名字带组名前缀），原位照常出行
  const activeIndex = entries.findIndex((entry) => entry.selector === props.activeSelector);
  if (activeIndex >= 0) {
    const pinned = entries[activeIndex];
    const prefix = pinned.submenu
      ? (props.groupTitles[pinned.submenu] || pinned.submenu) + " › "
      : "";
    out.push({
      key: `pinned:${pinned.selector ?? pinned.name}`,
      entry: { ...pinned, name: prefix + rowLabel(pinned) },
      group: false,
      depth: 0,
      childCount: 0,
      groupTitle: "",
      pinned: true,
    });
  }
  const childrenOf = new Map<string, EntryData[]>();
  for (const entry of entries) {
    if (entry.submenu) {
      if (!childrenOf.has(entry.submenu)) childrenOf.set(entry.submenu, []);
      childrenOf.get(entry.submenu)!.push(entry);
    }
  }
  /** 同名折叠计数（旧版 collapseScope）：标签 → 出现次数 */
  const collapseScope = (list: EntryData[]) => {
    const counts = new Map<string, number>();
    for (const entry of list) {
      const label = rowLabel(entry);
      counts.set(label, (counts.get(label) || 0) + 1);
    }
    return counts;
  };
  const emittedTop = new Set<string>();
  const emittedGroups = new Set<string>();
  const emitEntry = (entry: EntryData, depth: number, counts: Map<string, number>, emitted: Set<string>) => {
    const label = rowLabel(entry);
    if (emitted.has(label)) return;
    emitted.add(label);
    const variants = counts.get(label) || 1;
    out.push({
      key: `e:${entry.selector ?? label}:${depth}:${emitted.size}`,
      entry: { ...entry, name: label },
      group: false,
      depth,
      childCount: 0,
      groupTitle: "",
      tag: variants > 1 ? `${variants} 个变体` : "",
    });
  };
  props.entries.forEach((entry) => {
    if (!entry.submenu) {
      // 顶层：同名折叠去重
      const counts = collapseScope(entries.filter((e) => !e.submenu));
      emitEntry(entry, 0, counts, emittedTop);
      return;
    }
    if (!props.showGroups || emittedGroups.has(entry.submenu)) return;
    emittedGroups.add(entry.submenu);
    const title = props.groupTitles[entry.submenu] || entry.submenu;
    const children = childrenOf.get(entry.submenu) ?? [];
    const counts = collapseScope(children);
    // 组头（不可点）：▸ 图标 组名 + 「N 项」（N = 去重后的行数）
    const distinct = new Set(children.map(rowLabel)).size;
    out.push({
      key: `g:${entry.submenu}`,
      entry: { selector: entry.submenu, name: title },
      group: true,
      depth: -1,
      childCount: distinct,
      groupTitle: title,
    });
    const emitted = new Set<string>();
    children.forEach((child) => emitEntry(child, 1, counts, emitted));
  });
  return out;
});

function onSelect(selector: string) {
  emit("select", selector);
}
</script>

<template>
  <!--
    **不要在这里套 .grub-list** —— 旧版 DOM 里 `.grub-list` 是调用方提供的容器
    （`#grub-auto-list` / `#grub-list`），条目走 `.grub-list-body`。
    我在这里又套了一层，导致每个列表渲染出两个 .grub-list，
    多出来的那层把背景/边框/圆角重复画了一遍。
  -->
  <div class="grub-list-body">
    <!-- 置顶副本：同一条目只出现一次（正式列表里那份仍然保留，带 active 高亮） -->
    <GrubEntry
      v-if="activeEntry"
      :entry="activeEntry"
      :format="props.format"
      :pinned="true"
      :active="true"
      :disabled="disabled"
      :group-title="
        activeEntry.submenu ? groupTitles[activeEntry.submenu] || activeEntry.submenu : ''
      "
      :tag="pinnedTag"
      @select="onSelect"
    />
    <div class="grub-list-rows">
      <GrubEntry
        v-for="row in rows"
        :key="row.key"
        :entry="row.entry"
        :format="props.format"
        :group="row.group"
        :depth="row.depth"
        :child-count="row.childCount"
        :tag="row.entry.tag || ''"
        :active="!row.group && row.entry.selector === activeSelector"
        :disabled="disabled"
        :group-title="row.groupTitle"
        @select="onSelect"
      />
    </div>
  </div>
</template>
