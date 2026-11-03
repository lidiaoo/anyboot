<script setup lang="ts">
import { computed } from "vue";
import type { EntryData } from "./types";

/**
 * 引导条目行（**所有引导列表共用的唯一实现**）。
 *
 * 对齐旧前端 `ui/grub-list-view.js` 的 `makeRow()` —— 类名逐字保留，
 * 因为 `ui/styles.css` 里 5000+ 行样式都挂在这些类名上，改名等于全部重写。
 *
 * 视觉要点（历史定案，勿改）：
 *   · `.grub-entry` 是行盒子；`.is-group` = 组头 / `.is-child` = 组里的一项
 *   · `.active` = 当前选中（用 GRUB 原生那套，不自造）
 *   · `.is-pinned` = 置顶副本（`position: sticky; top: 0`）
 *   · `.depth-N` = 二级子菜单缩进
 *   · 名字 `.grub-entry-name` **必须完整多行显示，绝不省略号**
 */

const props = withDefaults(
  defineProps<{
    entry: EntryData;
    /** 是不是组头（组头不可点，右侧显示「N 项」） */
    group?: boolean;
    /** 嵌套深度（≥2 时加 `.depth-N` 缩进） */
    depth?: number;
    /** 是不是置顶副本 */
    pinned?: boolean;
    /** 是不是当前选中 */
    active?: boolean;
    /** 组内子项数量（组头右侧「N 项」） */
    childCount?: number;
    /** 置顶行要带上它原本所属的组名 */
    groupTitle?: string;
    /** 整体禁用（有别的动作在跑） */
    disabled?: boolean;
    /** 自定义右侧小字（不给就按 variants / kernel / class 自动拼） */
    tag?: string;
    /** 条目文本格式（旧版 grub.listFormat） */
    format?: string;
  }>(),
  {
    group: false,
    depth: 0,
    pinned: false,
    active: false,
    childCount: 0,
    groupTitle: "",
    disabled: false,
    tag: "",
    format: "minimal",
  },
);

const emit = defineEmits<{
  (e: "select", selector: string): void;
}>();

/** 置顶行脱离了原来的组，名字里把组名带上，不然看不出它属于哪个 submenu */
/** 内核名（旧版 grubKernelOf） */
function kernelOf(entry: { kernels?: string[]; name?: string }): string {
  return (entry.kernels && entry.kernels[0]) || entry.name || "";
}
/** 去内核前缀（旧版 grubStripKernel） */
function stripKernel(name: string): string {
  return (name || "").replace(/^vmlinuz-/, "").replace(/^linux-/, "");
}
/** 按 format 计算条目基础文本（旧版 renderFormatEntries 的 text 逻辑） */
function formatText(entry: { kernels?: string[]; name?: string }, format: string): string {
  return entry.kernels ? (
    format === "minimal"
      ? stripKernel(kernelOf(entry))
      : format === "simple"
        ? stripKernel(kernelOf(entry))
        : format === "medium"
          ? kernelOf(entry)
          : format === "detailed"
            ? (entry.name ?? "")
            : entry.name ?? ""
  ) : (entry.name ?? "");
}

const displayName = computed(() => {
  const prefix =
    props.pinned && props.entry.submenu
      ? `${props.groupTitle || props.entry.submenu} › `
      : "";
  // 缩进靠 CSS（.is-child / .depth-N），这里只用箭头补强「这是组里的项」
  const baseText = props.group
    ? props.entry.name
    : formatText(props.entry, props.format);
  // 组行箭头（旧版：默认展开 ▾ / 收起 ▸；本列表恒展开 → ▾）
  const arrow = props.group ? '▾ ' : '';
  const body =
    props.group || props.depth === 0 ? arrow + baseText : `↳ ${baseText}`;
  return prefix + body;
});

/** 右侧小字：组头显示「N 项」；普通项显示变体数 / 内核 / class */
const tagText = computed(() => {
  if (props.tag) return props.tag;
  if (props.group) return `${props.childCount} 项`;
  const variants = Number(props.entry.variants) || 1;
  const parts: string[] = [];
  if (variants > 1) parts.push(`${variants} 个变体`);
  // 旧版：detailed 格式才有内核角标；其余格式无角标（除非有变体数）
  if (props.format === "detailed") {
    parts.push(
      props.entry.chainloader
        ? "转交引导"
        : props.entry.kernels && props.entry.kernels[0]
          ? "内核 " + props.entry.kernels[0]
          : props.entry.class || "",
    );
  }
  return parts.filter(Boolean).join(" · ");
});

/** 组头不是 menuentry，写进 grubenv 也切不过去 —— 只作标题，不可点 */
const clickable = computed(() => !props.group && !props.disabled);

const classes = computed(() => [
  "grub-entry",
  props.group ? "is-group" : "is-child",
  props.depth >= 2 ? `depth-${props.depth}` : "",
  props.active ? "active" : "",
  props.pinned ? "is-pinned" : "",
]);

function onClick() {
  if (!clickable.value) return;
  const selector = props.entry.selector ?? "";
  if (selector) emit("select", selector);
}
</script>

<template>
  <button
    type="button"
    :class="classes"
    :disabled="group || disabled"
    :title="entry.selector || ''"
    @click="onClick"
  >
    <span class="grub-entry-name">{{ displayName }}</span>
    <small v-if="tagText" :class="group ? 'grub-entry-count' : 'grub-entry-tag'">
      {{ tagText }}
    </small>
  </button>
</template>

<!--
  样式**不写在这里**：`.grub-entry` 系列规则全在 `ui/styles.css` 里（5000+ 行），
  迁移期先让新旧共用同一份 CSS（`src/styles/base.css` 会 @import 旧表），
  等阶段 5 再决定是否拆进组件 scoped。
-->
