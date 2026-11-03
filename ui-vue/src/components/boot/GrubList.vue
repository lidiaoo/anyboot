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

/** 按 submenu 分好组的子项 */
const childrenOf = computed(() => {
  const map = new Map<string, EntryData[]>();
  for (const entry of props.entries) {
    if (!entry.submenu) continue;
    const list = map.get(entry.submenu);
    if (list) list.push(entry);
    else map.set(entry.submenu, [entry]);
  }
  return map;
});

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
}

const rows = computed<Row[]>(() => {
  const out: Row[] = [];
  const emittedGroups = new Set<string>();
  props.entries.forEach((entry, index) => {
    if (!entry.submenu) {
      out.push({
        key: `e${index}:${entry.selector ?? entry.name}`,
        entry,
        group: false,
        depth: 0,
        childCount: 0,
        groupTitle: "",
      });
      return;
    }
    if (!props.showGroups || emittedGroups.has(entry.submenu)) return;
    emittedGroups.add(entry.submenu);
    const title = props.groupTitles[entry.submenu] || entry.submenu;
    const children = childrenOf.value.get(entry.submenu) ?? [];
    out.push({
      key: `g:${entry.submenu}`,
      entry: { selector: entry.submenu, name: title },
      group: true,
      depth: 1,
      childCount: children.length,
      groupTitle: title,
    });
    children.forEach((child, childIndex) => {
      out.push({
        key: `c${index}-${childIndex}:${child.selector ?? child.name}`,
        entry: child,
        group: false,
        depth: 1,
        childCount: 0,
        groupTitle: title,
      });
    });
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
