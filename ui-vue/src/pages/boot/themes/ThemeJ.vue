<script setup lang="ts">
import { computed, ref, watch } from "vue";
import BootList from "@/components/boot/BootList.vue";
import LoaderIcon from "./LoaderIcon.vue";
import StateChip from "./StateChip.vue";
import type { LoaderInfo } from "./types";

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
        <div v-if="healthText" class="lj-note">{{ healthText }}</div>
      </div>
    </section>

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
        <p class="settings-hint">阶段 2 暂未迁移设置面板，可到「引导高级设置」里改</p>
      </div>
    </aside>
  </div>
</template>

<style scoped>
.lj-nav button {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
}
.lj-nav button .nm {
  flex: 1;
  min-width: 0;
  text-align: left;
}
.lj-note {
  margin-top: 10px;
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 8px;
  padding: 8px 10px;
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
}
.settings-hint {
  font-size: 12px;
  color: var(--fg-faint, #9ca3af);
  padding: 10px 12px;
  margin: 0;
}
</style>
