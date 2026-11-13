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
/** 历史（已完成）—— 旧版 txHistory */
const history = ref<{ key?: string; name?: string; dir?: string; bytes?: number; total?: number; at?: number }[]>([]);

/** 历史持久化（旧版 txPersist：ab-transfer-history，最多 20 条） */
function historyPersist() {
  try { localStorage.setItem("ab-transfer-history", JSON.stringify(history.value.slice(0, 20))); } catch { /* 忽略 */ }
}
/** 完成入历史（旧版 txAddHistory：同 key 去重） */
function historyAdd(entry: { key?: string; name?: string; dir?: string; bytes?: number; total?: number; at?: number }) {
  for (const h of history.value) {
    if (h.key && entry.key && h.key === entry.key) return;
  }
  history.value = [entry, ...history.value].slice(0, 20);
  historyPersist();
}
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
  try { history.value = JSON.parse(localStorage.getItem("ab-transfer-history") || "[]") || []; } catch { history.value = []; }
  await refresh();
  const { listen } = await import("@tauri-apps/api/event");
  unlisteners.push(
    await listen<TxItem>("hub-sftp-progress", (event) => {
      const task = event.payload;
      const index = items.value.findIndex((t) => t.key === task.key);
      items.value =
        index >= 0 ? items.value.map((t, i) => (i === index ? task : t)) : [...items.value, task];
      // done/failed/abandoned → 入历史（旧版三态都记）
      if (task.state === "done" || task.state === "failed" || task.state === "abandoned") {
        historyAdd({ key: task.key, name: task.name, dir: task.dir, bytes: task.bytes, total: task.total, at: Date.now() });
      }
    }),
  );
  // 兜底轮询（任务在后台跑，窗口可能刚开）
  const timer = window.setInterval(() => void refresh(), 1000); // 旧版 txPollList 1000ms
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
    history.value = [];
    historyPersist();
  items.value = items.value.filter((t) => t.state !== "done");
}

/** 状态徽章文案（旧版 .tx-badge） */
function badgeText(item: TxItem): string {
  if (item.state === "paused") return item.dir === "up" ? "↑ 已暂停" : "↓ 已暂停";
  return item.dir === "up" ? "↑ 上传" : "↓ 下载";
}
function badgeClass(item: TxItem): string {
  if (item.state === "paused") return "paused";
  return item.dir === "up" ? "up" : "down";
}
/** 进度 + 速度（旧版 txSpeedTick；这里给「已传 / 总计」兜底） */
function metaText(item: TxItem): string {
  const p = item.total > 0 ? `${pct(item)}% · ` : "";
  return p + `${fmtSize(item.bytes)} / ${fmtSize(item.total)}`;
}

function fmtSize(bytes?: number): string {
  if (!bytes) return "0 B";
  const units = ["B", "KB", "MB", "GB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v < 10 && i > 0 ? 1 : 0)} ${units[i]}`;
}

function pct(item: TxItem): number {
  if (!item.total) return 0;
  return Math.min(100, Math.round((item.bytes / item.total) * 100));
}
</script>

<template>
  <div class="page-head" style="display: flex; align-items: center; gap: 10px">
    <h2>传输</h2>
    <span style="flex: 1" />
    <button type="button" id="txPauseAll" class="btn" @click="ctlAll('pause', (t) => t.state === 'running')">
      全部暂停
    </button>
    <button type="button" id="txResumeAll" class="btn" @click="ctlAll('resume', (t) => t.state === 'paused')">
      全部继续
    </button>
    <button type="button" id="txAbandonAll" class="btn" @click="ctlAll('abandon', (t) => t.state !== 'done')">
      全部放弃
    </button>
    <button type="button" id="txClearDone" class="btn" @click="clearDone">清空已完成</button>
  </div>

  <div class="tx-list scroll-thin" id="txList">
    <p v-if="errorText" class="field-hint" style="color: rgb(var(--bad))">{{ errorText }}</p>

    <!-- 空态（旧版 renderTransfers 的空态文案） -->
    <div v-else-if="!items.length && !history.length" class="act-card" style="margin: 8px 0">
      <p class="t">当前没有传输任务</p>
      <p class="s">在「文件」窗口上传 / 下载时，进度会实时汇总到这里；暂停的任务可从断点继续。</p>
    </div>

    <template v-else>
      <!-- 进行中 / 已暂停的传输（旧版 .tx-card 结构） -->
      <div v-for="item in items" :key="item.key" class="tx-card">
        <div class="tx-top">
          <span :class="['tx-badge', badgeClass(item)]">{{ badgeText(item) }}</span>
          <span class="tx-name" :title="item.name">{{ item.name || "—" }}</span>
          <span class="tx-meta">{{ metaText(item) }}</span>
          <button type="button" class="tx-mini" @click="ctl(item.key, item.state === 'paused' ? 'resume' : 'pause')">
            {{ item.state === "paused" ? "继续" : "暂停" }}
          </button>
          <button type="button" class="tx-mini danger" title="删除半截文件并清掉这条记录" @click="ctl(item.key, 'abandon')">
            放弃
          </button>
        </div>
        <div :class="['tx-bar', { paused: item.state === 'paused' }]">
          <div :style="{ width: (item.total > 0 ? pct(item) : 8) + '%' }" />
        </div>
      </div>

      <!-- 历史（已完成） -->
      <div v-for="(h, i) in history" :key="'h' + i" class="tx-card">
        <div class="tx-top">
          <span class="tx-badge done">✓ 已完成</span>
          <span class="tx-name">{{ (h.dir === "up" ? "[↑] " : "[↓] ") + (h.name || "—") }}</span>
          <span class="tx-meta">{{ fmtSize(h.total) }}</span>
        </div>
      </div>
    </template>
  </div>
</template>
