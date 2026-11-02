<script setup lang="ts">
const toast = useToastStore();
import { computed, onMounted, onUnmounted, ref } from "vue";
import { call } from "@/api/tauri";
import { useServersStore } from "@/stores/servers";
import { useToastStore } from "@/stores/toast";

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
/** 弹层状态（旧版 5 个 .fb-modal） */
const mkdirOpen = ref(false);
const mkdirName = ref("");
const renameOpen = ref(false);
const renameName = ref("");
const delOpen = ref(false);
const infoOpen = ref(false);
/** 当前操作的目标条目 */
const targetEntry = ref<SftpEntry | null>(null);

/** 行操作菜单（旧版 #fbRowMenu） */
const rowMenuOpen = ref(false);
const menuEntry = ref<SftpEntry | null>(null);
/** 上级目录快跳（旧版 #fbPathMenu） */
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
/** 目标目录弹层（旧版 #fbDestModal） */
const destOpen = ref(false);
const destMode = ref<"copyTo" | "moveTo">("copyTo");
const destPath = ref("");

function openRowMenu(entry: SftpEntry, _e: MouseEvent) {
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
      void navigator.clipboard.writeText(entry.path).catch(() => {});
      break;
    case "delete":
      removeEntry(entry);
      break;
    case "info":
      showInfo(entry);
      break;
    case "copy":
    case "cut":
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
    const { call } = await import("@/api/tauri");
    if (destMode.value === "copyTo") {
      await call("hub_sftp_copy", { server: serverId.value, from: entry.path, to: target });
    } else {
      await call("hub_sftp_rename", { server: serverId.value, from: entry.path, to: target });
    }
    await listDir(cwd.value);
  } catch {
    /* 忽略 */
  }
}

const sshServers = computed(() => store.servers.filter((s) => s.protocol === "ssh"));
/** 当前选中的服务器对象（hub_sftp_* 要传整个对象而不是 id） */
const currentServer = computed(() => sshServers.value.find((s) => s.id === serverId.value));
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
      server: currentServer.value,
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
    await call("hub_sftp_upload", { server: currentServer.value, remote_dir: cwd.value });
    await refreshTransfers();
  } catch (error) {
    toast.show("上传失败：" + error, "bad");
    errorText.value = String(error);
  }
}

async function download() {
  if (!selectedPath.value) return;
  try {
    await call("hub_sftp_download", {
      server: currentServer.value,
      remote_path: selectedPath.value,
      filename: selectedPath.value.split("/").pop() ?? "",
      local_dir: "",
      total: 0,
    });
    await refreshTransfers();
  } catch (error) {
    toast.show("下载失败：" + error, "bad");
    errorText.value = String(error);
  }
}

function mkdir() {
  mkdirName.value = "";
  mkdirOpen.value = true;
}

async function confirmMkdir() {
  const name = mkdirName.value.trim();
  mkdirOpen.value = false;
  if (!name) return;
  const base = cwd.value.endsWith("/") ? cwd.value : `${cwd.value}/`;
  try {
    await call("hub_sftp_mkdir", { server: currentServer.value, path: `${base}${name}` });
    toast.show("目录已创建 ✓");
    await listDir(cwd.value);
  } catch (error) {
    toast.show("创建目录失败：" + error, "bad");
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
      server: currentServer.value,
      from: entry.path,
      to: entry.path.replace(/[^/]+$/, next),
    });
    toast.show("已重命名");
    await listDir(cwd.value);
  } catch (error) {
    toast.show("重命名失败：" + error, "bad");
    errorText.value = String(error);
  }
}

function removeEntry(entry: SftpEntry) {
  targetEntry.value = entry;
  delOpen.value = true;
}

async function confirmDelete() {
  const entry = targetEntry.value;
  delOpen.value = false;
  if (!entry) return;
  try {
    await call("hub_sftp_remove", { server: currentServer.value, path: entry.path, is_dir: entry.is_dir });
    toast.show("已删除");
    await listDir(cwd.value);
  } catch (error) {
    toast.show("删除失败：" + error, "bad");
    errorText.value = String(error);
  }
}

/** 选本地目录（旧版 hub_pick_directory，上传时的默认落点） */
async function pickLocalDir() {
  try {
    const dir = (await call("hub_pick_directory", { current: "" })) as string;
    if (dir) errorText.value = `已选择本地目录：${dir}`;
  } catch (error) {
    errorText.value = String(error);
  }
}

