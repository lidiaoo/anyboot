<script setup lang="ts">
import { computed, ref, watch } from "vue";
import BootList from "@/components/boot/BootList.vue";
import type { LoaderInfo } from "./types";

/** 引导器设置折叠栏（旧版 .lc-settings 的展开态） */

/**
 * 主题 C：顶部固定标签（`.lc`）。
 *
 * 对齐旧 `boot-adv.js` 的 `THEME_RENDERERS.C`：
 *
 * ```
 * .lc
 *   .lc-tabs       ← 顶部通道标签（.lc-tab，当前项高亮）
 *   .lc-toolbar    ← 工具栏（密度切换等）
 *   .lc-meta       ← 副信息
 *   .lc-body       ← 滚动容器
 *     .entries       ← 条目列表
 *     .la-foot       ← 健康提示
 * ```
 */

const props = defineProps<{
  loaders: LoaderInfo[];
  activeId?: string;
  onSelect?: (id: string) => void;
  onArrange?: () => void;
}>();

const fallbackId = computed(() => props.loaders[0]?.id ?? "");
const current = ref(
  props.activeId && props.loaders.some((item) => item.id === props.activeId)
    ? props.activeId
    : fallbackId.value,
);

watch(
  () => [props.activeId, props.loaders.map((l) => l.id).join(",")] as const,
  () => {
    if (!props.loaders.some((item) => item.id === current.value)) {
      current.value = fallbackId.value;
    }
  },
);

const activeLoader = computed(
  () => props.loaders.find((item) => item.id === current.value) ?? null,
);

/** 密度：舒适 / 紧凑（写 body dataset，样式在 styles.css 里） */
const density = ref<"full" | "compact">("full");
watch(
  density,
  (value) => {
    if (value === "full") delete document.body.dataset.bootStyle;
    else document.body.dataset.bootStyle = value;
  },
  { immediate: true },
);

const healthText = computed(() => {
  const loader = activeLoader.value;
  if (!loader) return "";
  if (loader.state === "off") return "未检测到这个引导器";
  if (loader.state === "warn") return "切换这个引导器的条目需要写入 NVRAM（可能弹管理员授权）";
  return "";
});

function select(id: string) {
  current.value = id;
  props.onSelect?.(id);
}
</script>

<template>
  <div class="lc">
    <div class="lc-tabs">
      <button
        v-for="loader in loaders"
        :key="loader.id"
        type="button"
        :class="['lc-tab', { 'is-on': loader.id === current }]"
        @click="select(loader.id)"
      >
        <i :class="['dot', loader.state]" />
        <span>{{ loader.name }}</span>
        <em>{{ loader.entries.length }}</em>
      </button>
      <button
        type="button"
        class="btn icon ghost sm lc-tabadd"
        title="编排：固定 / 排序 / 溢出顺序"
        aria-label="编排"
        @click="onArrange?.()"
      >
        ⚙
      </button>
    </div>

    <!--
      引导器设置折叠栏（旧版 boot-adv.js:1361 `.lc-settings`）：
      `.acc` 按钮（gear + 说明 + 折叠箭头）+ `.panel-in` 面板。
    -->


    <div class="lc-toolbar">
      <div class="lc-meta">
        {{ activeLoader ? `${activeLoader.esp || "ESP 未知"} · ${activeLoader.entries.length} 个条目` : "" }}
      </div>
      <span class="grow" />
      <span class="mut">密度</span>
      <div class="seg">
        <button
          type="button"
          :class="{ 'is-on': density === 'full' }"
          @click="density = 'full'"
        >
          舒适
        </button>
        <button
          type="button"
          :class="{ 'is-on': density === 'compact' }"
          @click="density = 'compact'"
        >
          紧凑
        </button>
      </div>
      <button type="button" class="btn icon ghost sm" title="搜索条目">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="11" cy="11" r="7" /><path d="M21 21l-4-4" /></svg>
      </button>
      <button type="button" class="btn icon ghost sm" title="引导器设置">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" /></svg>
      </button>
    </div>

    <div class="lc-body">
      <BootList v-if="activeLoader" :loader="activeLoader.id" />
      <p v-else class="empty">没有分配到引导器</p>
      <div v-if="healthText" class="la-foot">
        <span>{{ healthText }}</span>
      </div>
    </div>
  </div>
</template>

<!-- 样式全部来自 styles/vendor/legacy.css（旧 styles.css 的 .l? 主题段），这里不写自造样式 -->
