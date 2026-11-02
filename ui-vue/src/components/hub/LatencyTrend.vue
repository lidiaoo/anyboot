<script setup lang="ts">
import { computed, ref } from "vue";

/**
 * 延迟趋势（原 `ui/hub.html` 的「延迟趋势（会话内采样）」）。
 *
 * 会话内采样：每次测试连接把耗时推入环形缓冲（最多 40 个点），
 * 画成一条折线。切走再回来就丢了 —— 这是**会话内**趋势，不落盘。
 */

const MAX_POINTS = 40;

const points = ref<number[]>([]);

/** 记录一次采样（毫秒） */
function push(ms: number) {
  if (!Number.isFinite(ms) || ms < 0) return;
  const next = [...points.value, Math.round(ms)];
  points.value = next.length > MAX_POINTS ? next.slice(next.length - MAX_POINTS) : next;
}

function reset() {
  points.value = [];
}

defineExpose({ push, reset });

const stats = computed(() => {
  const list = points.value;
  if (!list.length) return null;
  const min = Math.min(...list);
  const max = Math.max(...list);
  const avg = Math.round(list.reduce((sum, v) => sum + v, 0) / list.length);
  return { min, max, avg, last: list[list.length - 1] ?? 0 };
});

/** 折线路径（viewBox 0 0 200 40） */
const path = computed(() => {
  const list = points.value;
  if (list.length < 2) return "";
  const max = Math.max(...list, 1);
  const step = 200 / (list.length - 1);
  return list
    .map((value, index) => {
      const x = (index * step).toFixed(1);
      const y = (40 - (value / max) * 36).toFixed(1);
      return `${index === 0 ? "M" : "L"}${x},${y}`;
    })
    .join(" ");
});
</script>

<template>
  <div class="latency">
    <div class="head">
      <span class="lbl">延迟趋势</span>
      <span v-if="stats" class="stats">
        最近 <b>{{ stats.last }}</b> ms · 均 {{ stats.avg }} · 低 {{ stats.min }} · 高 {{ stats.max }}
      </span>
      <span v-else class="stats dim">还没有采样 —— 点「测试连接」开始</span>
    </div>
    <svg v-if="path" class="spark" viewBox="0 0 200 40" preserveAspectRatio="none" aria-hidden="true">
      <path :d="path" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" />
    </svg>
  </div>
</template>

<style scoped>
.latency {
  margin-top: 12px;
}
.head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  font-size: 11.5px;
}
.lbl {
  color: var(--fg-faint, #9ca3af);
  text-transform: uppercase;
  letter-spacing: 0.06em;
}
.stats {
  color: var(--fg-dim, #6b7280);
}
.stats b {
  color: var(--fg, #111827);
}
.dim {
  color: var(--fg-faint, #9ca3af);
}
.spark {
  width: 100%;
  height: 40px;
  margin-top: 6px;
  color: var(--accent, #2563eb);
  background: var(--soft, #f9fafb);
  border-radius: 8px;
}
</style>
