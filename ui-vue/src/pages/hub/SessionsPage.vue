<script setup lang="ts">
import { onMounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 会话页（原 `ui/hub.html` 的 `#page-sessions`）。
 *
 * 显示**本窗口的真实记录**：进行中的终端会话（后端会话表）+ 运行中的隧道。
 */

interface TermSession {
  id: string;
  label?: string;
  server?: string;
  startedAt?: number;
}

const sessions = ref<TermSession[]>([]);
const loading = ref(true);

onMounted(async () => {
  try {
    sessions.value = (await call<TermSession[]>("hub_term_sessions")) ?? [];
  } catch {
    sessions.value = [];
  } finally {
    loading.value = false;
  }
});

function fmtTime(ts?: number): string {
  if (!ts) return "";
  return new Date(ts).toLocaleTimeString("zh-CN", { hour12: false });
}
</script>

<template>
  <div class="sessions-page">
    <p v-if="loading" class="hint">读取中…</p>
    <template v-else-if="sessions.length">
      <p class="sec">进行中的终端会话</p>
      <div v-for="s in sessions" :key="s.id" class="row">
        <span class="name">{{ s.label || s.server || s.id }}</span>
        <span class="time">{{ fmtTime(s.startedAt) }}</span>
      </div>
    </template>
    <div v-else class="empty">
      <p class="t">暂无会话</p>
      <p class="s">在「服务器」页打开终端、或启动隧道后，这里会显示本窗口的真实会话记录。</p>
    </div>
  </div>
</template>

<style scoped>
.sessions-page {
  max-width: 720px;
}
.sec {
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
  text-transform: uppercase;
  letter-spacing: 0.06em;
  margin: 0 0 8px;
}
.row {
  display: flex;
  justify-content: space-between;
  padding: 9px 12px;
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 8px;
  margin-bottom: 6px;
  font-size: 12.5px;
  background: var(--panel, #fff);
}
.time {
  color: var(--fg-faint, #9ca3af);
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11px;
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
