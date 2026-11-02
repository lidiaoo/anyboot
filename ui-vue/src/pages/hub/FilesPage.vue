<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { call } from "@/api/tauri";
import { useServersStore } from "@/stores/servers";

/**
 * 文件页（原 `ui/hub.html` 的 `#page-files`）—— 内嵌 SFTP 文件浏览器。
 *
 * 与独立窗 `hub-files.html` 共用后端 `hub_sftp` 命令族；这里是主窗内的第二入口。
 * 结构：顶栏（服务器选择 + 连接状态 + 独立窗按钮）→ 路径栏 → 文件列表。
 */

interface Entry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  mtime: number;
}

const store = useServersStore();
const serverId = ref("");
const cwd = ref("");
const entries = ref<Entry[]>([]);
const loading = ref(false);
const errorText = ref("");

const sshServers = computed(() => store.servers.filter((s) => s.protocol === "ssh"));

onMounted(async () => {
  if (!store.loaded) await store.load();
  const first = sshServers.value[0];
  if (first) {
    serverId.value = first.id;
    await listDir("");
  }
});

async function listDir(path: string) {
  if (!serverId.value) return;
  loading.value = true;
  errorText.value = "";
  try {
    const result = await call<{ cwd: string; entries: Entry[] }>("hub_sftp_list", {
      id: serverId.value,
      path,
    });
    cwd.value = result?.cwd ?? path;
    entries.value = result?.entries ?? [];
  } catch (error) {
    errorText.value = String(error);
    entries.value = [];
  } finally {
    loading.value = false;
  }
}

function open(entry: Entry) {
  if (entry.is_dir) void listDir(entry.path);
}

async function goUp() {
  const parent = cwd.value.replace(/\/[^/]+\/?$/, "") || "/";
  await listDir(parent);
}

async function openInWindow() {
  if (!serverId.value) return;
  try {
    await call("hub_open_window", { label: "server-files", serverId: serverId.value });
  } catch {
    /* 打开失败不阻塞 */
  }
}

function fmtSize(bytes: number): string {
  if (!bytes) return "";
  const units = ["B", "K", "M", "G"];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return `${value.toFixed(value < 10 && i > 0 ? 1 : 0)}${units[i]}`;
}
</script>

<template>
  <div class="files-page">
    <div class="topbar">
      <select v-model="serverId" class="srv-select" @change="listDir('')">
        <option v-for="server in sshServers" :key="server.id" :value="server.id">
          {{ server.name }} · {{ store.displayAddr(server) }}
        </option>
      </select>
      <span :class="['pill', errorText ? 'bad' : 'ok']">
        {{ errorText ? "连接失败" : loading ? "读取中…" : "已连接" }}
      </span>
      <span class="spacer" />
      <button type="button" class="btn sm" @click="openInWindow">⧉ 独立窗口</button>
    </div>

    <div class="pathbar">
      <button type="button" class="btn sm" :disabled="!cwd || cwd === '/'" @click="goUp">↑ 上一级</button>
      <button type="button" class="btn sm" :disabled="loading" @click="listDir(cwd)">刷新</button>
      <span class="cwd mono">{{ cwd || "/" }}</span>
    </div>

    <p v-if="errorText" class="err">{{ errorText }}</p>

    <div class="list">
      <button
        v-for="entry in entries"
        :key="entry.path"
        type="button"
        class="file-row"
        @click="open(entry)"
      >
        <span class="ico">{{ entry.is_dir ? "📁" : "📄" }}</span>
        <span class="name">{{ entry.name }}</span>
        <span class="size mono">{{ entry.is_dir ? "" : fmtSize(entry.size) }}</span>
      </button>
      <p v-if="!loading && !entries.length && !errorText" class="empty">这个目录是空的</p>
    </div>

    <p v-if="!sshServers.length" class="empty-hint">
      还没有 SSH 服务器 —— 到「服务器」页添加一台，这里就能浏览它的文件。
    </p>
  </div>
</template>

<style scoped>
.files-page {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 12px 16px;
  gap: 10px;
}
.topbar,
.pathbar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.srv-select {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 8px;
  padding: 5px 10px;
  font-size: 12.5px;
  background: var(--panel, #fff);
  max-width: 320px;
}
.pill {
  font-size: 11px;
  padding: 2px 9px;
  border-radius: 999px;
}
.pill.ok {
  background: #dcfce7;
  color: #15803d;
}
.pill.bad {
  background: #fee2e2;
  color: #b91c1c;
}
.spacer {
  flex: 1;
}
.cwd {
  font-size: 11.5px;
  color: var(--fg-dim, #4b5563);
  word-break: break-all;
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
}
.list {
  flex: 1;
  overflow-y: auto;
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  background: var(--panel, #fff);
}
.file-row {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  text-align: left;
  border: 0;
  border-bottom: 1px solid var(--line, #f8fafc);
  background: transparent;
  padding: 9px 14px;
  font-size: 12.5px;
  cursor: pointer;
}
.file-row:hover {
  background: var(--soft, #f9fafb);
}
.name {
  flex: 1;
  min-width: 0;
  overflow-wrap: anywhere;
}
.size {
  color: var(--fg-faint, #9ca3af);
}
.empty,
.empty-hint {
  padding: 30px 16px;
  text-align: center;
  color: var(--fg-faint, #9ca3af);
  font-size: 12.5px;
  margin: 0;
}
.err {
  font-size: 12.5px;
  color: #dc2626;
  margin: 0;
}
</style>
