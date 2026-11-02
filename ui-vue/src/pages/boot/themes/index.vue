<script setup lang="ts">
import { computed, defineAsyncComponent, type Component } from "vue";
import type { LoaderInfo } from "./types";
import type { LayoutMode, ThemeKey } from "./registry";

/**
 * 主题注册表 + 当前主题切换。
 *
 * 对齐旧 `ui/boot-adv.js` 的 `THEME_RENDERERS` —— 但**机制完全不同**：
 *
 * | | 旧实现 | 这里 |
 * |---|---|---|
 * | 渲染器拿到什么 | `bootBuild(l)` 返回的**真实 DOM 节点**（搬来搬去） | 纯 `LoaderInfo[]` 数据 |
 * | 切换主题 | 拆掉旧容器，把节点搬进新容器 | 换一个组件，数据不变 |
 *
 * 主题与布局档位（`THEME_LAYOUT_MODES`）：
 *   K 不套用主题（原页面，不在本注册表）
 *   A 卡片流      default / wide / lr
 *   B 左栏通道    **只有 wide**
 *   C 顶部固定    default / wide / lr
 *   G 命令面板    **只有 wide**
 *   I Bento       default / wide
 *   J 三栏        **只有 wide**
 */


const props = defineProps<{
  theme: ThemeKey | "K";
  layout?: LayoutMode;
  loaders: LoaderInfo[];
  /** 记忆里选中的通道 */
  activeId?: string;
  onSelect?: (id: string) => void;
}>();

export interface ThemeProps {
  loaders: LoaderInfo[];
  activeId?: string;
  onSelect?: (id: string) => void;
  onArrange?: () => void;
  onAdd?: () => void;
}

/** 懒加载：只有用到某个主题时才拉它的代码 */
const REGISTRY: Record<ThemeKey, Component> = {
  A: defineAsyncComponent(() => import("./ThemeA.vue")),
  B: defineAsyncComponent(() => import("./ThemeB.vue")),
  C: defineAsyncComponent(() => import("./ThemeC.vue")),
  G: defineAsyncComponent(() => import("./ThemeG.vue")),
  I: defineAsyncComponent(() => import("./ThemeI.vue")),
  J: defineAsyncComponent(() => import("./ThemeJ.vue")),
};



/** 当前主题对应的组件；K（不套用主题）返回 null，交给外层 slot */
const component = computed<Component | null>(() => {
  if (props.theme === "K") return null;
  return REGISTRY[props.theme] ?? null;
});

const themeProps = computed<ThemeProps>(() => ({
  loaders: props.loaders,
  activeId: props.activeId,
  onSelect: props.onSelect,
}));
</script>

<template>
  <!-- K = 不套用主题：交给原页面（本组件不渲染任何东西） -->
  <component :is="component" v-if="component" v-bind="themeProps" />
  <slot v-else />
</template>
