<script setup lang="ts">
import { onMounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 会话页（原 `ui/hub.html` 的 `#page-sessions`，3365 行）。
 *
 * 1:1 结构：
 * ```
 * .page-head > h2「会话」 + span.sub「本窗口打开的终端 · 运行中的隧道」
 * .session-list.scroll-thin   动态列表
 * ```
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
  <div class="page-head">
    <h2>会话</h2>
    <span class="sub">本窗口打开的终端 · 运行中的隧道</span>
  </div>

  <div class="session-list scroll-thin">
    <p v-if="loading" class="field-hint">读取中…</p>
    <template v-else-if="sessions.length">
      <p class="session-sec">进行中的终端会话</p>
      <div v-for="s in sessions" :key="s.id" class="session-row">
        <span class="mono">{{ s.label || s.server || s.id }}</span>
        <span style="flex: 1" />
        <span class="mono field-hint">{{ fmtTime(s.startedAt) }}</span>
      </div>
    </template>
    <div v-else class="act-card" style="margin: 8px 0">
      <p class="t">暂无会话</p>
      <p class="s">在「服务器」页打开终端、或启动隧道后，这里会显示本窗口的真实会话记录。</p>
    </div>
  </div>
</template>
