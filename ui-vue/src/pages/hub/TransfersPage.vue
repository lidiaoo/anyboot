<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 传输页（原 `ui/hub.html` 的 `#page-transfers`）。
 *
 * 显示「文件」窗口的 SFTP 实时队列：下载 / 上传进度、暂停/继续/放弃。
 * 队列数据由文件窗通过 Tauri 事件推过来（阶段 4 的文件窗完成后接入）。
 */

/** 对应 Rust `hub_sftp::XferTask`（camelCase） */
interface TxItem {
  key: string;
  dir: "up" | "down";
  name: string;
  remotePath: string;
  localPath: string;
  total: number;
  bytes: number;
  paused: boolean;
  state: string;
  serverName?: string;
}

const items = ref<TxItem[]>([]);
const loading = ref(true);
const errorText = ref("");
const unlisteners: (() => void)[] = [];

async function refresh() {
  try {
    items.value = (await call<TxItem[]>("hub_sftp_transfer_list")) ?? [];
  } catch (error) {
    errorText.value = String(error);
    items.value = [];
  } finally {
    loading.value = false;
  }
}

onMounted(async () => {
  await refresh();
  const { listen } = await import("@tauri-apps/api/event");
  unlisteners.push(
    await listen<TxItem>("hub-sftp-progress", (event) => {
      const task = event.payload;
      const index = items.value.findIndex((t) => t.key === task.key);
      items.value =
        index >= 0
          ? items.value.map((t, i) => (i === index ? task : t))
          : [...items.value, task];
    }),
  );
  // 兜底轮询：事件丢了也能对上（任务在后台跑，窗口可能刚开）
  const timer = window.setInterval(() => void refresh(), 3000);
  unlisteners.push(() => window.clearInterval(timer));
});

onUnmounted(() => {
  for (const off of unlisteners) off();
});

async function ctl(key: string, action: "pause" | "resume" | "abandon") {
  try {
    await call("hub_sftp_transfer_ctl", { key, action });
    await refresh();
  } catch (error) {
    errorText.value = String(error);
  }
}

/** 批量操作：对当前列表里符合条件的任务逐个下发 */
async function ctlAll(action: "pause" | "resume" | "abandon", filter?: (t: TxItem) => boolean) {
  const targets = filter ? items.value.filter(filter) : items.value;
  await Promise.all(targets.map((t) => ctl(t.key, action)));
}

function clearDone() {
  // 「清空已完成」在界面上先滤掉，后端清单由任务自身生命周期管理
  items.value = items.value.filter((t) => t.state !== "done");
}

function pct(item: TxItem): number {
  if (!item.total) return 0;
  return Math.min(100, Math.round((item.bytes / item.total) * 100));
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
      <button type="button" class="btn sm" @click="ctlAll('pause', (t) => t.state === 'running')">
        全部暂停
      </button>
      <button type="button" class="btn sm" @click="ctlAll('resume', (t) => t.state === 'paused')">
        全部继续
      </button>
      <button type="button" class="btn sm" @click="ctlAll('abandon', (t) => t.state !== 'done')">
        全部放弃
      </button>
      <button type="button" class="btn sm" @click="clearDone">清空已完成</button>
    </div>

    <p v-if="errorText" class="err">{{ errorText }}</p>
    <p v-if="loading" class="hint">读取中…</p>
    <div v-else-if="items.length" class="tx-list">
      <div v-for="item in items" :key="item.key" class="tx-row">
        <span class="dir">{{ item.dir === "up" ? "↑" : "↓" }}</span>
        <span class="name">{{ item.name }}</span>
        <div class="track"><div class="fill" :style="{ width: pct(item) + '%' }" /></div>
        <span class="pct">{{ pct(item) }}%</span>
        <span class="state">{{ item.state }}</span>
        <button
          v-if="item.state === 'running'"
          type="button"
          class="link-btn"
          @click="ctl(item.key, 'pause')"
        >
          暂停
        </button>
        <button
          v-else-if="item.state === 'paused'"
          type="button"
          class="link-btn"
          @click="ctl(item.key, 'resume')"
        >
          继续
        </button>
        <button
          v-if="item.state !== 'done'"
          type="button"
          class="link-btn danger"
          @click="ctl(item.key, 'abandon')"
        >
          放弃
        </button>
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
.link-btn {
  border: 0;
  background: transparent;
  color: var(--accent, #2563eb);
  font-size: 11.5px;
  cursor: pointer;
  padding: 0;
}
.link-btn.danger {
  color: #dc2626;
}
.err {
  font-size: 12.5px;
  color: #dc2626;
  margin: 0;
}
.tx-row {
  display: grid;
  grid-template-columns: 20px 1fr 120px 42px 60px auto auto;
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
