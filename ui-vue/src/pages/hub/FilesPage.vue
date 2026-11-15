<script setup lang="ts">
const toast = useToastStore();
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { t } from "@/i18n";
import { call, isTauri } from "@/api/tauri";
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
// 【修复】切换服务器下拉后列表不刷新：之前靠 <select @change="listDir('')">，但 v-model 的
// serverId 与 @change 同事件、顺序不保证，listDir 可能仍用旧 currentServer。改为 watch serverId：
// 无论 v-model 还是代码切换，一变就按新服务器重新列目录。初始化首设不重复拉（旧值为空跳过）。
watch(serverId, (nv, ov) => { if (nv && ov !== undefined && nv !== ov) void listDir(""); });
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

  // 传输列表 1s 轮询兜底（旧版 txPollList：事件是主路径，轮询兜底对账）
  const txPollTimer = window.setInterval(() => {
    void refreshTransfers();
  }, 1000);
  onUnmounted(() => window.clearInterval(txPollTimer));

  // Tauri 事件（进度 / 切换服务器）仅桌面端有；网页端靠 1s 轮询 + URL serverId，跳过 listen。
  if (isTauri()) {
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
  }
});

onUnmounted(() => {
  for (const off of unlisteners) off();
});

async function listDir(path: string) {
  if (!serverId.value) return;
  loading.value = true;
  errorText.value = "";
  try {
    // 【修复】后端 hub_sftp_list 返回的是**纯数组 Vec<SftpEntry>**，不是 {cwd, entries}；
    // 之前读 result.entries → undefined → 列表永远空（"这个目录是空的"）。改为直接读数组。
    // path 为空 = 先取远端家目录再列。
    let target = path;
    if (!target) {
      target = (await call<string>("hub_sftp_home", { server: currentServer.value })) || "/";
    }
    const result = await call<SftpEntry[]>("hub_sftp_list", {
      server: currentServer.value,
      path: target,
    });
    cwd.value = target;
    entries.value = Array.isArray(result) ? result : [];
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
    await call("hub_sftp_upload", { server: currentServer.value, remoteDir: cwd.value });
    await refreshTransfers();
  } catch (error) {
    toast.show(t("fb.uploadFail", { msg: String(error) }), "bad");
    errorText.value = String(error);
  }
}

async function download() {
  if (!selectedPath.value) return;
  try {
    await call("hub_sftp_download", {
      server: currentServer.value,
      remotePath: selectedPath.value,
      filename: selectedPath.value.split("/").pop() ?? "",
      localDir: "",
      total: 0,
    });
    await refreshTransfers();
  } catch (error) {
    toast.show(t("fb.dlFail", { msg: String(error) }), "bad");
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
    toast.show(t("fb.dirCreated"));
    await listDir(cwd.value);
  } catch (error) {
    toast.show(t("fb.mkdirFail", { msg: String(error) }), "bad");
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
    toast.show(t("fb.renamed"));
    await listDir(cwd.value);
  } catch (error) {
    toast.show(t("fb.renameFail", { msg: String(error) }), "bad");
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
    await call("hub_sftp_remove", { server: currentServer.value, path: entry.path, isDir: entry.is_dir });
    toast.show(t("fb.deleted"));
    await listDir(cwd.value);
  } catch (error) {
    toast.show(t("fb.delFail", { msg: String(error) }), "bad");
    errorText.value = String(error);
  }
}

