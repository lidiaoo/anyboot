<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 传输页（原 `ui/hub.html` 的 `#page-transfers`，3371 行）。
 *
 * 1:1 结构：
 * ```
 * .page-head（flex）> div(h2 + .sub) + span(flex:1) + 4 × button.btn
 * .tx-list.scroll-thin    动态队列
 * ```
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
        index >= 0 ? items.value.map((t, i) => (i === index ? task : t)) : [...items.value, task];
    }),
  );
  // 兜底轮询（任务在后台跑，窗口可能刚开）
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

async function ctlAll(action: "pause" | "resume" | "abandon", filter?: (t: TxItem) => boolean) {
  const targets = filter ? items.value.filter(filter) : items.value;
  await Promise.all(targets.map((t) => ctl(t.key, action)));
}

function clearDone() {
  items.value = items.value.filter((t) => t.state !== "done");
}

function pct(item: TxItem): number {
  if (!item.total) return 0;
  return Math.min(100, Math.round((item.bytes / item.total) * 100));
}
</script>

<template>
  <div class="page-head" style="display: flex; align-items: center; gap: 10px">
    <div>
      <h2>传输</h2>
      <span class="sub"
        >「文件」窗口的 SFTP 实时队列 · 暂停后可从断点继续（应用重启也不丢），放弃会清掉半截文件</span
      >
    </div>
    <span style="flex: 1" />
    <button type="button" class="btn" @click="ctlAll('pause', (t) => t.state === 'running')">
      全部暂停
    </button>
    <button type="button" class="btn" @click="ctlAll('resume', (t) => t.state === 'paused')">
      全部继续
    </button>
    <button type="button" class="btn" @click="ctlAll('abandon', (t) => t.state !== 'done')">
      全部放弃
    </button>
    <button type="button" class="btn" @click="clearDone">清空已完成</button>
  </div>

  <div class="tx-list scroll-thin">
    <p v-if="errorText" class="field-hint" style="color: rgb(var(--bad))">{{ errorText }}</p>
    <p v-else-if="loading" class="field-hint">读取中…</p>
    <template v-else-if="items.length">
      <div v-for="item in items" :key="item.key" class="tx-row">
        <span class="dir">{{ item.dir === "up" ? "↑" : "↓" }}</span>
        <span class="tx-name">{{ item.name }}</span>
        <div class="tx-track"><div class="tx-fill" :style="{ width: pct(item) + '%' }" /></div>
        <span class="mono tx-pct">{{ pct(item) }}%</span>
        <span class="tx-state">{{ item.state }}</span>
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
          class="link-btn"
          @click="ctl(item.key, 'abandon')"
        >
          放弃
        </button>
      </div>
    </template>
    <div v-else class="act-card" style="margin: 8px 0">
      <p class="t">暂无传输</p>
      <p class="s">在「文件」页上传或下载文件后，这里会显示实时队列。</p>
    </div>
  </div>
</template>