/** 回到远端家目录（旧版 hub_sftp_home） */
async function goHome() {
  try {
    const home = await call<string>("hub_sftp_home", { server: currentServer.value });
    if (home) await listDir(home);
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

function openInWindow() {
  if (!serverId.value) return;
  void call("hub_open_files_window", { server_id: serverId.value }).catch(() => {});
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
    <select id="fbServer" v-model="serverId" class="fb-select" aria-label="选择服务器" @change="listDir('')">
      <option v-for="server in sshServers" :key="server.id" :value="server.id">
        {{ server.name }} · {{ store.displayAddr(server) }}
      </option>
    </select>
    <span id="fbPill" :class="['fb-pill', connected ? 'fb-ok' : 'fb-bad']">
      <span class="fb-dot" />
      <span id="fbPillText">{{ errorText ? "连接失败" : loading ? "读取中…" : "已连接" }}</span>
    </span>
    <button type="button" id="fbOpenWin" class="fb-toolbtn" title="在独立窗口打开当前服务器的文件浏览" @click="openInWindow">
      ⧉ 独立窗口
    </button>
  </div>

  <div v-if="!sshServers.length" id="fbEmpty" class="fb-empty">
    还没有 SSH 服务器——先在「服务器」页添加一台并完成认证（先开一次终端）。
  </div>

  <div v-else id="fbMain" class="fb-main">
    <div class="fb-pathbar">
      <button type="button" id="fbUp" class="fb-iconbtn" aria-label="上一级" title="上一级" @click="goUp">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M19 12H5M11 18l-6-6 6-6" />
        </svg>
      </button>
      <button type="button" id="fbRefresh" class="fb-iconbtn" aria-label="刷新" title="刷新" @click="listDir(cwd)">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6" />
        </svg>
      </button>
      <nav id="fbCrumbs" class="fb-crumbs scroll-thin" aria-label="路径">
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
      <button type="button" id="fbHome" class="fb-iconbtn" aria-label="家目录" title="回到远端家目录" @click="goHome">⌂</button>
      <button type="button" id="fbPickLocal" class="fb-iconbtn" aria-label="选择本地目录" title="选择本地目录（上传落点）" @click="pickLocalDir">＋</button>
      <button type="button" id="fbPathDrop" class="fb-iconbtn" aria-label="路径快跳" title="上级目录快跳" @click="goUp">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>
      <button type="button" id="fbMkdir" class="fb-btn" @click="mkdir">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 5v14M5 12h14" />
        </svg>
        新建目录
      </button>
      <button type="button" id="fbUpload" class="fb-btn fb-primary" @click="upload">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 19V5M5 12l7-7 7 7" />
        </svg>
        上传
      </button>
      <button
        type="button"
        id="fbDownload"
        class="fb-btn"
        :disabled="!selectedPath"
        :title="selectedPath ? '下载选中的文件' : '未选中文件'"
        @click="download"
      >
        下载
      </button>
    </div>

    <div id="fbTableWrap" class="fb-tablewrap scroll-thin">
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
        <tbody id="fbTbody">
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
              <button type="button" class="fb-rowmore" title="更多操作" @click.stop="openRowMenu(entry, $event)">⋯</button>
            </td>
          </tr>
        </tbody>
      </table>
      <div v-if="!loading && !entries.length" id="fbEmptyTip" class="fb-emptytip">这个目录是空的</div>
    </div>

    <!-- 行操作菜单（旧版 #fbRowMenu，9 项） -->
    <div v-if="rowMenuOpen" id="fbRowMenu" class="fb-filemenu" role="menu" aria-label="文件操作">
      <div class="fb-fm-preview">
        <span class="fb-fm-icon" id="fbFmIcon" aria-hidden="true">
          <svg v-if="!menuEntry?.is_dir" id="fbFmIcoFile" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><path d="M14 2v6h6" /></svg>
          <svg v-else id="fbFmIcoDir" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" /></svg>
          <svg id="fbFmIcoLink" v-if="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2L3 14h7l-1 8 10-12h-7z" /></svg>
        </span>
        <span class="fb-fm-names">
          <span class="fb-fm-name" id="fbFmName">{{ menuEntry?.name || "—" }}</span>
          <span class="fb-fm-meta mono" id="fbFmMeta">{{ menuEntry ? (menuEntry.is_dir ? "目录" : fmtSize(menuEntry.size)) : "—" }}</span>
        </span>
        <button type="button" class="fb-fm-close" id="fbFmClose" aria-label="关闭" @click="rowMenuOpen = false">✕</button>
      </div>
      <div class="fb-fm-group" style="margin-top: 10px">
        <button role="menuitem" class="fb-fm-act fb-accent" id="fbRmDownload" @click="menuAction('download')">下载到本机</button>
        <button role="menuitem" class="fb-fm-act" id="fbRmRename" @click="menuAction('rename')">重命名</button>
        <button role="menuitem" class="fb-fm-act" id="fbRmCopyPath" @click="menuAction('copyPath')">复制路径</button>
        <button role="menuitem" class="fb-fm-act fb-danger" id="fbRmDelete" @click="menuAction('delete')">删除</button>
        <button role="menuitem" class="fb-fm-act" id="fbRmInfo" @click="menuAction('info')">详情</button>
        <button role="menuitem" class="fb-fm-act" id="fbRmCopy" @click="menuAction('copy')">复制</button>
        <button role="menuitem" class="fb-fm-act" id="fbRmCut" @click="menuAction('cut')">剪切</button>
        <button role="menuitem" class="fb-fm-act" id="fbRmCopyTo" @click="menuAction('copyTo')">复制到…</button>
        <button role="menuitem" class="fb-fm-act" id="fbRmMoveTo" @click="menuAction('moveTo')">移动到…</button>
      </div>
    </div>

    <!-- 上级目录快跳（旧版 #fbPathMenu） -->
    <div v-if="pathMenuOpen" id="fbPathMenu" class="fb-popmenu" role="menu" aria-label="上级目录">
      <button v-for="p in parentPaths" :key="p" type="button" class="fb-popmenu-item" @click="listDir(p); pathMenuOpen = false">{{ p }}</button>
    </div>

    <!-- 新建目录（旧版 #fbMkdirModal） -->
    <div v-if="mkdirOpen" id="fbMkdirModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbMkdirTitle">
      <div class="fb-modal-card">
        <div class="fb-modal-head">
          <h3 id="fbMkdirTitle">新建目录</h3>
          <button type="button" id="fbMkdirClose" aria-label="关闭" @click="mkdirOpen = false">✕</button>
        </div>
        <form id="fbMkdirForm" class="fb-modal-body" @submit.prevent="confirmMkdir">
          <label for="fbMkdirName">目录名（当前目录下创建）</label>
          <input id="fbMkdirName" v-model="mkdirName" placeholder="例如 backup" autocomplete="off" required />
        </form>
        <div class="fb-modal-foot">
          <button type="button" class="fb-btn fb-plain" id="fbMkdirCancel" @click="mkdirOpen = false">取消</button>
          <button type="button" class="fb-btn fb-primary" id="fbMkdirOk" @click="confirmMkdir">创建</button>
        </div>
      </div>
    </div>

    <!-- 重命名（旧版 #fbRenameModal） -->
    <div v-if="renameOpen" id="fbRenameModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbRenameTitle">
      <div class="fb-modal-card">
        <div class="fb-modal-head">
          <h3 id="fbRenameTitle">重命名</h3>
          <button type="button" id="fbRenameClose" aria-label="关闭" @click="renameOpen = false">✕</button>
        </div>
        <form id="fbRenameForm" class="fb-modal-body" @submit.prevent="confirmRename">
          <label for="fbRenameName">新名称</label>
          <input id="fbRenameName" v-model="renameName" autocomplete="off" required />
        </form>
        <div class="fb-modal-foot">
          <button type="button" class="fb-btn fb-plain" id="fbRenameCancel" @click="renameOpen = false">取消</button>
          <button type="button" class="fb-btn fb-primary" id="fbRenameOk" @click="confirmRename">确定</button>
        </div>
      </div>
    </div>

    <!-- 删除（旧版 #fbDelModal） -->
    <div v-if="delOpen" id="fbDelModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbDelTitle">
      <div class="fb-modal-card">
        <div class="fb-modal-head">
          <h3 id="fbDelTitle">删除</h3>
          <button type="button" id="fbDelClose" aria-label="关闭" @click="delOpen = false">✕</button>
        </div>
        <div class="fb-modal-body">
          <p id="fbDelText">确定删除「{{ targetEntry?.name }}」？此操作不可恢复。</p>
        </div>
        <div class="fb-modal-foot">
          <button type="button" class="fb-btn fb-plain" id="fbDelCancel" @click="delOpen = false">取消</button>
          <button type="button" class="fb-btn fb-primary fb-danger" id="fbDelOk" @click="confirmDelete">删除</button>
        </div>
      </div>
    </div>

    <!-- 详情（旧版 #fbInfoModal） -->
    <div v-if="infoOpen" id="fbInfoModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbInfoTitle">
      <div class="fb-modal-card">
        <div class="fb-modal-head">
          <h3 id="fbInfoTitle">属性</h3>
          <button type="button" id="fbInfoClose" aria-label="关闭" @click="infoOpen = false">✕</button>
        </div>
        <div id="fbInfoRows" class="fb-modal-body">
          <p><b>名称</b>{{ targetEntry?.name }}</p>
          <p><b>路径</b><span class="mono">{{ targetEntry?.path }}</span></p>
          <p><b>类型</b>{{ targetEntry?.is_dir ? "目录" : "文件" }}</p>
          <p><b>大小</b>{{ targetEntry?.is_dir ? "—" : fmtSize(targetEntry?.size ?? 0) }}</p>
          <p><b>权限</b><span class="mono">{{ targetEntry?.perms || "—" }}</span></p>
          <p><b>修改时间</b>{{ fmtTime(targetEntry?.mtime ?? 0) }}</p>
        </div>
        <div class="fb-modal-foot">
          <button type="button" class="fb-btn fb-primary" id="fbInfoOk" @click="infoOpen = false">关闭</button>
        </div>
      </div>
    </div>

    <!-- 目标目录（旧版 #fbDestModal：复制到/移动到） -->
    <div v-if="destOpen" id="fbDestModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbDestTitle">
      <div class="fb-modal-card">
        <div class="fb-modal-head">
          <h3 id="fbDestTitle">{{ destMode === "moveTo" ? "移动到…" : "复制到…" }}</h3>
          <button type="button" id="fbDestClose" aria-label="关闭" @click="destOpen = false">✕</button>
        </div>
        <form id="fbDestForm" class="fb-modal-body" @submit.prevent="confirmDest">
          <label id="fbDestLabel" for="fbDestPath">目标路径（含文件名）</label>
          <input id="fbDestPath" v-model="destPath" class="mono" placeholder="/path/to/dir" autocomplete="off" required />
        </form>
        <div class="fb-modal-foot">
          <button type="button" class="fb-btn fb-plain" id="fbDestCancel" @click="destOpen = false">取消</button>
          <button type="button" class="fb-btn fb-primary" id="fbDestOk" @click="confirmDest">确定</button>
        </div>
      </div>
    </div>

    <!-- 传输进度（旧版 #fbTransfer，只显示当前这一条） -->
    <div v-if="activeTransfer" id="fbTransfer" class="fb-transfer">
      <svg class="fb-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" width="14" height="14">
        <path d="M21 12a9 9 0 1 1-9-9" />
      </svg>
      <span id="fbTransferDir" class="fb-tbadge" :class="activeTransfer.dir === 'up' ? 'fb-up' : 'fb-down'">
        {{ activeTransfer.dir === "up" ? "↑ 上传" : "↓ 下载" }}
      </span>
      <span id="fbTransferName">{{ activeTransfer.name }}</span>
      <div id="fbTransferBar" class="fb-bar">
        <div id="fbTransferFill" :style="{ width: pct(activeTransfer) + '%' }" />
      </div>
      <span id="fbTransferMeta" class="fb-tmeta">{{ fmtSize(activeTransfer.bytes) }} / {{ fmtSize(activeTransfer.total) }}</span>
      <span id="fbTransferPct" class="fb-pct">{{ pct(activeTransfer) }}%</span>
      <button
        type="button"
        id="fbTransferAct"
        class="fb-tbtn"
        @click="transferCtl(activeTransfer.state === 'running' ? 'pause' : 'resume')"
      >
        {{ activeTransfer.state === "running" ? "暂停" : "继续" }}
      </button>
      <button type="button" id="fbTransferGiveup" class="fb-tbtn danger" @click="transferCtl('abandon')">
        放弃
      </button>
    </div>

    <div class="fb-foot">
      <span id="fbCount">{{ entries.length }} 项</span><span aria-hidden="true">·</span
      ><span>SFTP 复用 SSH 会话</span>
    </div>
  </div>
</template>
