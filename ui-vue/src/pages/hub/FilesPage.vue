<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
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

/** 对应 Rust `hub_sftp::XferTask`（camelCase） */
interface XferTask {
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

const store = useServersStore();
const serverId = ref("");
const cwd = ref("");
const entries = ref<Entry[]>([]);
const loading = ref(false);
const errorText = ref("");
/** 传输队列（hub-sftp-progress 事件 + transfer_list 轮询） */
const transfers = ref<XferTask[]>([]);
const unlisteners: (() => void)[] = [];

const sshServers = computed(() => store.servers.filter((s) => s.protocol === "ssh"));

onMounted(async () => {
  if (!store.loaded) await store.load();
  const first = sshServers.value[0];
  if (first) {
    serverId.value = first.id;
    await listDir("");
  }
  await refreshTransfers();

  const { listen } = await import("@tauri-apps/api/event");
  unlisteners.push(
    await listen<XferTask>("hub-sftp-progress", (event) => {
      const task = event.payload;
      const index = transfers.value.findIndex((t) => t.key === task.key);
      transfers.value =
        index >= 0
          ? transfers.value.map((t, i) => (i === index ? task : t))
          : [...transfers.value, task];
    }),
  );
  unlisteners.push(
    await listen<string>("hub-files-open-server", async (event) => {
      if (event.payload) {
        serverId.value = event.payload;
        await listDir("");
      }
    }),
  );
});

onUnmounted(() => {
  for (const off of unlisteners) off();
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

async function goHome() {
  try {
    const home = await call<string>("hub_sftp_home", { id: serverId.value });
    await listDir(home || "");
  } catch {
    await listDir("");
  }
}

async function refreshTransfers() {
  try {
    transfers.value = (await call<XferTask[]>("hub_sftp_transfer_list")) ?? [];
  } catch {
    transfers.value = [];
  }
}

async function download(entry: Entry) {
  try {
    await call("hub_sftp_download", { id: serverId.value, remotePath: entry.path });
    await refreshTransfers();
  } catch (error) {
    errorText.value = String(error);
  }
}

async function upload() {
  try {
    await call("hub_sftp_upload", { id: serverId.value, remoteDir: cwd.value });
    await refreshTransfers();
  } catch (error) {
    errorText.value = String(error);
  }
}

async function transferCtl(key: string, action: "pause" | "resume" | "abandon") {
  try {
    await call("hub_sftp_transfer_ctl", { key, action });
    await refreshTransfers();
  } catch (error) {
    errorText.value = String(error);
  }
}

async function mkdir() {
  const name = window.prompt("新建文件夹名称", "");
  if (!name?.trim()) return;
  const base = cwd.value.endsWith("/") ? cwd.value : `${cwd.value}/`;
  try {
    await call("hub_sftp_mkdir", { id: serverId.value, path: `${base}${name.trim()}` });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

async function removeEntry(entry: Entry) {
  if (!window.confirm(`确定删除「${entry.name}」？`)) return;
  try {
    await call("hub_sftp_remove", { id: serverId.value, path: entry.path });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

async function renameEntry(entry: Entry) {
  const next = window.prompt("新名称", entry.name);
  if (!next?.trim() || next === entry.name) return;
  try {
    await call("hub_sftp_rename", {
      id: serverId.value,
      from: entry.path,
      to: entry.path.replace(/[^/]+$/, next.trim()),
    });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

function pct(task: XferTask): number {
  if (!task.total) return 0;
  return Math.min(100, Math.round((task.bytes / task.total) * 100));
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
      <button type="button" class="btn sm" :disabled="!serverId || loading" @click="upload">上传</button>
      <button type="button" class="btn sm" :disabled="!serverId || loading" @click="mkdir">
        新建文件夹
      </button>
      <button type="button" class="btn sm" @click="goHome">家目录</button>
      <button type="button" class="btn sm" @click="openInWindow">⧉ 独立窗口</button>
    </div>

    <div class="pathbar">
      <button type="button" class="btn sm" :disabled="!cwd || cwd === '/'" @click="goUp">↑ 上一级</button>
      <button type="button" class="btn sm" :disabled="loading" @click="listDir(cwd)">刷新</button>
      <span class="cwd mono">{{ cwd || "/" }}</span>
    </div>

    <p v-if="errorText" class="err">{{ errorText }}</p>

    <div class="list">
      <div v-for="entry in entries" :key="entry.path" class="file-item">
        <button type="button" class="file-row" @click="open(entry)">
          <span class="ico">{{ entry.is_dir ? "📁" : "📄" }}</span>
          <span class="name">{{ entry.name }}</span>
          <span class="size mono">{{ entry.is_dir ? "" : fmtSize(entry.size) }}</span>
        </button>
        <span class="ops">
          <button v-if="!entry.is_dir" type="button" class="link-btn" @click="download(entry)">
            下载
          </button>
          <button type="button" class="link-btn" @click="renameEntry(entry)">重命名</button>
          <button type="button" class="link-btn danger" @click="removeEntry(entry)">删除</button>
        </span>
      </div>
      <p v-if="!loading && !entries.length && !errorText" class="empty">这个目录是空的</p>
    </div>

    <div v-if="transfers.length" class="transfers">
      <div class="tx-head">
        <span>传输队列</span>
        <span class="spacer" />
        <button type="button" class="link-btn" @click="refreshTransfers">刷新</button>
      </div>
      <div v-for="task in transfers" :key="task.key" class="tx-row">
        <span class="dir">{{ task.dir === "up" ? "↑" : "↓" }}</span>
        <span class="tx-name">{{ task.name }}</span>
        <div class="track"><div class="fill" :style="{ width: pct(task) + '%' }" /></div>
        <span class="pct mono">{{ pct(task) }}%</span>
        <span class="state">{{ task.state }}</span>
        <button
          v-if="task.state === 'running'"
          type="button"
          class="link-btn"
          @click="transferCtl(task.key, 'pause')"
        >
          暂停
        </button>
        <button
          v-else-if="task.state === 'paused'"
          type="button"
          class="link-btn"
          @click="transferCtl(task.key, 'resume')"
        >
          继续
        </button>
        <button
          v-if="task.state !== 'done'"
          type="button"
          class="link-btn danger"
          @click="transferCtl(task.key, 'abandon')"
        >
          放弃
        </button>
      </div>
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
.file-item {
  display: flex;
  align-items: center;
  border-bottom: 1px solid var(--line, #f8fafc);
  padding-right: 12px;
}
.file-item:hover {
  background: var(--soft, #f9fafb);
}
.ops {
  display: flex;
  gap: 8px;
  flex: none;
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
.transfers {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  background: var(--panel, #fff);
  padding: 10px 12px;
  max-height: 180px;
  overflow-y: auto;
}
.tx-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
  margin-bottom: 8px;
}
.tx-row {
  display: grid;
  grid-template-columns: 18px 1fr 110px 40px 56px auto auto;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  padding: 4px 0;
}
.dir {
  color: var(--fg-faint, #9ca3af);
}
.tx-name {
  overflow-wrap: anywhere;
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
