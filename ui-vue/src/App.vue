<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { useTransport } from "@/composables/useTransport";

/**
 * 阶段 1 验收页：证明「类型 + 数据层 + 传输通道」都能用。
 *
 * 真实页面在阶段 2/3/4 替换本文件。
 * 验收要点：
 *   ① 快照能到达（revision 递增）
 *   ② 三种通道可切（IPC / ws / http）
 *   ③ 设置的两个来源都读得到（后端 settings + 本地偏好）
 */

const ui = useUiStateStore();
const settings = useSettingsStore();
const transport = useTransport();

const entryName = ref("");
const log = ref<string[]>([]);

function pushLog(text: string) {
  log.value.unshift(`${new Date().toLocaleTimeString()}  ${text}`);
  if (log.value.length > 12) log.value.pop();
}

onMounted(async () => {
  entryName.value = location.pathname.split("/").pop() || "index.html";
  await ui.start();
  pushLog(`通道：${transport.activeTransport.value}（${transport.label.value}）`);

  // macOS 上没法 attach WebKit devtools，验收信息写进 App 的 trace 日志。
  // 3 秒后（首份快照应该已到达）打一行，格式固定便于 grep：
  //   [vue-stage1] ready=1 rev=... refind=auto grub=auto channels=3 grubEntries=68 simEntries=15
  window.setTimeout(() => {
    const line =
      `[vue-stage1] ready=${ui.ready ? 1 : 0} rev=${ui.revision} ` +
      `transport=${transport.activeTransport.value} ` +
      `refind=${settings.refindMode} grub=${settings.grubMode} ` +
      `channels=${ui.channels.length} grubEntries=${ui.grub?.entries.length ?? 0} ` +
      `simEntries=${ui.refindSim?.entries.length ?? 0} ` +
      `httpListening=${ui.http?.listening ? 1 : 0}`;
    void import("@/api/tauri")
      .then(({ isTauri, call }) => {
        if (isTauri()) return call<void>("hub_report_error", { source: "vue", message: line, stack: "" });
        return undefined;
      })
      .catch(() => {
        /* 日志失败不影响页面 */
      });
  }, 3000);
});

const snapshot = computed(() => ui.state);
const refindMode = computed(() => settings.refindMode);
const grubMode = computed(() => settings.grubMode);
const channelsCount = computed(() => ui.channels.length);
const grubEntries = computed(() => ui.grub?.entries.length ?? 0);
const simEntries = computed(() => ui.refindSim?.entries.length ?? 0);
</script>

<template>
  <main class="probe">
    <header>
      <h1>AnyBoot · Vue 阶段 1 · 数据层</h1>
      <p class="entry">
        入口 <code>{{ entryName }}</code>
        · 通道 <strong>{{ transport.label.value }}</strong>
        · revision <code>{{ ui.revision }}</code>
      </p>
    </header>

    <section>
      <h2>快照</h2>
      <ul class="kv">
        <li><span>是否就绪</span><b :class="ui.ready ? 'ok' : 'bad'">{{ ui.ready ? "是" : "否" }}</b></li>
        <li><span>rEFInd 模式</span><b>{{ refindMode }}</b></li>
        <li><span>GRUB 模式</span><b>{{ grubMode }}</b></li>
        <li><span>rEFInd 模拟扫描条目</span><b>{{ simEntries }}</b></li>
        <li><span>GRUB 条目</span><b>{{ grubEntries }}</b></li>
        <li><span>三通道</span><b>{{ channelsCount }}</b></li>
        <li><span>HTTP 服务</span><b>{{ ui.http?.listening ? "监听中" : "未监听" }}</b></li>
      </ul>
    </section>

    <section>
      <h2>前端偏好（localStorage）</h2>
      <ul class="kv">
        <li><span>数据刷新</span><b>{{ settings.dataRefreshSec }} 秒</b></li>
        <li><span>每次重探地址</span><b>{{ settings.reprobeAlways ? "是" : "否（复用）" }}</b></li>
        <li><span>连续失败阈值</span><b>{{ settings.offlineRetryCount }} 次</b></li>
        <li><span>失败重载</span><b>{{ settings.failReloadSec }} 秒</b></li>
      </ul>
    </section>

    <section>
      <h2>通道切换</h2>
      <div class="buttons">
        <button @click="transport.switchTo('auto')">自动</button>
        <button @click="transport.switchTo('ws')">推送</button>
        <button @click="transport.switchTo('http')">轮询</button>
        <button @click="transport.reconnect()">重连</button>
      </div>
      <p v-if="ui.pushError" class="err">{{ ui.pushError }}</p>
    </section>

    <section>
      <h2>事件</h2>
      <ul class="log">
        <li v-for="(line, i) in log" :key="i">{{ line }}</li>
      </ul>
    </section>

    <p v-if="!snapshot" class="waiting">等待快照…</p>
  </main>
</template>

<style scoped>
.probe {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  padding: 28px 32px;
  color: #1a1a1a;
  max-width: 860px;
}
h1 {
  font-size: 19px;
  margin: 0 0 6px;
}
h2 {
  font-size: 13px;
  margin: 22px 0 8px;
  color: #6b7280;
  text-transform: uppercase;
  letter-spacing: 0.08em;
}
.entry {
  color: #6b7280;
  font-size: 12.5px;
  margin: 0;
}
code {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  background: #f3f4f6;
  padding: 1px 5px;
  border-radius: 4px;
  font-size: 11.5px;
}
.kv {
  list-style: none;
  padding: 0;
  margin: 0;
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  overflow: hidden;
}
.kv li {
  display: flex;
  justify-content: space-between;
  padding: 8px 12px;
  font-size: 13px;
  border-bottom: 1px solid #f3f4f6;
}
.kv li:last-child {
  border-bottom: 0;
}
.kv span {
  color: #6b7280;
}
.kv b.ok {
  color: #16a34a;
}
.kv b.bad {
  color: #dc2626;
}
.buttons {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
button {
  font-size: 12.5px;
  padding: 6px 14px;
  border-radius: 7px;
  border: 1px solid #d1d5db;
  background: #fff;
  cursor: pointer;
}
button:hover {
  background: #f9fafb;
}
.log {
  list-style: none;
  padding: 10px 12px;
  margin: 0;
  background: #f9fafb;
  border-radius: 8px;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 11.5px;
  color: #4b5563;
  line-height: 1.7;
}
.err {
  color: #dc2626;
  font-size: 12.5px;
  margin: 8px 0 0;
}
.waiting {
  color: #9ca3af;
  font-size: 13px;
  margin-top: 20px;
}
</style>
