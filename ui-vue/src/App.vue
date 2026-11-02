<script setup lang="ts">
import { onMounted, ref } from "vue";

/**
 * 阶段 0 探针页。
 *
 * 目的：证明 Vue 应用在 Tauri 里能跑、IPC 能通、打包路径正确。
 * 真实页面在阶段 2/3/4 替换本文件。
 */

interface ProbeResult {
  label: string;
  ok: boolean;
  detail: string;
}

const results = ref<ProbeResult[]>([]);
const entryName = ref("");

function add(label: string, ok: boolean, detail: string) {
  results.value.push({ label, ok, detail });
}

onMounted(async () => {
  entryName.value = location.pathname.split("/").pop() || "index.html";
  add("页面加载", true, location.href);

  const isTauri = "__TAURI_INTERNALS__" in window || "__TAURI__" in window;
  add("Tauri 环境", isTauri, isTauri ? "已检测到 IPC 通道" : "非 Tauri（浏览器里打开）");

  if (!isTauri) return;

  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const state = await invoke<{ revision?: number; status?: { ok?: boolean } }>(
      "get_ui_state",
    );
    add(
      "IPC · get_ui_state",
      true,
      `revision=${state?.revision ?? "?"} status.ok=${state?.status?.ok ?? "?"}`,
    );
  } catch (error) {
    add("IPC · get_ui_state", false, String(error));
  }

  try {
    const { invoke } = await import("@tauri-apps/api/core");
    // 用真实存在的命令：hub 入口读服务器列表，其余入口读访问提示
    const list = await invoke<unknown[]>("hub_list_servers");
    add("IPC · hub_list_servers", true, `返回 ${Array.isArray(list) ? list.length : "?"} 条`);
  } catch (error) {
    add("IPC · hub_list_servers", false, String(error));
  }
});
</script>

<template>
  <main class="probe">
    <h1>AnyBoot · Vue 迁移阶段 0</h1>
    <p class="entry">入口：<code>{{ entryName }}</code></p>

    <ul class="checks">
      <li v-for="item in results" :key="item.label" :class="{ ok: item.ok, bad: !item.ok }">
        <span class="dot" />
        <strong>{{ item.label }}</strong>
        <span class="detail">{{ item.detail }}</span>
      </li>
    </ul>

    <p v-if="!results.length" class="waiting">检测中…</p>
  </main>
</template>

<style scoped>
.probe {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  padding: 32px;
  color: #1a1a1a;
}
h1 {
  font-size: 20px;
  margin: 0 0 8px;
}
.entry {
  color: #666;
  font-size: 13px;
  margin: 0 0 20px;
}
.checks {
  list-style: none;
  padding: 0;
  margin: 0;
  max-width: 720px;
}
.checks li {
  display: flex;
  align-items: baseline;
  gap: 10px;
  padding: 9px 12px;
  border-radius: 8px;
  border: 1px solid #e5e7eb;
  margin-bottom: 8px;
  font-size: 13px;
}
.checks li.ok {
  background: #f0fdf4;
  border-color: #bbf7d0;
}
.checks li.bad {
  background: #fef2f2;
  border-color: #fecaca;
}
.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex: none;
  background: #16a34a;
}
.bad .dot {
  background: #dc2626;
}
.detail {
  color: #666;
  margin-left: auto;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  word-break: break-all;
}
.waiting {
  color: #999;
  font-size: 13px;
}
</style>
