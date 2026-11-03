<script setup lang="ts">
import { computed, defineAsyncComponent, ref, type Component, type ComponentPublicInstance } from "vue";
import type { LoaderInfo } from "./types";
import type { LayoutMode, ThemeKey } from "./registry";

/**
 * 主题宿主 + 布局分栏（对齐旧 `ui/boot-adv.js` 的 `bootThemeStage`）。
 *
 * ## 结构（逐级对齐旧版）
 * ```
 * .k-stage[.h]                     左右布局时加 .h
 *   section.k-region               左（或唯一）列
 *     .k-rhd > b(标题) + span.grow
 *     .k-rbody > .k-theme-fit > <主题组件>
 *   .k-split                       左右拖拽条（双击复位 50%）
 *   section.k-region               右列（仅 .h）
 * ```
 *
 * ## 拖拽（旧版 boot-adv.js:2702 `makeSplit`）
 * pointerdown → setPointerCapture → pointermove 改 ratio → pointerup 存盘
 * 双击复位到 0.5。
 */

const props = defineProps<{
  theme: ThemeKey | "K";
  layout?: LayoutMode;
  loaders: LoaderInfo[];
  /** 左右布局时每个引导器分到哪列（旧版 advState.side：'l'/'r'，缺省视为左列） */
  side?: Record<string, string>;
  activeId?: string;
  onSelect?: (id: string) => void;
  /** 左右分栏比例（0.15~0.85），默认 0.5 */
  ratio?: number;
}>();

const emit = defineEmits<{
  (e: "update:ratio", value: number): void;
  (e: "reorder", order: string[]): void;
}>();

const REGISTRY: Record<ThemeKey, Component> = {
  A: defineAsyncComponent(() => import("./ThemeA.vue")),
  B: defineAsyncComponent(() => import("./ThemeB.vue")),
  C: defineAsyncComponent(() => import("./ThemeC.vue")),
  G: defineAsyncComponent(() => import("./ThemeG.vue")),
  I: defineAsyncComponent(() => import("./ThemeI.vue")),
  J: defineAsyncComponent(() => import("./ThemeJ.vue")),
};

const component = computed<Component | null>(() =>
  props.theme === "K" ? null : (REGISTRY[props.theme] ?? null),
);

const themeProps = computed(() => ({
  loaders: props.loaders,
  activeId: props.activeId,
  onSelect: props.onSelect,
}));

/** 左右布局的分列 loaders（旧版：左=side!==2，右=side===2） */
function loadersFor(col: 0 | 1): LoaderInfo[] {
  if (!splitH.value) return props.loaders;
  const inRight = (id: string) => props.side?.[id] === "r";
  return props.loaders.filter((l) => (col === 1 ? inRight(l.id) : !inRight(l.id)));
}
const leftProps = computed(() => ({ ...themeProps.value, loaders: loadersFor(0) }));
const rightProps = computed(() => ({ ...themeProps.value, loaders: loadersFor(1) }));

/** 左右布局：layout === "lr" */
const splitH = computed(() => props.layout === "lr");

/** 当前比例（受控 + 本地兜底） */
const localRatio = ref(props.ratio ?? 0.5);
const currentRatio = computed(() => props.ratio ?? localRatio.value);
const firstWidth = computed(() => `${(currentRatio.value * 100).toFixed(1)}%`);

const dragging = ref(false);
let stageEl: HTMLElement | null = null;

function setStage(el: Element | ComponentPublicInstance | null) {
  stageEl = el as HTMLElement | null;
}

function startDrag(event: PointerEvent) {
  if (!splitH.value || !stageEl) return;
  event.preventDefault();
  dragging.value = true;
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}

function onDrag(event: PointerEvent) {
  if (!dragging.value || !stageEl) return;
  const rect = stageEl.getBoundingClientRect();
  const next = Math.min(0.85, Math.max(0.15, (event.clientX - rect.left) / rect.width));
  localRatio.value = next;
  emit("update:ratio", next);
}

function endDrag() {
  if (!dragging.value) return;
  dragging.value = false;
}

function resetRatio() {
  localRatio.value = 0.5;
  emit("update:ratio", 0.5);
}
</script>

<template>
  <!-- K = 不套用主题：交给原页面；非 K 由组件内 slot 承接（避免双渲染） -->
  <slot v-if="!component" />

  <div v-if="component" class="ba-theme-root">
    <div
      :ref="setStage"
      class="k-stage"
      :class="{ h: splitH }"
    >
    <!-- 左（或唯一）列 -->
    <section class="k-region" :style="splitH ? { width: firstWidth, flex: 'none' } : undefined">
      <div class="k-rhd">
        <b>{{ splitH ? "左列" : (loaders[0]?.name ?? "引导项") }}</b>
        <span class="grow" />
      </div>
      <div class="k-rbody is-theme">
        <div class="k-theme-fit">
          <component :is="component" v-bind="leftProps" @reorder="(order: string[]) => emit('reorder', order)">
            <slot />
          </component>
        </div>
      </div>
    </section>

    <!-- 左右拖拽条（旧版 boot-adv.js:2702 makeSplit） -->
    <div
      v-if="splitH"
      class="k-split"
      :class="{ 'is-drag': dragging }"
      title="拖动调整左右两列的大小（双击恢复对半分）"
      @pointerdown="startDrag"
      @pointermove="onDrag"
      @pointerup="endDrag"
      @dblclick="resetRatio"
    />

    <!-- 右列（仅左右布局） -->
    <section v-if="splitH" class="k-region" style="flex: 1">
      <div class="k-rhd">
        <b>右列</b>
        <span class="grow" />
      </div>
      <div class="k-rbody is-theme">
        <div class="k-theme-fit">
          <component :is="component" v-bind="rightProps" @reorder="(order: string[]) => emit('reorder', order)" />
        </div>
      </div>
    </section>
    </div>
  </div>
</template>
