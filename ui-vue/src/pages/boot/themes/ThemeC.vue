<script setup lang="ts">
import { computed, ref, watch } from "vue";
import BootList from "@/components/boot/BootList.vue";
import LoaderSettings from "@/components/boot/LoaderSettings.vue";
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
        <span>{{ loader.name }}</span>
        <em>{{ loader.entries.length }}</em>
      </button>
    </div>

    <!--
      引导器设置折叠栏（旧版 boot-adv.js:1361 `.lc-settings`）：
      `.acc` 按钮（gear + 说明 + 折叠箭头）+ `.panel-in` 面板。
    -->


    <div class="lc-toolbar">
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
    </div>

    <div class="lc-body">
      <BootList v-if="activeLoader" :loader="activeLoader.id" />
      <LoaderSettings v-if="activeLoader" :loader="activeLoader.id" />
      <p v-else class="empty">没有分配到引导器</p>
      <div v-if="healthText" class="la-foot">
        <span>{{ healthText }}</span>
      </div>
    </div>
  </div>
</template>

<!-- 样式全部来自 styles/vendor/legacy.css（旧 styles.css 的 .l? 主题段），这里不写自造样式 -->
