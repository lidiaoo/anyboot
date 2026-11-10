<script setup lang="ts">
import { computed, ref, watch } from "vue";
import BootList from "@/components/boot/BootList.vue";
import LoaderSettings from "@/components/boot/LoaderSettings.vue";
import LoaderIcon from "./LoaderIcon.vue";
import StateChip from "./StateChip.vue";
import type { LoaderInfo } from "./types";

/**
 * 主题 B：左栏通道 + 右栏详情（`.lb`）。
 *
 * 对齐旧 `boot-adv.js` 的 `THEME_RENDERERS.B`：
 *
 * ```
 * .lb
 *   aside.lb-nav              ← 左栏
 *     .lb-nav-head              「引导器 N」+ 添加按钮
 *     .lb-list                  通道列表（.is-on = 当前）
 *   (右栏)
 *     h2  ← 图标 + 名称 + 状态胶囊
 *     p   ← ESP · N 个条目
 *     .lb-scroll   ← 滚动容器
 *       .lb-block（启动项 / 引导器设置各一块）
 * ```
 *
 * 注意：B 只有 `wide` 档位（`THEME_LAYOUT_MODES.B = ["wide"]`）——
 * 左栏 + 右栏在这个宽度以下只剩一条缝。
 */

const props = defineProps<{
  loaders: LoaderInfo[];
  /** 记忆里选中的通道（可能不在本列，需兜底到第一个） */
  activeId?: string;
  onSelect?: (id: string) => void;
  onAdd?: () => void;
  /** 「编排与排序」：打开引导高级设置 */
  onArrange?: () => void;
}>();

/**
 * 左右布局下每个渲染器只拿到**自己那一列**的引导器：
 * 记忆里的 active 可能不在这一列（themeSel 是全局的），直接用会取到 undefined
 * → 读 .entries 当场抛错，整页布置中断（「左右布局点了没变化」的一个真凶）。
 */
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

const metaText = computed(() => {
  const loader = activeLoader.value;
  if (!loader) return "";
  return `${loader.esp || "ESP 未知"} · ${loader.entries.length} 个条目`;
});


function select(id: string) {
  current.value = id;
  props.onSelect?.(id);
}
</script>

<template>
  <div class="lb">
    <aside class="lb-nav">
      <div class="lb-nav-head">
        <span class="grow">引导器 {{ loaders.length }}</span>
        <button
          type="button"
          class="btn icon ghost sm"
          title="添加 / 重新识别通道"
          aria-label="添加"
          @click="onAdd?.()"
        >
          +
        </button>
      </div>
      <div class="lb-list">
        <!-- 旧版 boot-adv.js:1251：.lb-item > .lb-grip + 图标 + .nm(b+small) + dot -->
        <button
          v-for="loader in loaders"
          :key="loader.id"
          type="button"
          class="lb-item"
          :class="{ 'is-on': loader.id === current }"
          @click="select(loader.id)"
        >
          <svg
            class="i sm lb-grip"
            viewBox="0 0 16 16"
            fill="currentColor"
            aria-hidden="true"
          >
            <circle cx="6" cy="4" r="1.1" /><circle cx="10" cy="4" r="1.1" />
            <circle cx="6" cy="8" r="1.1" /><circle cx="10" cy="8" r="1.1" />
            <circle cx="6" cy="12" r="1.1" /><circle cx="10" cy="12" r="1.1" />
          </svg>
          <LoaderIcon :id="loader.id" size="sm" />
          <span class="nm">
            <b>{{ loader.name }}</b>
            <small>{{ loader.entries.length }} 个条目</small>
          </span>
          <i :class="['dot', loader.state]" />
        </button>
      </div>
      <!-- 旧版 boot-adv.js:1266：.lb-nav-foot > 编排与排序 / 新增 -->
      <div class="lb-nav-foot">
        <button type="button" class="btn sm ghost" @click="onArrange?.()">编排与排序</button>
        <button type="button" class="btn sm ghost" title="添加 / 重新识别通道" @click="onAdd?.()">＋</button>
      </div>
    </aside>

    <div class="lb-main">
      <template v-if="activeLoader">
        <!-- 旧版 boot-adv.js:1272：.lb-main-head > .tt(mainName + mainMeta) + 下次启动 + more -->
        <div class="lb-main-head">
          <div class="tt">
            <h2>
              <LoaderIcon :id="activeLoader.id" size="md" />
              {{ activeLoader.name }}
              <StateChip :state="activeLoader.state" />
            </h2>
            <p>{{ metaText }}</p>
          </div>
        </div>

        <div class="lb-scroll">
          <div class="lb-block">
            <div class="lb-block-body">
              <BootList :loader="activeLoader.id" />
              <LoaderSettings :loader="activeLoader.id" />
            </div>
          </div>

        </div>
      </template>
      <p v-else class="empty">这一列没有分配到引导器</p>
    </div>
  </div>
</template>

<!-- 样式全部来自 styles/vendor/legacy.css（旧 styles.css 的 .l? 主题段），这里不写自造样式 -->
