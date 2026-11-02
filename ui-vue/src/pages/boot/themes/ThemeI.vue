<script setup lang="ts">
import { computed, ref } from "vue";
import BootList from "@/components/boot/BootList.vue";
import LoaderIcon from "./LoaderIcon.vue";
import type { LoaderInfo } from "./types";

/**
 * 主题 I：Bento 磁贴 + 底部抽屉（`.li`）。
 *
 * 对齐旧 `boot-adv.js` 的 `THEME_RENDERERS.I`：
 *
 * ```
 * .li
 *   .li-grid        ← 网格：第一格是 hero（当前默认项），其余每个引导器一格
 *     .li-hero
 *       .v           ← 大字：当前默认项
 *       .s           ← 来源行「rEFInd → ESP」
 *       .acts        ← 两个动作按钮
 *     .li-tile       ← 引导器磁贴（整块可点，打开底部抽屉）
 *       .top         图标 + 名称 + 状态点
 *       .big         条目数（大字）
 *       .s           默认 · xxx
 *       .foot        权限胶囊 + 箭头
 *   （抽屉）。li-sheet   ← 点磁贴后从底部弹出，里面是完整的引导列表
 * ```
 *
 * 2026-11-01 用户定案：hero 里那行标签去掉（页面与设置预演重复出现、纯占位），
 * 数值与来源行保留。
 */

const props = defineProps<{
  loaders: LoaderInfo[];
  activeId?: string;
  onSelect?: (id: string) => void;
}>();

const sheetLid = ref<string | null>(null);

const heroLoader = computed(() => props.loaders[0] ?? null);

/** hero 大字：第一个引导器的默认项 */
const heroValue = computed(() => {
  const loader = heroLoader.value;
  if (!loader) return "—";
  const id = loader.id;
  // 与 BootList 同口径：自动模式看 lastBoot，手动看 vars 的 active
  return id;
});

const heroSource = computed(() => {
  const loader = heroLoader.value;
  if (!loader) return "没有启用的引导器";
  return `${loader.name} → ${loader.esp || "ESP"}`;
});

const sheetLoader = computed(
  () => props.loaders.find((item) => item.id === sheetLid.value) ?? null,
);

function openSheet(id: string) {
  sheetLid.value = id;
}

function closeSheet() {
  sheetLid.value = null;
}
</script>

<template>
  <div class="li">
    <div class="li-grid">
      <!-- hero：当前默认项 -->
      <div class="li-hero">
        <div>
          <div class="v">{{ heroValue }}</div>
          <div class="s">{{ heroSource }}</div>
          <div class="acts">
            <button type="button" class="btn sm">取消一次性启动</button>
            <button type="button" class="btn sm ghost">换成默认</button>
          </div>
        </div>
        <div class="li-tile" style="background: transparent; border: 0; padding: 8px 0 0">
          <div class="s">BootNext 用后自动失效；不改固件顺序</div>
        </div>
      </div>

      <!-- 每个引导器一格磁贴 -->
      <div
        v-for="loader in loaders"
        :key="loader.id"
        class="li-tile is-clickable"
        @click="openSheet(loader.id)"
      >
        <div class="top">
          <LoaderIcon :id="loader.id" size="sm" />
          <span class="nm">{{ loader.name }}</span>
          <i :class="['dot', loader.state]" />
        </div>
        <div class="big">{{ loader.entries.length }}<small>项</small></div>
        <div class="s">默认 · —</div>
        <div class="foot">
          <span :class="['chip', loader.perm === 'nvram' ? 'elev' : 'off']">
            {{ loader.perm === "nvram" ? "需提权" : "免提权" }}
          </span>
          <span class="grow" />
          <button
            type="button"
            class="btn icon ghost sm"
            :title="`查看 ${loader.name}（打开底部抽屉并定位）`"
            aria-label="查看"
            @click.stop="openSheet(loader.id)"
          >
            →
          </button>
        </div>
      </div>

      <p v-if="!loaders.length" class="empty">
        <b>没有启用的引导器</b>
        <span>去「引导高级设置」勾选要显示的引导器</span>
      </p>
    </div>

    <!-- 底部抽屉：完整引导列表 -->
    <Teleport to="body">
      <div v-if="sheetLoader" class="li-sheet-backdrop" @click.self="closeSheet">
        <div class="li-sheet">
          <header>
            <LoaderIcon :id="sheetLoader.id" size="sm" />
            <b>{{ sheetLoader.name }}</b>
            <span class="grow" />
            <button type="button" class="btn icon ghost sm" @click="closeSheet">✕</button>
          </header>
          <div class="li-sheet-body">
            <BootList :loader="sheetLoader.id" @select="(s) => onSelect?.(s)" />
          </div>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.empty {
  grid-column: 1 / -1;
  padding: 24px 12px;
  text-align: center;
  color: var(--fg-faint, #9ca3af);
  font-size: 12px;
}
.empty b {
  display: block;
  margin-bottom: 4px;
  color: var(--fg-dim, #6b7280);
}
.li-sheet-backdrop {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.32);
  display: flex;
  align-items: flex-end;
  z-index: 60;
}
.li-sheet {
  width: 100%;
  max-height: 72vh;
  background: var(--panel, #fff);
  border-radius: 12px 12px 0 0;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
.li-sheet header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--line, #e5e7eb);
}
.li-sheet-body {
  overflow-y: auto;
  padding: 8px 12px 16px;
}
</style>
