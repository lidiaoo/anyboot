<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { call } from "@/api/tauri";
import FbModal from "@/components/hub/FbModal.vue";

/**
 * 文件窗（原 `ui/hub-files.html`，1262 行）—— hub_sftp 命令族 16 个。
 *
 * ## 1:1 结构（独立窗自己的一整套，与主窗内嵌的 .fb-* 不同）
 * ```
 * header > span.appicon + h1#title + span.pill(.dot + #connText)
 *        + .tools（远程管理 / 主题）
 * .pathbar > .iconbtn#upBtn + #refreshBtn + nav.crumbs#crumbs + #pathDropBtn
 *          + button.btn#mkdirBtn + button.btn.primary#uploadBtn + #downloadBtn
 * .table-wrap.scroll-thin > table（thead 5 列 + tbody#tbody）+ .empty-tip
 * .transfer（spin + tbadge + name + bar + meta + pct + 暂停/放弃）
 * footer > #footCount
 * .modal#pick（服务器选择）+ 5 个 .modal（新建/重命名/删除/属性/下载到）
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

const serverId = ref("");
const cwd = ref("");
const entries = ref<SftpEntry[]>([]);
const loading = ref(false);
const errorText = ref("");
const selectedPath = ref("");
const transfers = ref<XferTask[]>([]);
const servers = ref<{ id: string; name: string; host: string; protocol: string }[]>([]);
const unlisteners: (() => void)[] = [];

/** 弹层 */
const pickOpen = ref(false);
const mkdirOpen = ref(false);
const mkdirName = ref("");
const renameOpen = ref(false);
const renameName = ref("");
const delOpen = ref(false);
const infoOpen = ref(false);
const targetEntry = ref<SftpEntry | null>(null);

const dark = ref(false);
try {
  dark.value = localStorage.getItem("anyboot.theme") === "dark";
} catch {
  /* 吃默认 */
}
function applyTheme(isDark: boolean) {
  const html = document.documentElement;
  html.setAttribute("data-theme", isDark ? "dark" : "light");
  html.classList.toggle("light", !isDark);
  html.classList.toggle("dark", isDark);
}
applyTheme(dark.value);

function toggleTheme() {
  dark.value = !dark.value;
  applyTheme(dark.value);
  try {
    localStorage.setItem("anyboot.theme", dark.value ? "dark" : "light");
  } catch {
    /* 忽略 */
  }
}

function openHub() {
  void call("hub_open_window", {}).catch(() => {});
}

const sshServers = computed(() => servers.value.filter((s) => s.protocol === "ssh"));
const currentServer = computed(() => sshServers.value.find((s) => s.id === serverId.value));

/** 面包屑 */
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

const activeTransfer = computed(() => transfers.value.find((t) => t.state !== "done") ?? null);

