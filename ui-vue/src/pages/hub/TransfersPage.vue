<script setup lang="ts">
import { onMounted, ref } from "vue";

/**
 * 传输页（原 `ui/hub.html` 的 `#page-transfers`）。
 *
 * 显示「文件」窗口的 SFTP 实时队列：下载 / 上传进度、暂停/继续/放弃。
 * 队列数据由文件窗通过 Tauri 事件推过来（阶段 4 的文件窗完成后接入）。
 */

interface TxItem {
  id: string;
  name: string;
  direction: "up" | "down";
  total: number;
  done: number;
  state: string;
}

const items = ref<TxItem[]>([]);
const loading = ref(true);

onMounted(() => {
  // 阶段 4 文件窗完成后，这里改成订阅 tx 事件流
  items.value = [];
  loading.value = false;
});

function pct(item: TxItem): number {
  if (!item.total) return 0;
  return Math.min(100, Math.round((item.done / item.total) * 100));
}
</script>

<template>
  <div class="tx-page">
    <div class="tx-head">
      <div>
        <h2>传输</h2>
        <span class="sub">
          「文件」窗口的 SFTP 实时队列 · 暂停后可从断点继续（应用重启也不丢），放弃会清掉半截文件
        </span>
      </div>
      <span class="spacer" />
      <button type="button" class="btn sm" disabled>全部暂停</button>
      <button type="button" class="btn sm" disabled>全部继续</button>
      <button type="button" class="btn sm" disabled>全部放弃</button>
      <button type="button" class="btn sm" disabled>清空已完成</button>
    </div>

    <p v-if="loading" class="hint">读取中…</p>
    <div v-else-if="items.length" class="tx-list">
      <div v-for="item in items" :key="item.id" class="tx-row">
        <span class="dir">{{ item.direction === "up" ? "↑" : "↓" }}</span>
        <span class="name">{{ item.name }}</span>
        <div class="track"><div class="fill" :style="{ width: pct(item) + '%' }" /></div>
        <span class="pct">{{ pct(item) }}%</span>
        <span class="state">{{ item.state }}</span>
      </div>
    </div>
    <div v-else class="empty">
      <p class="t">暂无传输</p>
      <p class="s">在「文件」页上传或下载文件后，这里会显示实时队列。</p>
    </div>
  </div>
</template>

<style scoped>
.tx-page {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.tx-head {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.tx-head h2 {
  font-size: 16px;
  margin: 0;
}
.sub {
  font-size: 12px;
  color: var(--fg-faint, #9ca3af);
  display: block;
  margin-top: 2px;
}
.spacer {
  flex: 1;
}
.tx-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.tx-row {
  display: grid;
  grid-template-columns: 20px 1fr 140px 46px 70px;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 8px;
  background: var(--panel, #fff);
  font-size: 12.5px;
}
.dir {
  color: var(--fg-faint, #9ca3af);
}
.track {
  height: 5px;
  border-radius: 3px;
  background: var(--soft, #f1f5f9);
  overflow: hidden;
}
.fill {
  height: 100%;
  background: var(--accent, #2563eb);
}
.pct,
.state {
  font-size: 11px;
  color: var(--fg-faint, #9ca3af);
  font-family: ui-monospace, Menlo, monospace;
}
.hint,
.empty .s {
  font-size: 12.5px;
  color: var(--fg-faint, #9ca3af);
}
.empty {
  padding: 40px 0;
  text-align: center;
}
.empty .t {
  font-size: 14px;
  color: var(--fg-dim, #6b7280);
  margin: 0 0 6px;
}
</style>
