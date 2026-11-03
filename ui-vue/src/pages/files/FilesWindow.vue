<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { call } from "@/api/tauri";

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
  link?: boolean;
  target?: string;
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

/** 行操作菜单（旧版 #rowMenu） */
const rowMenuOpen = ref(false);
const menuEntry = ref<SftpEntry | null>(null);
const rowMenuRef = ref<HTMLElement | null>(null);

/** 上级目录快跳（旧版 #pathMenu） */
const pathMenuOpen = ref(false);
const parentPaths = computed(() => {
  const parts = (cwd.value || "/").split("/").filter(Boolean);
  const out: string[] = ["/"];
  let acc = "";
  for (const p of parts) {
    acc += `/${p}`;
    out.push(acc);
  }
  return out;
});

/** 目标目录弹层（旧版 #destModal：复制到 / 移动到） */
const destOpen = ref(false);
const destMode = ref<"copyTo" | "moveTo">("copyTo");
const destPath = ref("");

/** toast（旧版 #toast） */
const toastText = ref("");
function showToast(text: string) {
  toastText.value = text;
  window.setTimeout(() => {
    if (toastText.value === text) toastText.value = "";
  }, 2600);
}

/** 状态栏时钟（旧版 #footClock） */
const clockText = ref("");
let clockTimer = 0;
function tickClock() {
  clockText.value = new Date().toLocaleTimeString("zh-CN", { hour12: false });
}
onMounted(() => {
  tickClock();
  clockTimer = window.setInterval(tickClock, 1000);
});
onUnmounted(() => {
  if (clockTimer) window.clearInterval(clockTimer);
});

function openRowMenu(entry: SftpEntry, _event: MouseEvent) {
  menuEntry.value = entry;
  selectedPath.value = entry.path;
  rowMenuOpen.value = true;
}


function menuAction(act: string) {
  const entry = menuEntry.value;
  rowMenuOpen.value = false;
  if (!entry) return;
  switch (act) {
    case "download":
      selectedPath.value = entry.path;
      void download();
      break;
    case "rename":
      startRename(entry);
      break;
    case "copyPath":
      void navigator.clipboard.writeText(entry.path).then(() => showToast("路径已复制"));
      break;
    case "delete":
      startDelete(entry);
      break;
    case "info":
      showInfo(entry);
      break;
    case "copy":
    case "cut":
      showToast(act === "copy" ? "已复制（暂存）" : "已剪切（暂存）");
      break;
    case "copyTo":
      destMode.value = "copyTo";
      destPath.value = cwd.value;
      destOpen.value = true;
      break;
    case "moveTo":
      destMode.value = "moveTo";
      destPath.value = cwd.value;
      destOpen.value = true;
      break;
  }
}

async function confirmDest() {
  const entry = menuEntry.value;
  destOpen.value = false;
  if (!entry || !destPath.value.trim()) return;
  const target = `${destPath.value.replace(/\/$/, "")}/${entry.name}`;
  try {
    if (destMode.value === "copyTo") {
      await call("hub_sftp_copy", { server: currentServer.value, from: entry.path, to: target });
      showToast("已复制");
    } else {
      await call("hub_sftp_rename", { server: currentServer.value, from: entry.path, to: target });
      showToast("已移动");
    }
    await listDir(cwd.value);
  } catch (error) {
    showToast(String(error));
  }
}