/** 选本地目录（旧版 hub_pick_directory，上传时的默认落点） */
async function pickLocalDir() {
  try {
    const dir = (await call("hub_pick_directory", { current: "" })) as string;
    if (dir) errorText.value = t("fb.localDirChosen", { dir });
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
  // 软链接：异步读取指向目标（旧版 hub_sftp_readlink）
  if (entry.link && !entry.target) {
    void call<string>("hub_sftp_readlink", { server: serverId.value, path: entry.path })
      .then((tgt) => { if (targetEntry.value?.path === entry.path) targetEntry.value = { ...entry, target: String(tgt ?? "") }; })
      .catch(() => {});
  }
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

async function openInWindow() {
  if (!serverId.value) return;
  if (!isTauri()) {
    // 网页端：本机 HTTP 已提供 hub-files.html；带 token + serverId 在新标签打开
    try {
      const { getAuthToken } = await import("@/api/http");
      const tok = encodeURIComponent(getAuthToken());
      window.open(`/hub-files.html?token=${tok}&serverId=${encodeURIComponent(serverId.value)}`, "_blank");
    } catch { /* ignore */ }
    return;
  }
  void call("hub_open_files_window", { serverId: serverId.value }).catch(() => {});
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
  <div class="hub-page">
    <div class="page-head"><h2>{{ $t("fb.title") }}</h2></div>
    <div class="hub-page-body">
      <div class="fb-topbar">
        <select id="fbServer" v-model="serverId" class="fb-select" :aria-label="$t('fb.selectServerAria')">
          <option v-for="server in sshServers" :key="server.id" :value="server.id">
            {{ server.name }} · {{ store.displayAddr(server) }}
          </option>
        </select>
        <span id="fbPill" :class="['fb-pill', connected ? 'fb-ok' : 'fb-bad']">
          <span class="fb-dot" />
          <span id="fbPillText">{{ errorText ? $t("fb.connectFail") : loading ? $t("fb.reading") : $t("fb.connected") }}</span>
        </span>
        <button type="button" id="fbOpenWin" class="fb-toolbtn" :title="$t('fb.openWin')" @click="openInWindow">
          ⧉ {{ $t("fb.independent") }}
        </button>
      </div>

      <div v-if="!sshServers.length" id="fbEmpty" class="fb-empty">
        {{ $t("fb.noSsh") }}
      </div>

      <div v-else id="fbMain" class="fb-main">
        <div class="fb-pathbar">
          <button type="button" id="fbUp" class="fb-iconbtn" :aria-label="$t('fb.up')" :title="$t('fb.up')" @click="goUp">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M19 12H5M11 18l-6-6 6-6" />
            </svg>
          </button>
          <button type="button" id="fbRefresh" class="fb-iconbtn" :aria-label="$t('fb.refresh')" :title="$t('fb.refresh')" @click="listDir(cwd)">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v6h-6" />
            </svg>
          </button>
          <nav id="fbCrumbs" class="fb-crumbs scroll-thin" :aria-label="$t('fb.path')">
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
          <button type="button" id="fbHome" class="fb-iconbtn" :aria-label="$t('fb.home')" :title="$t('fb.homeTitle')" @click="goHome">⌂</button>
          <button type="button" id="fbPickLocal" class="fb-iconbtn" :aria-label="$t('fb.pickLocal')" :title="$t('fb.pickLocalTitle')" @click="pickLocalDir">＋</button>
          <button type="button" id="fbPathDrop" class="fb-iconbtn" :aria-label="$t('fb.jump')" :title="$t('fb.jumpTitle')" @click="goUp">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M6 9l6 6 6-6" />
            </svg>
          </button>
          <button type="button" id="fbMkdir" class="fb-btn" @click="mkdir">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 5v14M5 12h14" />
            </svg>
            {{ $t("fb.newDir") }}
          </button>
          <button type="button" id="fbUpload" class="fb-btn fb-primary" @click="upload">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 19V5M5 12l7-7 7 7" />
            </svg>
            {{ $t("fb.upload") }}
          </button>
          <button
            type="button"
            id="fbDownload"
            class="fb-btn"
            :disabled="!selectedPath"
            :title="selectedPath ? $t('fb.dlSel') : $t('fb.dlNone')"
            @click="download"
          >
            {{ $t("fb.download") }}
          </button>
        </div>

        <div id="fbTableWrap" class="fb-tablewrap scroll-thin">
          <table class="fb-table" :aria-label="$t('fb.title')">
            <thead>
              <tr>
                <th>{{ $t("fb.colName") }}</th>
                <th style="width: 90px">{{ $t("fb.colSize") }}</th>
                <th style="width: 120px">{{ $t("fb.colPerms") }}</th>
                <th style="width: 130px">{{ $t("fb.colMtime") }}</th>
                <th style="width: 64px; text-align: right">{{ $t("fb.colAction") }}</th>
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
                <td><span class="fb-namecell"><span class="fb-ico">{{ entry.is_dir ? "📁" : "📄" }}</span>{{ entry.name }}</span></td>
                <td class="mono">{{ entry.is_dir ? "—" : fmtSize(entry.size) }}</td>
                <td class="mono fb-dim">{{ entry.perms || "—" }}</td>
                <td class="mono">{{ fmtTime(entry.mtime) }}</td>
                <td style="text-align: right">
                  <button type="button" class="fb-rowmore" :title="$t('fb.moreOps')" @click.stop="openRowMenu(entry, $event)">⋯</button>
                </td>
              </tr>
            </tbody>
          </table>
          <div v-if="!loading && !entries.length" id="fbEmptyTip" class="fb-emptytip">{{ $t("fb.emptyDir") }}</div>
        </div>

        <!-- 行操作菜单（旧版 #fbRowMenu，9 项） -->
        <div v-if="rowMenuOpen" id="fbRowMenu" class="fb-filemenu" role="menu" :aria-label="$t('fb.fileOps')">
          <div class="fb-fm-preview">
            <span class="fb-fm-icon" id="fbFmIcon" aria-hidden="true">
              <svg v-if="!menuEntry?.is_dir" id="fbFmIcoFile" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><path d="M14 2v6h6" /></svg>
              <svg v-else id="fbFmIcoDir" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" /></svg>
              <svg id="fbFmIcoLink" v-if="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M13 2L3 14h7l-1 8 10-12h-7z" /></svg>
            </span>
            <span class="fb-fm-names">
              <span class="fb-fm-name" id="fbFmName">{{ menuEntry?.name || "—" }}</span>
              <span class="fb-fm-meta mono" id="fbFmMeta">{{ menuEntry ? (menuEntry.is_dir ? $t("fb.dir") : fmtSize(menuEntry.size)) : "—" }}</span>
            </span>
            <button type="button" class="fb-fm-close" id="fbFmClose" :aria-label="$t('common.close')" @click="rowMenuOpen = false">✕</button>
          </div>
          <div class="fb-fm-group" style="margin-top: 10px">
            <button role="menuitem" class="fb-fm-act fb-accent" id="fbRmDownload" @click="menuAction('download')">{{ $t("fb.dlTo") }}</button>
            <button role="menuitem" class="fb-fm-act" id="fbRmRename" @click="menuAction('rename')">{{ $t("fb.rename") }}</button>
            <button role="menuitem" class="fb-fm-act" id="fbRmCopyPath" @click="menuAction('copyPath')">{{ $t("fb.copyPath") }}</button>
            <button role="menuitem" class="fb-fm-act fb-danger" id="fbRmDelete" @click="menuAction('delete')">{{ $t("fb.delete") }}</button>
          </div>
          <div>
            <p class="fb-fm-label">{{ $t("fb.moreOps") }}</p>
            <div class="fb-fm-group">
              <button role="menuitem" class="fb-fm-act" id="fbRmInfo" @click="menuAction('info')">{{ $t("fb.info") }}</button>
              <button role="menuitem" class="fb-fm-act" id="fbRmCopy" @click="menuAction('copy')">{{ $t("fb.copy") }}</button>
              <button role="menuitem" class="fb-fm-act" id="fbRmCut" @click="menuAction('cut')">{{ $t("fb.cut") }}</button>
              <button role="menuitem" class="fb-fm-act" id="fbRmCopyTo" @click="menuAction('copyTo')">{{ $t("fb.copyTo") }}</button>
              <button role="menuitem" class="fb-fm-act" id="fbRmMoveTo" @click="menuAction('moveTo')">{{ $t("fb.moveTo") }}</button>
            </div>
          </div>
        </div>

        <!-- 上级目录快跳（旧版 #fbPathMenu） -->
        <div v-if="pathMenuOpen" id="fbPathMenu" class="fb-popmenu" role="menu" :aria-label="$t('fb.upDirMenu')">
          <button v-for="p in parentPaths" :key="p" type="button" @click="listDir(p); pathMenuOpen = false">{{ p }}</button>
        </div>

        <!-- 新建目录（旧版 #fbMkdirModal） -->
        <div v-if="mkdirOpen" id="fbMkdirModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbMkdirTitle">
          <div class="fb-modal-card">
            <div class="fb-modal-head">
              <h3 id="fbMkdirTitle">{{ $t("fb.mkdirTitle") }}</h3>
              <button type="button" id="fbMkdirClose" :aria-label="$t('common.close')" @click="mkdirOpen = false">✕</button>
            </div>
            <form id="fbMkdirForm" class="fb-modal-body" @submit.prevent="confirmMkdir">
              <label for="fbMkdirName">{{ $t("fb.mkdirName") }}</label>
              <input id="fbMkdirName" v-model="mkdirName" :placeholder="$t('fb.mkdirPh')" autocomplete="off" required />
            </form>
            <div class="fb-modal-foot">
              <button type="button" class="fb-btn fb-plain" id="fbMkdirCancel" @click="mkdirOpen = false">{{ $t("common.cancel") }}</button>
              <button type="button" class="fb-btn fb-primary" id="fbMkdirOk" @click="confirmMkdir">{{ $t("fb.create") }}</button>
            </div>
          </div>
        </div>

        <!-- 重命名（旧版 #fbRenameModal） -->
        <div v-if="renameOpen" id="fbRenameModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbRenameTitle">
          <div class="fb-modal-card">
            <div class="fb-modal-head">
              <h3 id="fbRenameTitle">{{ $t("fb.renameTitle") }}</h3>
              <button type="button" id="fbRenameClose" :aria-label="$t('common.close')" @click="renameOpen = false">✕</button>
            </div>
            <form id="fbRenameForm" class="fb-modal-body" @submit.prevent="confirmRename">
              <label for="fbRenameName">{{ $t("fb.newName") }}</label>
              <input id="fbRenameName" v-model="renameName" autocomplete="off" required />
            </form>
            <div class="fb-modal-foot">
              <button type="button" class="fb-btn fb-plain" id="fbRenameCancel" @click="renameOpen = false">{{ $t("common.cancel") }}</button>
              <button type="button" class="fb-btn fb-primary" id="fbRenameOk" @click="confirmRename">{{ $t("fb.ok") }}</button>
            </div>
          </div>
        </div>

        <!-- 删除（旧版 #fbDelModal） -->
        <div v-if="delOpen" id="fbDelModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbDelTitle">
          <div class="fb-modal-card">
            <div class="fb-modal-head">
              <h3 id="fbDelTitle">{{ $t("fb.delTitle") }}</h3>
              <button type="button" id="fbDelClose" :aria-label="$t('common.close')" @click="delOpen = false">✕</button>
            </div>
            <div class="fb-modal-body">
              <p id="fbDelText">{{ $t("fb.delText", { name: targetEntry?.name }) }}</p>
            </div>
            <div class="fb-modal-foot">
              <button type="button" class="fb-btn fb-plain" id="fbDelCancel" @click="delOpen = false">{{ $t("common.cancel") }}</button>
              <button type="button" class="fb-btn fb-dangerbtn" id="fbDelOk" @click="confirmDelete">{{ $t("fb.delete") }}</button>
            </div>
          </div>
        </div>

        <!-- 详情（旧版 #fbInfoModal） -->
        <div v-if="infoOpen" id="fbInfoModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbInfoTitle">
          <div class="fb-modal-card">
            <div class="fb-modal-head">
              <h3 id="fbInfoTitle">{{ $t("fb.infoTitle") }}</h3>
              <button type="button" id="fbInfoClose" :aria-label="$t('common.close')" @click="infoOpen = false">✕</button>
            </div>
            <div id="fbInfoRows" class="fb-modal-body">
              <p><b>{{ $t("fb.kName") }}</b>{{ targetEntry?.name }}</p>
              <p><b>{{ $t("fb.kPath") }}</b><span class="mono">{{ targetEntry?.path }}</span></p>
              <p><b>{{ $t("fb.kType") }}</b>{{ targetEntry?.is_dir ? $t("fb.dir") : $t("fb.file") }}</p>
              <p><b>{{ $t("fb.kSize") }}</b>{{ targetEntry?.is_dir ? "—" : fmtSize(targetEntry?.size ?? 0) }}</p>
              <p><b>{{ $t("fb.kPerms") }}</b><span class="mono">{{ targetEntry?.perms || "—" }}</span></p>
              <p v-if="targetEntry?.link"><b>{{ $t("fb.kLink") }}</b><span class="mono">{{ targetEntry?.target || $t("fb.reading") }}</span></p>
              <p><b>{{ $t("fb.kMtime") }}</b>{{ fmtTime(targetEntry?.mtime ?? 0) }}</p>
            </div>
            <div class="fb-modal-foot">
              <button type="button" class="fb-btn fb-primary" id="fbInfoOk" @click="infoOpen = false">{{ $t("common.close") }}</button>
            </div>
          </div>
        </div>

        <!-- 目标目录（旧版 #fbDestModal：复制到/移动到） -->
        <div v-if="destOpen" id="fbDestModal" class="fb-modal" role="dialog" aria-modal="true" aria-labelledby="fbDestTitle">
          <div class="fb-modal-card">
            <div class="fb-modal-head">
              <h3 id="fbDestTitle">{{ destMode === "moveTo" ? $t("fb.moveTo") : $t("fb.copyTo") }}</h3>
              <button type="button" id="fbDestClose" :aria-label="$t('common.close')" @click="destOpen = false">✕</button>
            </div>
            <form id="fbDestForm" class="fb-modal-body" @submit.prevent="confirmDest">
              <label id="fbDestLabel" for="fbDestPath">{{ $t("fb.destPath") }}</label>
              <input id="fbDestPath" v-model="destPath" class="mono" placeholder="/path/to/dir" autocomplete="off" required />
            </form>
            <div class="fb-modal-foot">
              <button type="button" class="fb-btn fb-plain" id="fbDestCancel" @click="destOpen = false">{{ $t("common.cancel") }}</button>
              <button type="button" class="fb-btn fb-primary" id="fbDestOk" @click="confirmDest">{{ $t("fb.ok") }}</button>
            </div>
          </div>
        </div>

        <!-- 传输进度（旧版 #fbTransfer，只显示当前这一条） -->
        <div v-if="activeTransfer" id="fbTransfer" class="fb-transfer">
          <svg class="fb-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" width="14" height="14">
            <path d="M21 12a9 9 0 1 1-9-9" />
          </svg>
          <span id="fbTransferDir" class="fb-tbadge" :class="activeTransfer.dir === 'up' ? 'fb-up' : 'fb-down'">
            {{ activeTransfer.dir === "up" ? $t("tx.up") : $t("tx.down") }}
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
            {{ activeTransfer.state === "running" ? $t("tx.pause") : $t("tx.resume") }}
          </button>
          <button type="button" id="fbTransferGiveup" class="fb-tbtn danger" @click="transferCtl('abandon')">
            {{ $t("tx.abandon") }}
          </button>
        </div>

        <div class="fb-foot">
          <span id="fbCount">{{ $t("fb.count", { n: entries.length }) }}</span><span aria-hidden="true">·</span
          ><span>{{ $t("fb.sftp") }}</span>
        </div>
      </div>
    </div>
  </div>
</template>
