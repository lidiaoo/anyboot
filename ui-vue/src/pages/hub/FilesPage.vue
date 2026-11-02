<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { call } from "@/api/tauri";
import { useServersStore } from "@/stores/servers";

/**
 * 文件页（原 `ui/hub.html` 的 `#page-files`，3387 行）—— `hub_sftp_*` 命令族。
 *
 * ## 1:1 结构（旧版就这一套，样式全在 hub-inline.css 的 `.fb-*`）
 * ```
 * .fb-topbar   .fb-select + .fb-pill + .fb-toolbtn
 * .fb-empty
 * .fb-main
 *   .fb-pathbar  .fb-iconbtn×3 + .fb-crumbs + .fb-btn×3（新建目录/上传/下载）
 *   .fb-tablewrap > table.fb-table（thead + tbody）+ .fb-emptytip
 *   .fb-transfer （进度条 + 暂停/放弃）
 *   .fb-foot     .fb-count + 「SFTP 复用 SSH 会话」
 * ```
 */

interface SftpEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  mtime: number;
  perms?: string;
}

interface XferTask {
  key: string;
  dir: "up" | "down";
  name: string;
  total: number;
  bytes: number;
  state: string;
}

const store = useServersStore();
const serverId = ref("");
const cwd = ref("");
const entries = ref<SftpEntry[]>([]);
const loading = ref(false);
const errorText = ref("");
const selectedPath = ref("");
const transfers = ref<XferTask[]>([]);
const unlisteners: (() => void)[] = [];

const sshServers = computed(() => store.servers.filter((s) => s.protocol === "ssh"));
const connected = computed(() => !errorText.value);

/** 面包屑（旧版 .fb-crumbs） */
const crumbs = computed(() => {
  const parts = (cwd.value || "/").split("/").filter(Boolean);
  const out: { name: string; path: string }[] = [{ name: "/", path: "/" }];
  let acc = "";
  for (const part of parts) {
    acc += `/${part}`;
    out.push({ name: part, path: acc });
  }
  return out;
});