const dark = ref(false);
try {
  dark.value = localStorage.getItem("anyboot.theme-resolved") === "dark";
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


// 页面心跳（旧版）：后端据此区分「窗口对象还在」和「页面还活着」——
// 停跳 15s = 僵尸窗口 → 销毁重建。不发心跳活窗口会被反复重建！
void call("hub_page_ping", { ready: true }).catch(() => {});
const pagePingTimer = window.setInterval(() => {
  void call("hub_page_ping", { ready: true }).catch(() => {});
}, 3000);
onUnmounted(() => window.clearInterval(pagePingTimer));

onMounted(async () => {
  try {
    servers.value = (await call<{ id: string; name: string; host: string; protocol: string }[]>(
      "hub_list_servers",
    )) ?? [];
  } catch {
    servers.value = [];
  }
  // 旧版 boot：hub_sftp_take_pending 取排队服务器；无 → 空态「没有指定服务器」
  try {
    const pending = await call<{ id?: string; name?: string } | null>("hub_sftp_take_pending");
    if (pending?.id) {
      serverId.value = pending.id;
      await listDir("");
    }
  } catch { /* 无排队服务器 */ }
  await refreshTransfers();

  const { listen } = await import("@tauri-apps/api/event");
  // 复用已开窗口：hub 请求切到某服务器并回到家目录（旧版 hub-files-open-server）
  unlisteners.push(
    await listen<{ id?: string }>("hub-files-open-server", async (event) => {
      const id = event.payload?.id;
      if (!id) return;
      serverId.value = id;
      await listDir("");
    }),
  );
  // 主题跟随
  unlisteners.push(
    await listen<{ theme?: string }>("theme-changed", (event) => {
      const theme = event.payload?.theme;
      if (theme) applyTheme(theme !== "light");
    }),
  );
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
      server: sshServers.value.find((s) => s.id === serverId.value),
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
    const uploaded = (await call("hub_sftp_upload", {
      server: sshServers.value.find((s) => s.id === serverId.value),
      remoteDir: cwd.value,
    })) as Array<unknown> | null;
    if (uploaded?.length) showToast("已上传 " + uploaded.length + " 个文件");
    await refreshTransfers();
    await listDir(cwd.value); // 上传的新文件要出现在列表里（旧版 finally refresh）
  } catch (error) {
    const msg = String(error);
    if (msg !== "未选择文件") errorText.value = msg; // 取消选择不算错误
  }
}

async function download() {
  const entry = entries.value.find((e) => e.path === selectedPath.value);
  if (!selectedPath.value || !entry) return;
  try {
    // 下载路径取终端设置的「下载路径」（旧版：空/~ = 未设置 → 系统下载目录）
    let downDir = "";
    try {
      const s = JSON.parse(localStorage.getItem("ab-term-settings") || "{}") as { downDir?: unknown };
      const raw = typeof s.downDir === "string" ? s.downDir.trim() : "";
      if (raw && raw !== "~" && raw !== "~/") downDir = raw;
    } catch { /* 吃默认 */ }
    if (!downDir) {
      try { downDir = (await call<string>("hub_download_dir_default")) || ""; } catch { /* 忽略 */ }
    }
    const local = await call<string>("hub_sftp_download", {
      server: sshServers.value.find((s) => s.id === serverId.value),
      remotePath: selectedPath.value,
      filename: entry.name,
      localDir: downDir || null,
      total: entry.size || 0,
    });
    showToast("已下载到 " + (local || downDir || "本地"));
    await refreshTransfers();
  } catch (error) {
    showToast("下载失败：" + error);
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
    await call("hub_sftp_mkdir", { server: sshServers.value.find((s) => s.id === serverId.value), path: `${base}${name}` });
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
      server: sshServers.value.find((s) => s.id === serverId.value),
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
    await call("hub_sftp_remove", { server: sshServers.value.find((s) => s.id === serverId.value), path: entry.path, isDir: entry.is_dir });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

function showInfo(entry: SftpEntry) {
  targetEntry.value = entry;
  infoOpen.value = true;
  if (entry.link && !entry.target) {
    void call<string>("hub_sftp_readlink", { server: currentServer.value, path: entry.path })
      .then((tgt) => { if (targetEntry.value?.path === entry.path) targetEntry.value = { ...entry, target: String(tgt ?? "") }; })
      .catch(() => {});
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
    <h1 id="title">{{ currentServer?.name || "文件" }}</h1>
    <span id="connPill" class="pill" :class="errorText ? 'bad' : 'ok'" :hidden="!currentServer">
      <span class="dot" />
      <span id="connText">{{ errorText ? "连接失败" : loading ? "读取中…" : "已连接" }}</span>
    </span>
    <div class="tools">
      <button type="button" class="toolbtn" id="hubBtn" aria-label="打开远程管理" title="打开远程管理" @click="openHub">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect x="2" y="3" width="20" height="7" rx="2" />
          <rect x="2" y="14" width="20" height="7" rx="2" />
          <path d="M6 6.5h.01M6 17.5h.01" />
        </svg>
        远程管理
      </button>
      <button type="button" class="toolbtn" id="themeBtn" aria-label="切换主题" @click="toggleTheme">
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
    <button type="button" class="iconbtn" id="upBtn" aria-label="上一级" title="上一级" @click="goUp">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M19 12H5M11 18l-6-6 6-6" />
      </svg>
    </button>
    <button type="button" class="iconbtn" id="refreshBtn" aria-label="刷新" title="刷新" @click="listDir(cwd)">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6" />
      </svg>
    </button>
    <nav class="crumbs scroll-thin" id="crumbs" aria-label="路径">
      <template v-for="(crumb, i) in crumbs" :key="crumb.path">
        <span v-if="i > 0" class="crumb-sep" aria-hidden="true">›</span>
        <button type="button" class="crumb" :aria-current="i === crumbs.length - 1" @click="listDir(crumb.path)">
          {{ crumb.name }}
        </button>
      </template>
    </nav>
    <button type="button" class="iconbtn" id="pathDropBtn" aria-label="路径快跳" title="上级目录快跳" @click.stop="pathMenuOpen = !pathMenuOpen">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M6 9l6 6 6-6" />
      </svg>
    </button>
    <button type="button" class="btn" id="mkdirBtn" @click="startMkdir">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 5v14M5 12h14" />
      </svg>
      新建目录
    </button>
    <button type="button" class="btn primary" id="uploadBtn" @click="upload">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 19V5M5 12l7-7 7 7" />
      </svg>
      上传
    </button>
    <button type="button" class="btn" id="downloadBtn" :disabled="!selectedPath" :title="selectedPath ? '下载选中的文件' : '未选中文件'" @click="download">
      下载
    </button>
  </div>

  <!-- 上级目录快跳菜单（旧版 #pathMenu） -->
  <div v-if="pathMenuOpen" class="popmenu" id="pathMenu" role="menu" aria-label="上级目录">
    <button
      v-for="p in parentPaths"
      :key="p"
      type="button"
      @click="listDir(p); pathMenuOpen = false"
    >
      {{ p }}
    </button>
  </div>

  <div class="table-wrap scroll-thin" id="tableWrap">
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
      <tbody id="tbody">
        <tr
          v-for="entry in entries"
          :key="entry.path"
          :class="{ 'is-selected': selectedPath === entry.path }"
          @click="selectEntry(entry)"
          @dblclick="openEntry(entry)"
          @contextmenu.prevent="openRowMenu(entry, $event)"
        >
          <td><span class="ico">{{ entry.is_dir ? "📁" : "📄" }}</span>{{ entry.name }}</td>
          <td class="mono">{{ entry.is_dir ? "—" : fmtSize(entry.size) }}</td>
          <td class="mono">{{ entry.perms || "—" }}</td>
          <td class="mono">{{ fmtTime(entry.mtime) }}</td>
          <td style="text-align: right">
            <button type="button" class="rowdl" :title="'操作'" :aria-label="entry.name + ' 操作'" @click.stop="openRowMenu(entry, $event)">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><circle cx="5" cy="12" r="1.6" /><circle cx="12" cy="12" r="1.6" /><circle cx="19" cy="12" r="1.6" /></svg>
            </button>
          </td>
        </tr>
      </tbody>
    </table>
    <div v-if="!serverId" id="emptyTip" class="empty-tip">没有指定服务器</div>
    <div v-else-if="!loading && !entries.length" id="emptyTip" class="empty-tip">
      {{ errorText || "这个目录是空的" }}
    </div>
  </div>

  <!-- 传输进度（旧版 #transfer） -->
  <div v-if="activeTransfer" id="transfer" class="transfer">
    <svg class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" width="14" height="14">
      <path d="M21 12a9 9 0 1 1-9-9" />
    </svg>
    <span id="transferDir" class="tbadge" :class="activeTransfer.dir === 'up' ? 'up' : 'down'">
      {{ activeTransfer.dir === "up" ? "↑ 上传" : "↓ 下载" }}
    </span>
    <span id="transferName">{{ activeTransfer.name }}</span>
    <div id="transferBar" class="bar"><div id="transferFill" :style="{ width: pct(activeTransfer) + '%' }" /></div>
    <span id="transferMeta" class="tmeta">{{ fmtSize(activeTransfer.bytes) }} / {{ fmtSize(activeTransfer.total) }}</span>
    <span id="transferPct" class="pct">{{ pct(activeTransfer) }}%</span>
    <button type="button" id="transferAct" class="tbtn" @click="transferCtl(activeTransfer.state === 'running' ? 'pause' : 'resume')">
      {{ activeTransfer.state === "running" ? "暂停" : "继续" }}
    </button>
    <button type="button" id="transferGiveup" class="tbtn danger" @click="transferCtl('abandon')">放弃</button>
  </div>

  <footer>
    <span id="footCount">{{ entries.length }} 项</span>
    <span aria-hidden="true">·</span><span>SFTP 复用 SSH 会话</span>
    <span id="footClock" class="clock">{{ clockText }}</span>
  </footer>

  <!-- 行操作菜单（旧版 #rowMenu） -->
  <div v-if="rowMenuOpen" id="rowMenu" ref="rowMenuRef" class="filemenu" role="menu" aria-label="文件操作">
    <div class="fm-preview">
      <span id="fmIcon" class="fm-icon" aria-hidden="true">
        <svg id="fmIcoFile" v-if="!menuEntry?.is_dir" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
          <path d="M14 2v6h6" />
        </svg>
        <svg id="fmIcoDir" v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
        </svg>
        <svg id="fmIcoLink" v-if="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M13 2L3 14h7l-1 8 10-12h-7z" />
        </svg>
      </span>
      <span class="fm-names">
        <span class="fm-name" id="fmName">{{ menuEntry?.name || "—" }}</span>
        <span class="fm-meta mono" id="fmMeta">{{ menuEntry ? (menuEntry.is_dir ? "目录" : fmtSize(menuEntry.size)) : "—" }}</span>
      </span>
      <button type="button" id="fmClose" class="fm-close" aria-label="关闭" @click="rowMenuOpen = false">✕</button>
    </div>
    <button role="menuitem" id="rmDownload" class="fm-act accent" @click="menuAction('download')">下载</button>
    <button role="menuitem" id="rmRename" class="fm-act" @click="menuAction('rename')">重命名</button>
    <button role="menuitem" id="rmCopyPath" class="fm-act" @click="menuAction('copyPath')">复制路径</button>
    <button role="menuitem" id="rmDelete" class="fm-act danger" @click="menuAction('delete')">删除</button>
    <p class="fm-label">更多操作</p>
    <div class="fm-group">
    <button role="menuitem" id="rmInfo" class="fm-act" @click="menuAction('info')">详情</button>
    <button role="menuitem" id="rmCopy" class="fm-act" @click="menuAction('copy')">复制</button>
    <button role="menuitem" id="rmCut" class="fm-act" @click="menuAction('cut')">剪切</button>
    <button role="menuitem" id="rmCopyTo" class="fm-act" @click="menuAction('copyTo')">复制到...</button>
    <button role="menuitem" id="rmMoveTo" class="fm-act" @click="menuAction('moveTo')">移动到...</button>
    </div>
  </div>

  <!-- 新建目录弹层（旧版 #mkdirModal） -->
  <div v-if="mkdirOpen" id="mkdirModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="mkdirTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="mkdirTitle">新建目录</h3>
        <button type="button" id="mkdirClose" aria-label="关闭" @click="mkdirOpen = false">✕</button>
      </div>
      <form id="mkdirForm" class="modal-body" @submit.prevent="confirmMkdir">
        <input id="mkdirName" v-model="mkdirName" placeholder="例如 backup" autocomplete="off" required />
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="mkdirCancel" @click="mkdirOpen = false">取消</button>
          <button type="submit" class="btn primary" id="mkdirOk">创建</button>
        </div>
      </form>
    </div>
  </div>

  <!-- 重命名弹层（旧版 #renameModal） -->
  <div v-if="renameOpen" id="renameModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="renameTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="renameTitle">重命名</h3>
        <button type="button" id="renameClose" aria-label="关闭" @click="renameOpen = false">✕</button>
      </div>
      <form id="renameForm" class="modal-body" @submit.prevent="confirmRename">
        <input id="renameName" v-model="renameName" autocomplete="off" required />
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="renameCancel" @click="renameOpen = false">取消</button>
          <button type="submit" class="btn primary" id="renameOk">重命名</button>
        </div>
      </form>
    </div>
  </div>

  <!-- 删除弹层（旧版 #delModal） -->
  <div v-if="delOpen" id="delModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="delTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="delTitle">删除</h3>
        <button type="button" id="delClose" aria-label="关闭" @click="delOpen = false">✕</button>
      </div>
      <div class="modal-body">
        <p id="delText">确定删除「{{ targetEntry?.name }}」？此操作不可恢复。</p>
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="delCancel" @click="delOpen = false">取消</button>
          <button type="button" class="btn dangerbtn" id="delOk" @click="confirmDelete">删除</button>
        </div>
      </div>
    </div>
  </div>

  <!-- 详情弹层（旧版 #infoModal） -->
  <div v-if="infoOpen" id="infoModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="infoTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="infoTitle">详情</h3>
        <button type="button" id="infoClose" aria-label="关闭" @click="infoOpen = false">✕</button>
      </div>
      <div id="infoRows" class="modal-body">
        <p><b>名称</b> {{ targetEntry?.name }}</p>
        <p><b>大小</b> {{ targetEntry?.is_dir ? "—" : fmtSize(targetEntry?.size ?? 0) }}</p>
        <p><b>权限</b> <span class="mono">{{ targetEntry?.perms || "—" }}</span></p>
        <p><b>修改时间</b> {{ fmtTime(targetEntry?.mtime ?? 0) }}</p>
        <p><b>路径</b> <span class="mono">{{ targetEntry?.path }}</span></p>
        <p v-if="targetEntry?.link"><b>指向</b> <span class="mono">{{ targetEntry?.target || "读取中…" }}</span></p>
      </div>
      <div class="modal-foot">
        <button type="button" class="btn primary" id="infoOk" @click="infoOpen = false">关闭</button>
      </div>
    </div>
  </div>

  <!-- 目标目录弹层（旧版 #destModal：复制到/移动到） -->
  <div v-if="destOpen" id="destModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="destTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="destTitle">{{ destMode === "moveTo" ? "移动到" : "复制到" }}</h3>
        <button type="button" id="destClose" aria-label="关闭" @click="destOpen = false">✕</button>
      </div>
      <form id="destForm" class="modal-body" @submit.prevent="confirmDest">
        <label id="destLabel" for="destPath">目标路径（含文件名）</label>
        <input id="destPath" v-model="destPath" class="mono" placeholder="/path/to/dir" autocomplete="off" required />
        <p class="hint">目录不存在会原样报错；目标已存在会被覆盖。</p>
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="destCancel" @click="destOpen = false">取消</button>
          <button type="submit" class="btn primary" id="destOk">确定</button>
        </div>
      </form>
    </div>
  </div>

  <!-- toast（旧版 #toast） -->
  <div v-if="toastText" id="toast">{{ toastText }}</div>

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
