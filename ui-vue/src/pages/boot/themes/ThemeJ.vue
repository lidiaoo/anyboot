<script setup lang="ts">
import { computed, ref, useSlots, watch } from "vue";
import BootList from "@/components/boot/BootList.vue";
import LoaderSettings from "@/components/boot/LoaderSettings.vue";
import LoaderIcon from "./LoaderIcon.vue";
import StateChip from "./StateChip.vue";
import type { LoaderInfo } from "./types";
const collapsed = ref(false);
/** 是否有设置正文（旧版靠 .ba-set-body 是否停靠来判断） */
const slots = useSlots();
const settingsSlot = computed(() => Boolean(slots.settings));

/**
 * 主题 J：三栏（`.lj`）—— 通道栏 / 详情 / 设置栏。
 *
 * 对齐旧 `boot-adv.js` 的 `THEME_RENDERERS.J`：
 *
 * ```
 * .lj
 *   aside.lj-c1        ← 通道栏（「通道」+ 列表，⚠ 移动端要变成顶部横滑）
 *     .lj-nav
 *   section.lj-c2      ← 详情
 *     .hd  （图标 + 名称 + 状态胶囊）
 *     .bd  （滚动容器）
 *   aside.lj-c3        ← 引导器设置（可折叠成图标条）
 *     .hd
 *     .bd
 * ```
 *
 * 移动端（≤760）的既有定案：**通道栏到顶部横滑、列表占满到底部**，
 * 样式在 `ui/styles.css` 的 `.vw-760 .lj-c1` 系列里，这里只保证 DOM 结构对。
 */

const props = defineProps<{
  loaders: LoaderInfo[];
  activeId?: string;
  onSelect?: (id: string) => void;
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

/** 设置栏折叠（只留图标条） */
const settingsCollapsed = ref(false);

function select(id: string) {
  current.value = id;
  props.onSelect?.(id);
}

const healthText = computed(() => {
  const loader = activeLoader.value;
  if (!loader) return "";
  if (loader.state === "off") return "未检测到这个引导器";
  if (loader.state === "warn") return "切换条目需写入 NVRAM（可能弹管理员授权）";
  return "";
});
</script>

<template>
  <div class="lj">
    <aside class="lj-c1">
      <div class="hd">通道</div>
      <nav class="lj-nav">
        <button
          v-for="loader in loaders"
          :key="loader.id"
          type="button"
          class="lj-navi"
          :class="{ 'is-on': loader.id === current }"
          @click="select(loader.id)"
        >
          <LoaderIcon :id="loader.id" size="sm" />
          <span class="nm">{{ loader.name }}</span>
          <i :class="['dot', loader.state]" />
        </button>
      </nav>
    </aside>

    <section class="lj-c2">
      <div class="hd">
        <template v-if="activeLoader">
          <LoaderIcon :id="activeLoader.id" size="sm" />
          <b>{{ activeLoader.name }}</b>
          <StateChip :state="activeLoader.state" />
        </template>
        <template v-else>没有分配到引导器</template>
      </div>
      <div class="bd">
        <BootList v-if="activeLoader" :loader="activeLoader.id" @select="(s) => onSelect?.(s)" />
        <LoaderSettings v-if="activeLoader" :loader="activeLoader.id" />
        <div v-if="healthText" class="lj-note">{{ healthText }}</div>
      </div>
    </section>

    <!-- 右侧设置栏（旧版 aside.lj-c3 > .hd + .bd） -->
    <aside class="lj-c3">
      <div class="hd">
        <svg class="i sm" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true">
          <circle cx="8" cy="8" r="2.4" />
          <path d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M12.4 3.6 11 5M5 11l-1.4 1.4" />
        </svg>
        <b>引导器设置</b>
        <span class="grow" />
        <button type="button" class="btn icon ghost sm" :aria-expanded="!collapsed" title="收起设置栏（只留图标条）" @click="collapsed = !collapsed">
          {{ collapsed ? "‹" : "›" }}
        </button>
      </div>
      <div class="bd">
        <!-- 有设置项时放设置正文；没有则显示空态（旧版 .lj-setempty） -->
        <div v-if="!settingsSlot" class="lj-setempty">
          <b>这个引导器没有可调设置</b>
          <small>在中间列表里点一条，即为下次启动</small>
        </div>
        <slot v-else name="settings" />
      </div>
    </aside>

    <aside :class="['lj-c3', { 'is-collapsed': settingsCollapsed }]">
      <div class="hd">
        <span aria-hidden="true">⚙</span>
        <b>引导器设置</b>
        <span class="grow" />
        <button
          type="button"
          class="btn icon ghost sm"
          title="收起设置栏（只留图标条）"
          aria-label="收起"
          @click="settingsCollapsed = !settingsCollapsed"
        >
          {{ settingsCollapsed ? "‹" : "›" }}
        </button>
      </div>
      <div v-show="!settingsCollapsed" class="bd">
        <div class="lj-setempty"><b>配置见中间引导列表</b><small>以实际引导器的引导列表配置为准</small></div>
      </div>
    </aside>
  </div>
</template>

<!-- 样式来自 styles/vendor/legacy.css，这里不写自造样式 -->