/** 当前传输（旧版 .fb-transfer 只显示一条） */
const activeTransfer = computed(() => transfers.value.find((t) => t.state !== "done") ?? null);

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
        index >= 0 ? transfers.value.map((t, i) => (i === index ? task : t)) : [...transfers.value, task];
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
    const result = await call<{ cwd: string; entries: SftpEntry[] }>("hub_sftp_list", {
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

async function refreshTransfers() {
  try {
    transfers.value = (await call<XferTask[]>("hub_sftp_transfer_list")) ?? [];
  } catch {
    transfers.value = [];
  }
}

async function goUp() {
  const parent = cwd.value.replace(/\/[^/]+\/?$/, "") || "/";
  await listDir(parent);
}

function openEntry(entry: SftpEntry) {
  if (entry.is_dir) void listDir(entry.path);
  else selectedPath.value = entry.path;
}

function selectEntry(entry: SftpEntry) {
  selectedPath.value = entry.path;
}

async function upload() {
  try {
    await call("hub_sftp_upload", { id: serverId.value, remoteDir: cwd.value });
    await refreshTransfers();
  } catch (error) {
    errorText.value = String(error);
  }
}

async function download() {
  if (!selectedPath.value) return;
  try {
    await call("hub_sftp_download", { id: serverId.value, remotePath: selectedPath.value });
    await refreshTransfers();
  } catch (error) {
    errorText.value = String(error);
  }
}

async function mkdir() {
  const name = window.prompt("新建目录名称", "");
  if (!name?.trim()) return;
  const base = cwd.value.endsWith("/") ? cwd.value : `${cwd.value}/`;
  try {
    await call("hub_sftp_mkdir", { id: serverId.value, path: `${base}${name.trim()}` });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

async function removeEntry(entry: SftpEntry) {
  if (!window.confirm(`确定删除「${entry.name}」？`)) return;
  try {
    await call("hub_sftp_remove", { id: serverId.value, path: entry.path });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

async function transferCtl(action: "pause" | "resume" | "abandon") {
  const task = activeTransfer.value;
  if (!task) return;
  try {
    await call("hub_sftp_transfer_ctl", { key: task.key, action });
    await refreshTransfers();
  } catch (error) {
    errorText.value = String(error);
  }
}

function openInWindow() {
  if (!serverId.value) return;
  void call("hub_open_window", { label: "server-files", serverId: serverId.value }).catch(() => {});
}

function fmtSize(bytes: number): string {
  if (!bytes) return "—";
  const units = ["B", "K", "M", "G"];
  let value = bytes;
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i += 1;
  }
  return `${value.toFixed(value < 10 && i > 0 ? 1 : 0)}${units[i]}`;
}

function fmtTime(ms: number): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString("zh-CN", { hour12: false }).replace(/\//g, "-");
}

function pct(task: XferTask): number {
  if (!task.total) return 0;
  return Math.min(100, Math.round((task.bytes / task.total) * 100));
}
</script>

<template>
  <div class="fb-topbar">
    <select v-model="serverId" class="fb-select" aria-label="选择服务器" @change="listDir('')">
      <option v-for="server in sshServers" :key="server.id" :value="server.id">
        {{ server.name }} · {{ store.displayAddr(server) }}
      </option>
    </select>
    <span :class="['fb-pill', connected ? 'fb-ok' : 'fb-bad']">
      <span class="fb-dot" />
      <span>{{ errorText ? "连接失败" : loading ? "读取中…" : "已连接" }}</span>
    </span>
    <button type="button" class="fb-toolbtn" title="在独立窗口打开当前服务器的文件浏览" @click="openInWindow">
      ⧉ 独立窗口
    </button>
  </div>

  <div v-if="!sshServers.length" class="fb-empty">
    还没有 SSH 服务器 —— 到「服务器」页添加一台，这里就能浏览它的文件。
  </div>

  <div v-else class="fb-main">
    <div class="fb-pathbar">
      <button type="button" class="fb-iconbtn" aria-label="上一级" title="上一级" @click="goUp">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 12H5M11 18l-6-6 6-6" />
        </svg>
      </button>
      <button type="button" class="fb-iconbtn" aria-label="刷新" title="刷新" @click="listDir(cwd)">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6" />
        </svg>
      </button>
      <nav class="fb-crumbs scroll-thin" aria-label="路径">
        <!-- 分隔符是独立 span 放在前面（旧版 fbCrumbs() 的顺序），不是嵌在按钮里 -->
        <template v-for="(crumb, i) in crumbs" :key="crumb.path">
          <span v-if="i > 0" class="fb-crumb-sep" aria-hidden="true">›</span>
          <button
            type="button"
            class="fb-crumb"
            :aria-current="i === crumbs.length - 1"
            @click="listDir(crumb.path)"
          >
            {{ crumb.name }}
          </button>
        </template>
      </nav>
      <button type="button" class="fb-btn" @click="mkdir">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 5v14M5 12h14" />
        </svg>
        新建目录
      </button>
      <button type="button" class="fb-btn fb-primary" @click="upload">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 19V5M5 12l7-7 7 7" />
        </svg>
        上传
      </button>
      <button
        type="button"
        class="fb-btn"
        :disabled="!selectedPath"
        :title="selectedPath ? '下载选中的文件' : '未选中文件'"
        @click="download"
      >
        下载
      </button>
    </div>

    <div class="fb-tablewrap scroll-thin">
      <table class="fb-table" aria-label="文件列表">
        <thead>
          <tr>
            <th>名称</th>
            <th style="width: 90px">大小</th>
            <th style="width: 120px">权限</th>
            <th style="width: 130px">修改时间</th>
            <th style="width: 64px; text-align: right">操作</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="entry in entries"
            :key="entry.path"
            :class="{ 'is-selected': selectedPath === entry.path }"
            @click="selectEntry(entry)"
            @dblclick="openEntry(entry)"
          >
            <td>
              <span class="fb-ico">{{ entry.is_dir ? "📁" : "📄" }}</span>
              {{ entry.name }}
            </td>
            <td class="mono">{{ entry.is_dir ? "—" : fmtSize(entry.size) }}</td>
            <td class="mono">{{ entry.perms || "—" }}</td>
            <td class="mono">{{ fmtTime(entry.mtime) }}</td>
            <td style="text-align: right">
              <button type="button" class="link-btn" @click.stop="removeEntry(entry)">删除</button>
            </td>
          </tr>
        </tbody>
      </table>
      <div v-if="!loading && !entries.length" class="fb-emptytip">这个目录是空的</div>
    </div>

    <div v-if="activeTransfer" class="fb-transfer">
      <svg class="fb-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" width="14" height="14">
        <path d="M21 12a9 9 0 1 1-9-9" />
      </svg>
      <span class="fb-tbadge fb-down">{{ activeTransfer.dir === "up" ? "↑ 上传" : "↓ 下载" }}</span>
      <span>{{ activeTransfer.name }}</span>
      <div class="fb-bar"><div :style="{ width: pct(activeTransfer) + '%' }" /></div>
      <span class="fb-tmeta">{{ fmtSize(activeTransfer.bytes) }} / {{ fmtSize(activeTransfer.total) }}</span>
      <span class="fb-pct">{{ pct(activeTransfer) }}%</span>
      <button
        type="button"
        class="fb-tbtn"
        @click="transferCtl(activeTransfer.state === 'running' ? 'pause' : 'resume')"
      >
        {{ activeTransfer.state === "running" ? "暂停" : "继续" }}
      </button>
      <button type="button" class="fb-tbtn danger" @click="transferCtl('abandon')">放弃</button>
    </div>

    <div class="fb-foot">
      <span>{{ entries.length }} 项</span><span aria-hidden="true">·</span
      ><span>SFTP 复用 SSH 会话</span>
    </div>
  </div>
</template>
