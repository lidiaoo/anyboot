<script setup lang="ts">
/** 卡片底部的健康状态文案（旧版 `l.healthText`） */
function footText(loader: { id: string; state: string; entries: unknown[] }): string {
  if (loader.state === "warn") return "需要提权才能修改（root 助手未安装）";
  if (loader.state === "off") return "未检测到该引导器";
  return `${loader.entries.length} 个可启动项 · 读取正常`;
}

import { computed, nextTick, ref } from "vue";
import BootList from "@/components/boot/BootList.vue";
import LoaderSettings from "@/components/boot/LoaderSettings.vue";
import type { LoaderInfo } from "./types";

/**
 * 主题 A：卡片流（`.la`）。
 *
 * 对齐旧 `boot-adv.js` 的 `THEME_RENDERERS.A`：
 *
 * ```
 * .la
 *   .la-index              ← 顶部通道快捷条（点一下滚到对应卡片）
 *     .la-chip.is-on       ← 当前高亮
 *     span(flex:1)
 *     button.btn.icon.ghost.sm  ← 编排按钮
 *   .la-list               ← 竖向卡片列表（滚动容器）
 * ```
 *
 * **与原实现的唯一区别**：条目不再由 `bootBuild(l)` 搬运真实 DOM 节点，
 * 而是由 `BootList` 组件按 loader id 自己从 store 取数据渲染。
 */

const props = defineProps<{
  loaders: LoaderInfo[];
  /** 打开「设置与编排」抽屉 */
  onArrange?: () => void;
}>();

const activeId = ref(props.loaders[0]?.id ?? "");
const cardRefs = ref(new Map<string, HTMLElement>());

function setCardRef(id: string, el: unknown) {
  if (el instanceof HTMLElement) cardRefs.value.set(id, el);
  else cardRefs.value.delete(id);
}

async function focusLoader(id: string) {
  activeId.value = id;
  await nextTick();
  const target = cardRefs.value.get(id);
  if (!target) return;
  target.scrollIntoView({ behavior: "smooth", block: "start" });
  // 闪一下，让用户看清滚到了哪张卡
  target.classList.remove("ba-flash");
  void target.offsetWidth;
  target.classList.add("ba-flash");
}

const hasLoaders = computed(() => props.loaders.length > 0);
</script>

<template>
  <div class="la">
    <div class="la-index">
      <button
        v-for="loader in loaders"
        :key="loader.id"
        type="button"
        :class="['la-chip', { 'is-on': loader.id === activeId }]"
        @click="focusLoader(loader.id)"
      >
        <i :class="['dot', loader.state]" />
        <span class="nm">{{ loader.name }}</span>
        <em>{{ loader.entries.length }}</em>
      </button>
      <span style="flex: 1" />
      <button
        type="button"
        class="btn icon ghost sm"
        title="拖拽排序 / 编排显示"
        aria-label="编排"
        @click="onArrange?.()"
      >
        ⇅
      </button>
    </div>

    <div class="la-list">
      <div
        v-for="loader in loaders"
        :key="loader.id"
        :ref="(el) => setCardRef(loader.id, el)"
        class="la-card"
      >
        <BootList :loader="loader.id" />
        <LoaderSettings :loader="loader.id" />
        <!-- 底部健康状态条（旧版 .la-foot：icon(info) + 状态文案） -->
        <div class="la-foot">
          <svg
            class="i sm"
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
            aria-hidden="true"
          >
            <circle cx="8" cy="8" r="6.2" />
            <path d="M8 5v3.4M8 11h.01" />
          </svg>
          <span>{{ footText(loader) }}</span>
        </div>
      </div>
      <p v-if="!hasLoaders" class="empty">
        <b>没有启用的引导器</b>
        <span>去「引导高级设置」勾选要显示的引导器</span>
      </p>
    </div>
  </div>
</template>

<!-- 样式全部来自 styles/vendor/legacy.css（旧 styles.css 的 .l? 主题段），这里不写自造样式 -->