onMounted(async () => {
  try {
    servers.value = (await call<{ id: string; name: string; host: string; protocol: string }[]>(
      "hub_list_servers",
    )) ?? [];
  } catch {
    servers.value = [];
  }
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

function startMkdir() {
  mkdirName.value = "";
  mkdirOpen.value = true;
}

async function confirmMkdir() {
  const name = mkdirName.value.trim();
  mkdirOpen.value = false;
  if (!name) return;
  const base = cwd.value.endsWith("/") ? cwd.value : `${cwd.value}/`;
  try {
    await call("hub_sftp_mkdir", { id: serverId.value, path: `${base}${name}` });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

function startRename(entry: SftpEntry) {
  targetEntry.value = entry;
  renameName.value = entry.name;
  renameOpen.value = true;
}

async function confirmRename() {
  const entry = targetEntry.value;
  const next = renameName.value.trim();
  renameOpen.value = false;
  if (!entry || !next || next === entry.name) return;
  try {
    await call("hub_sftp_rename", {
      id: serverId.value,
      from: entry.path,
      to: entry.path.replace(/[^/]+$/, next),
    });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

function startDelete(entry: SftpEntry) {
  targetEntry.value = entry;
  delOpen.value = true;
}

async function confirmDelete() {
  const entry = targetEntry.value;
  delOpen.value = false;
  if (!entry) return;
  try {
    await call("hub_sftp_remove", { id: serverId.value, path: entry.path });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

function showInfo(entry: SftpEntry) {
  targetEntry.value = entry;
  infoOpen.value = true;
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

function pickServer(id: string) {
  pickOpen.value = false;
  serverId.value = id;
  void listDir("");
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
  <header>
    <span class="appicon" aria-hidden="true">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
      </svg>
    </span>
    <h1>{{ currentServer?.name || "文件" }}</h1>
    <span v-if="currentServer" class="pill" :class="errorText ? 'bad' : 'ok'">
      <span class="dot" />
      <span>{{ errorText ? "连接失败" : loading ? "读取中…" : "已连接" }}</span>
    </span>
    <div class="tools">
      <button type="button" class="toolbtn" aria-label="打开远程管理" title="打开远程管理" @click="openHub">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect x="2" y="3" width="20" height="7" rx="2" />
          <rect x="2" y="14" width="20" height="7" rx="2" />
          <path d="M6 6.5h.01M6 17.5h.01" />
        </svg>
        远程管理
      </button>
      <button type="button" class="toolbtn" aria-label="切换主题" @click="toggleTheme">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 3a9 9 0 0 1 0 18z" fill="currentColor" stroke="none" />
        </svg>
        主题
      </button>
      <button type="button" class="toolbtn" aria-label="切换服务器" @click="pickOpen = true">切换服务器</button>
    </div>
  </header>

  <div class="pathbar">
    <button type="button" class="iconbtn" aria-label="上一级" title="上一级" @click="goUp">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M19 12H5M11 18l-6-6 6-6" />
      </svg>
    </button>
    <button type="button" class="iconbtn" aria-label="刷新" title="刷新" @click="listDir(cwd)">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6" />
      </svg>
    </button>
    <nav class="crumbs scroll-thin" aria-label="路径">
      <template v-for="(crumb, i) in crumbs" :key="crumb.path">
        <span v-if="i > 0" class="crumb-sep" aria-hidden="true">›</span>
        <button type="button" class="crumb" :aria-current="i === crumbs.length - 1" @click="listDir(crumb.path)">
          {{ crumb.name }}
        </button>
      </template>
    </nav>
    <button type="button" class="btn" @click="startMkdir">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 5v14M5 12h14" />
      </svg>
      新建目录
    </button>
    <button type="button" class="btn primary" @click="upload">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 19V5M5 12l7-7 7 7" />
      </svg>
      上传
    </button>
    <button type="button" class="btn" :disabled="!selectedPath" :title="selectedPath ? '下载选中的文件' : '未选中文件'" @click="download">
      下载
    </button>
  </div>

  <div class="table-wrap scroll-thin">
    <table aria-label="文件列表">
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
          <td><span class="ico">{{ entry.is_dir ? "📁" : "📄" }}</span>{{ entry.name }}</td>
          <td class="mono">{{ entry.is_dir ? "—" : fmtSize(entry.size) }}</td>
          <td class="mono">{{ entry.perms || "—" }}</td>
          <td class="mono">{{ fmtTime(entry.mtime) }}</td>
          <td style="text-align: right">
            <button type="button" class="link-btn" @click.stop="startRename(entry)">改名</button>
            <button type="button" class="link-btn" @click.stop="showInfo(entry)">属性</button>
            <button type="button" class="link-btn" @click.stop="startDelete(entry)">删除</button>
          </td>
        </tr>
      </tbody>
    </table>
    <div v-if="!loading && !entries.length" class="empty-tip">
      {{ errorText || "这个目录是空的" }}
    </div>
  </div>

  <div v-if="activeTransfer" class="transfer">
    <svg class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" width="14" height="14">
      <path d="M21 12a9 9 0 1 1-9-9" />
    </svg>
    <span class="tbadge down">{{ activeTransfer.dir === "up" ? "↑ 上传" : "↓ 下载" }}</span>
    <span>{{ activeTransfer.name }}</span>
    <div class="bar"><div :style="{ width: pct(activeTransfer) + '%' }" /></div>
    <span class="tmeta">{{ fmtSize(activeTransfer.bytes) }} / {{ fmtSize(activeTransfer.total) }}</span>
    <span class="pct">{{ pct(activeTransfer) }}%</span>
    <button type="button" class="tbtn" @click="transferCtl(activeTransfer.state === 'running' ? 'pause' : 'resume')">
      {{ activeTransfer.state === "running" ? "暂停" : "继续" }}
    </button>
    <button type="button" class="tbtn danger" @click="transferCtl('abandon')">放弃</button>
  </div>

  <footer>
    <span>{{ entries.length }} 项 · SFTP 复用 SSH 会话</span>
  </footer>

  <!-- 5 个弹层 -->
  <FbModal :open="mkdirOpen" title="新建目录" confirm-text="创建" @confirm="confirmMkdir" @cancel="mkdirOpen = false">
    <label for="winMkdirName">目录名（当前目录下创建）</label>
    <input id="winMkdirName" v-model="mkdirName" placeholder="例如 backup" autocomplete="off" />
  </FbModal>

  <FbModal :open="renameOpen" title="重命名" @confirm="confirmRename" @cancel="renameOpen = false">
    <label for="winRenameName">新名称</label>
    <input id="winRenameName" v-model="renameName" autocomplete="off" />
  </FbModal>

  <FbModal :open="delOpen" title="删除" confirm-text="删除" :danger="true" @confirm="confirmDelete" @cancel="delOpen = false">
    <p class="hint">确定删除「{{ targetEntry?.name }}」？此操作不可恢复。</p>
  </FbModal>

  <FbModal :open="infoOpen" title="属性" confirm-text="关闭" :cancel-text="' '" @confirm="infoOpen = false" @cancel="infoOpen = false">
    <p><b>名称</b> {{ targetEntry?.name }}</p>
    <p><b>路径</b> <span class="mono">{{ targetEntry?.path }}</span></p>
    <p><b>大小</b> {{ targetEntry?.is_dir ? "—" : fmtSize(targetEntry?.size ?? 0) }}</p>
  </FbModal>

  <div v-if="pickOpen" class="modal" @click.self="pickOpen = false">
    <div class="modal-card" role="dialog" aria-modal="true">
      <div class="modal-head">
        <h3>选择服务器</h3>
        <button type="button" aria-label="关闭" @click="pickOpen = false">✕</button>
      </div>
      <div class="modal-body">
        <button
          v-for="s in sshServers"
          :key="s.id"
          type="button"
          class="btn"
          style="width: 100%; justify-content: flex-start; margin-bottom: 6px"
          @click="pickServer(s.id)"
        >
          {{ s.name }} · {{ s.host }}
        </button>
        <p v-if="!sshServers.length" class="hint">还没有 SSH 服务器</p>
      </div>
    </div>
  </div>
</template>
