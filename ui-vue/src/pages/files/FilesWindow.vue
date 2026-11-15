<script setup lang="ts">
import WindowNavMenu from "@/components/hub/WindowNavMenu.vue";
import { t } from "@/i18n";
import { computed, onMounted, onUnmounted, ref } from "vue";
import { call, isTauri } from "@/api/tauri";

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
  mtime: string;
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
      void navigator.clipboard.writeText(entry.path).then(() => showToast(t("fb.pathCopied")));
      break;
    case "delete":
      startDelete(entry);
      break;
    case "info":
      showInfo(entry);
      break;
    case "copy":
    case "cut":
      showToast(act === "copy" ? t("fb.stagedCopy") : t("fb.stagedCut"));
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
      showToast(t("fb.copied"));
    } else {
      await call("hub_sftp_rename", { server: currentServer.value, from: entry.path, to: target });
      showToast(t("fb.moved"));
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
  const browser = !isTauri();
  if (browser) {
    // 网页端：服务器从 URL 参数带进来（?serverId=…），没有 take_pending/listen 那套。
    try {
      const q = new URLSearchParams(location.search);
      const sid = q.get("serverId") || q.get("server") || "";
      if (sid) { serverId.value = sid; await openHome(); }
    } catch { /* ignore */ }
    void refreshTransfers();
    return;
  }
  // 旧版 boot：hub_sftp_take_pending 取排队服务器；无 → 空态「没有指定服务器」
  try {
    const pending = await call<{ id?: string; name?: string } | null>("hub_sftp_take_pending");
    if (pending?.id) {
      serverId.value = pending.id;
      await openHome();
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
      await openHome();
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

/** 旧版 boot：hub_sftp_home 取远端家目录再列（空 path 直接 ls 会失败/列错） */
async function openHome(): Promise<void> {
  if (!serverId.value) return;
  try {
    const home = await call<string>("hub_sftp_home", {
      server: sshServers.value.find((s) => s.id === serverId.value),
    });
    await listDir(home || "/");
  } catch (error) {
    errorText.value = String(error);
    await listDir("/");
  }
}
async function listDir(path: string) {
  if (!serverId.value) return;
  loading.value = true;
  errorText.value = "";
  try {
    // Rust hub_sftp_list 返回 Vec<SftpEntry>（数组），字段是 dir/link/target/size/mtime/perms；
    // 前端渲染需要 path（cwd + name）与 is_dir（dir），这里做一次映射。
    type RawEntry = { name: string; dir?: boolean; link?: boolean; target?: string; size?: number; mtime?: string; perms?: string };
    const raw = (await call<RawEntry[]>("hub_sftp_list", {
      server: sshServers.value.find((s) => s.id === serverId.value),
      path,
    })) ?? [];
    const base = path.endsWith("/") ? path : path + "/";
    cwd.value = path;
    entries.value = raw.map((e) => ({
      name: e.name,
      path: base + e.name,
      is_dir: Boolean(e.dir),
      size: e.size ?? 0,
      mtime: e.mtime ?? "",
      perms: e.perms,
      link: e.link,
      target: e.target,
    }));
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
  if (!isTauri()) {
    // 网页端：浏览器 File picker → base64 → hub_sftp_upload_bytes
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.onchange = async () => {
      const files = Array.from(input.files ?? []);
      if (!files.length) return;
      try {
        for (const f of files) {
          const buf = await f.arrayBuffer();
          const b64 = arrayBufferToB64(buf);
          await call("hub_sftp_upload_bytes", {
            server: sshServers.value.find((s) => s.id === serverId.value),
            remoteDir: cwd.value, filename: f.name, dataB64: b64,
          });
        }
        showToast(t("fb.uploaded", { n: files.length }));
        await listDir(cwd.value);
      } catch (e) { errorText.value = String(e); }
    };
    input.click();
    return;
  }
  try {
    const uploaded = (await call("hub_sftp_upload", {
      server: sshServers.value.find((s) => s.id === serverId.value),
      remoteDir: cwd.value,
    })) as Array<unknown> | null;
    if (uploaded?.length) showToast(t("fb.uploaded", { n: uploaded.length }));
    await refreshTransfers();
    await listDir(cwd.value); // 上传的新文件要出现在列表里（旧版 finally refresh）
  } catch (error) {
    const msg = String(error);
    if (msg !== t("fb.notSelectedFile")) errorText.value = msg; // 取消选择不算错误
  }
}

async function download() {
  const entry = entries.value.find((e) => e.path === selectedPath.value);
  if (!selectedPath.value || !entry) return;
  if (!isTauri()) {
    // 网页端：hub_sftp_download_bytes 拿 base64 → Blob 触发浏览器下载
    try {
      const b64 = await call<string>("hub_sftp_download_bytes", {
        server: sshServers.value.find((s) => s.id === serverId.value),
        remotePath: selectedPath.value,
      });
      const buf = b64ToArrayBuffer(String(b64));
      const blob = new Blob([buf]);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url; a.download = entry.name; document.body.appendChild(a); a.click();
      document.body.removeChild(a); URL.revokeObjectURL(url);
      showToast(t("fb.downloaded", { name: entry.name }));
    } catch (e) { showToast("下载失败：" + e); }
    return;
  }
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
    showToast(t("fb.downloadedTo", { dir: local || downDir || t("fb.local") }));
    await refreshTransfers();
  } catch (error) {
    showToast(t("fb.dlFail", { msg: String(error) }));
  }
}

// base64 <-> ArrayBuffer（浏览器环境）
function arrayBufferToB64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let bin = "";
  const CH = 0x8000;
  for (let i = 0; i < bytes.length; i += CH) {
    bin += String.fromCharCode.apply(null, Array.from(bytes.subarray(i, i + CH)) as unknown as number[]);
  }
  return btoa(bin);
}
function b64ToArrayBuffer(b64: string): ArrayBuffer {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes.buffer;
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
  void openHome();
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

function fmtTime(v: string | number): string {
  // Rust SftpEntry.mtime 已是展示串（"10-16 09:12" / "2025-03-01"）；数字则格式化
  if (v === undefined || v === null || v === "" || v === 0) return "—";
  if (typeof v === "string") return v;
  return new Date(v).toLocaleString("zh-CN", { hour12: false }).replace(/\//g, "-");
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
    <h1 id="title">{{ currentServer?.name || $t("fb.title") }}</h1>
    <span id="connPill" class="pill" :class="errorText ? 'bad' : 'ok'" :hidden="!currentServer">
      <span class="dot" />
      <span id="connText">{{ errorText ? $t("fb.connectFail") : loading ? $t("fb.reading") : $t("fb.connected") }}</span>
    </span>
    <div class="tools">
      <WindowNavMenu class="win-nav-menu" />
      <button type="button" class="toolbtn" id="themeBtn" :aria-label="$t('keysWin.themeAria')" @click="toggleTheme">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 3a9 9 0 0 1 0 18z" fill="currentColor" stroke="none" />
        </svg>
        {{ $t("keysWin.theme") }}
      </button>
    </div>
  </header>

  <div class="pathbar">
    <button type="button" class="iconbtn" id="upBtn" :aria-label="$t('fb.up')" :title="$t('fb.up')" @click="goUp">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M19 12H5M11 18l-6-6 6-6" />
      </svg>
    </button>
    <button type="button" class="iconbtn" id="refreshBtn" :aria-label="$t('fb.refresh')" :title="$t('fb.refresh')" @click="listDir(cwd)">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6" />
      </svg>
    </button>
    <nav class="crumbs scroll-thin" id="crumbs" :aria-label="$t('fb.path')">
      <template v-for="(crumb, i) in crumbs" :key="crumb.path">
        <span v-if="i > 0" class="crumb-sep" aria-hidden="true">›</span>
        <button type="button" class="crumb" :aria-current="i === crumbs.length - 1" @click="listDir(crumb.path)">
          {{ crumb.name }}
        </button>
      </template>
    </nav>
    <button type="button" class="iconbtn" id="pathDropBtn" :aria-label="$t('fb.jump')" :title="$t('fb.jumpTitle')" @click.stop="pathMenuOpen = !pathMenuOpen">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M6 9l6 6 6-6" />
      </svg>
    </button>
    <button type="button" class="btn" id="mkdirBtn" @click="startMkdir">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 5v14M5 12h14" />
      </svg>
      {{ $t("fb.newDir") }}
    </button>
    <button type="button" class="btn primary" id="uploadBtn" @click="upload">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 19V5M5 12l7-7 7 7" />
      </svg>
      {{ $t("fb.upload") }}
    </button>
    <button type="button" class="btn" id="downloadBtn" :disabled="!selectedPath" :title="selectedPath ? $t('fb.dlSel') : $t('fb.dlNone')" @click="download">
      {{ $t("fb.download") }}
    </button>
  </div>

  <!-- 上级目录快跳菜单（旧版 #pathMenu） -->
  <div v-if="pathMenuOpen" class="popmenu" id="pathMenu" role="menu" :aria-label="$t('fb.upDirMenu')">
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
    <table :aria-label="$t('fb.title')">
      <thead>
        <tr>
          <th>{{ $t("fb.colName") }}</th>
          <th style="width: 90px">{{ $t("fb.colSize") }}</th>
          <th class="col-perms" style="width: 120px">{{ $t("fb.colPerms") }}</th>
          <th class="col-time" style="width: 130px">{{ $t("fb.colMtime") }}</th>
          <th style="width: 64px; text-align: right">{{ $t("fb.colAction") }}</th>
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
          <td class="mono col-perms">{{ entry.perms || "—" }}</td>
          <td class="mono col-time">{{ fmtTime(entry.mtime) }}</td>
          <td style="text-align: right">
            <button type="button" class="rowdl" :title="$t('fb.colAction')" :aria-label="entry.name + ' ' + $t('fb.colAction')" @click.stop="openRowMenu(entry, $event)">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><circle cx="5" cy="12" r="1.6" /><circle cx="12" cy="12" r="1.6" /><circle cx="19" cy="12" r="1.6" /></svg>
            </button>
          </td>
        </tr>
      </tbody>
    </table>
    <div v-if="!serverId" id="emptyTip" class="empty-tip">{{ $t("fb.noServer") }}</div>
    <div v-else-if="!loading && !entries.length" id="emptyTip" class="empty-tip">
      {{ errorText || $t("fb.emptyDir") }}
    </div>
  </div>

  <!-- 传输进度（旧版 #transfer） -->
  <div v-if="activeTransfer" id="transfer" class="transfer">
    <svg class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" width="14" height="14">
      <path d="M21 12a9 9 0 1 1-9-9" />
    </svg>
    <span id="transferDir" class="tbadge" :class="activeTransfer.dir === 'up' ? 'up' : 'down'">
      {{ activeTransfer.dir === "up" ? $t("tx.up") : $t("tx.down") }}
    </span>
    <span id="transferName">{{ activeTransfer.name }}</span>
    <div id="transferBar" class="bar"><div id="transferFill" :style="{ width: pct(activeTransfer) + '%' }" /></div>
    <span id="transferMeta" class="tmeta">{{ fmtSize(activeTransfer.bytes) }} / {{ fmtSize(activeTransfer.total) }}</span>
    <span id="transferPct" class="pct">{{ pct(activeTransfer) }}%</span>
    <button type="button" id="transferAct" class="tbtn" @click="transferCtl(activeTransfer.state === 'running' ? 'pause' : 'resume')">
      {{ activeTransfer.state === "running" ? $t("tx.pause") : $t("tx.resume") }}
    </button>
    <button type="button" id="transferGiveup" class="tbtn danger" @click="transferCtl('abandon')">{{ $t("tx.abandon") }}</button>
  </div>

  <footer>
    <span id="footCount">{{ $t("fb.count", { n: entries.length }) }}</span>
    <span aria-hidden="true">·</span><span>{{ $t("fb.sftp") }}</span>
  </footer>

  <!-- 行操作菜单（旧版 #rowMenu） -->
  <div v-if="rowMenuOpen" id="rowMenu" ref="rowMenuRef" class="filemenu" role="menu" :aria-label="$t('fb.fileOps')">
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
        <span class="fm-meta mono" id="fmMeta">{{ menuEntry ? (menuEntry.is_dir ? $t("fb.dir") : fmtSize(menuEntry.size)) : "—" }}</span>
      </span>
      <button type="button" id="fmClose" class="fm-close" :aria-label="$t('common.close')" @click="rowMenuOpen = false">✕</button>
    </div>
    <button role="menuitem" id="rmDownload" class="fm-act accent" @click="menuAction('download')">{{ $t("fb.download") }}</button>
    <button role="menuitem" id="rmRename" class="fm-act" @click="menuAction('rename')">{{ $t("fb.rename") }}</button>
    <button role="menuitem" id="rmCopyPath" class="fm-act" @click="menuAction('copyPath')">{{ $t("fb.copyPath") }}</button>
    <button role="menuitem" id="rmDelete" class="fm-act danger" @click="menuAction('delete')">{{ $t("fb.delete") }}</button>
    <p class="fm-label">{{ $t("fb.moreOps") }}</p>
    <div class="fm-group">
    <button role="menuitem" id="rmInfo" class="fm-act" @click="menuAction('info')">{{ $t("fb.info") }}</button>
    <button role="menuitem" id="rmCopy" class="fm-act" @click="menuAction('copy')">{{ $t("fb.copy") }}</button>
    <button role="menuitem" id="rmCut" class="fm-act" @click="menuAction('cut')">{{ $t("fb.cut") }}</button>
    <button role="menuitem" id="rmCopyTo" class="fm-act" @click="menuAction('copyTo')">{{ $t("fb.copyTo") }}</button>
    <button role="menuitem" id="rmMoveTo" class="fm-act" @click="menuAction('moveTo')">{{ $t("fb.moveTo") }}</button>
    </div>
  </div>

  <!-- 新建目录弹层（旧版 #mkdirModal） -->
  <div v-if="mkdirOpen" id="mkdirModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="mkdirTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="mkdirTitle">{{ $t("fb.mkdirTitle") }}</h3>
        <button type="button" id="mkdirClose" :aria-label="$t('common.close')" @click="mkdirOpen = false">✕</button>
      </div>
      <form id="mkdirForm" class="modal-body" @submit.prevent="confirmMkdir">
        <input id="mkdirName" v-model="mkdirName" :placeholder="$t('fb.mkdirPh')" autocomplete="off" required />
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="mkdirCancel" @click="mkdirOpen = false">{{ $t("common.cancel") }}</button>
          <button type="submit" class="btn primary" id="mkdirOk">{{ $t("fb.create") }}</button>
        </div>
      </form>
    </div>
  </div>

  <!-- 重命名弹层（旧版 #renameModal） -->
  <div v-if="renameOpen" id="renameModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="renameTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="renameTitle">{{ $t("fb.renameTitle") }}</h3>
        <button type="button" id="renameClose" :aria-label="$t('common.close')" @click="renameOpen = false">✕</button>
      </div>
      <form id="renameForm" class="modal-body" @submit.prevent="confirmRename">
        <input id="renameName" v-model="renameName" autocomplete="off" required />
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="renameCancel" @click="renameOpen = false">{{ $t("common.cancel") }}</button>
          <button type="submit" class="btn primary" id="renameOk">{{ $t("fb.rename") }}</button>
        </div>
      </form>
    </div>
  </div>

  <!-- 删除弹层（旧版 #delModal） -->
  <div v-if="delOpen" id="delModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="delTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="delTitle">{{ $t("fb.delTitle") }}</h3>
        <button type="button" id="delClose" :aria-label="$t('common.close')" @click="delOpen = false">✕</button>
      </div>
      <div class="modal-body">
        <p id="delText">{{ $t("fb.delText", { name: targetEntry?.name }) }}</p>
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="delCancel" @click="delOpen = false">{{ $t("common.cancel") }}</button>
          <button type="button" class="btn dangerbtn" id="delOk" @click="confirmDelete">{{ $t("fb.delete") }}</button>
        </div>
      </div>
    </div>
  </div>

  <!-- 详情弹层（旧版 #infoModal） -->
  <div v-if="infoOpen" id="infoModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="infoTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="infoTitle">{{ $t("fb.info") }}</h3>
        <button type="button" id="infoClose" :aria-label="$t('common.close')" @click="infoOpen = false">✕</button>
      </div>
      <div id="infoRows" class="modal-body">
        <p><b>{{ $t("fb.kName") }}</b> {{ targetEntry?.name }}</p>
        <p><b>{{ $t("fb.kSize") }}</b> {{ targetEntry?.is_dir ? "—" : fmtSize(targetEntry?.size ?? 0) }}</p>
        <p><b>{{ $t("fb.kPerms") }}</b> <span class="mono">{{ targetEntry?.perms || "—" }}</span></p>
        <p><b>{{ $t("fb.kMtime") }}</b> {{ fmtTime(targetEntry?.mtime ?? "") }}</p>
        <p><b>{{ $t("fb.kPath") }}</b> <span class="mono">{{ targetEntry?.path }}</span></p>
        <p v-if="targetEntry?.link"><b>{{ $t("fb.kLink") }}</b> <span class="mono">{{ targetEntry?.target || $t("fb.reading") }}</span></p>
      </div>
      <div class="modal-foot">
        <button type="button" class="btn primary" id="infoOk" @click="infoOpen = false">{{ $t("common.close") }}</button>
      </div>
    </div>
  </div>

  <!-- 目标目录弹层（旧版 #destModal：复制到/移动到） -->
  <div v-if="destOpen" id="destModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="destTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="destTitle">{{ destMode === "moveTo" ? $t("fb.moveTo") : $t("fb.copyTo") }}</h3>
        <button type="button" id="destClose" :aria-label="$t('common.close')" @click="destOpen = false">✕</button>
      </div>
      <form id="destForm" class="modal-body" @submit.prevent="confirmDest">
        <label id="destLabel" for="destPath">{{ $t("fb.destPath") }}</label>
        <input id="destPath" v-model="destPath" class="mono" placeholder="/path/to/dir" autocomplete="off" required />
        <p class="hint">{{ $t("fb.destHint") }}</p>
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="destCancel" @click="destOpen = false">{{ $t("common.cancel") }}</button>
          <button type="submit" class="btn primary" id="destOk">{{ $t("fb.ok") }}</button>
        </div>
      </form>
    </div>
  </div>

  <!-- toast（旧版 #toast） -->
  <div v-if="toastText" id="toast">{{ toastText }}</div>

  <div v-if="pickOpen" class="modal" @click.self="pickOpen = false">
    <div class="modal-card" role="dialog" aria-modal="true">
      <div class="modal-head">
        <h3>{{ $t("fb.selectServer") }}</h3>
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
        <p v-if="!sshServers.length" class="hint">{{ $t("fb.noSshShort") }}</p>
      </div>
    </div>
  </div>
</template>
