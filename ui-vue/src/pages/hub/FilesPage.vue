<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { call } from "@/api/tauri";
import { useServersStore } from "@/stores/servers";
import FbModal from "@/components/hub/FbModal.vue";

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
      server: currentServer.value,
      from: entry.path,
      to: entry.path.replace(/[^/]+$/, next),
    });
    await listDir(cwd.value);
  } catch (error) {
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
    await listDir(cwd.value);
  } catch (error) {
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

/** 复制远端文件 / 目录（旧版 hub_sftp_copy） */
async function copyEntry(entry: SftpEntry) {
  const base = cwd.value.endsWith("/") ? cwd.value : `${cwd.value}/`;
  const target = `${base}${entry.name} (副本)`;
  try {
    await call("hub_sftp_copy", { server: currentServer.value, from: entry.path, to: target });
    await listDir(cwd.value);
  } catch (error) {
    errorText.value = String(error);
  }
}

/** 读软链接目标（旧版 hub_sftp_readlink） */
async function readLink(entry: SftpEntry) {
  try {
    const target = await call<string>("hub_sftp_readlink", { server: currentServer.value, path: entry.path });
    targetEntry.value = entry;
    infoOpen.value = true;
    errorText.value = target ? `链接指向：${target}` : "";
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
    还没有 SSH 服务器 —— 到「服务器」页添加一台，这里就能浏览它的文件。
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
              <button type="button" class="link-btn" @click.stop="copyEntry(entry)">复制</button>
              <button type="button" class="link-btn" @click.stop="readLink(entry)">链接</button>
              <button type="button" class="link-btn" @click.stop="startRename(entry)">改名</button>
              <button type="button" class="link-btn" @click.stop="showInfo(entry)">属性</button>
              <button type="button" class="link-btn" @click.stop="removeEntry(entry)">删除</button>
            </td>
          </tr>
        </tbody>
      </table>
      <div v-if="!loading && !entries.length" id="fbEmptyTip" class="fb-emptytip">这个目录是空的</div>
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

    <!-- 5 个弹层（旧版 #fbMkdirModal / #fbRenameModal / #fbDelModal / #fbInfoModal / #fbDestModal） -->
    <FbModal :open="mkdirOpen" title="新建目录" confirm-text="创建" @confirm="confirmMkdir" @cancel="mkdirOpen = false">
      <label for="fbMkdirName">目录名（当前目录下创建）</label>
      <input id="fbMkdirName" v-model="mkdirName" placeholder="例如 backup" autocomplete="off" />
    </FbModal>

    <FbModal :open="renameOpen" title="重命名" confirm-text="确定" @confirm="confirmRename" @cancel="renameOpen = false">
      <label for="fbRenameName">新名称</label>
      <input id="fbRenameName" v-model="renameName" autocomplete="off" />
    </FbModal>

    <FbModal
      :open="delOpen"
      title="删除"
      confirm-text="删除"
      :danger="true"
      @confirm="confirmDelete"
      @cancel="delOpen = false"
    >
      <p class="field-hint">确定删除「{{ targetEntry?.name }}」？此操作不可恢复。</p>
    </FbModal>

    <FbModal :open="infoOpen" title="属性" confirm-text="关闭" :cancel-text="' '" @confirm="infoOpen = false" @cancel="infoOpen = false">
      <div class="fb-info-rows">
        <p><b>名称</b>{{ targetEntry?.name }}</p>
        <p><b>路径</b><span class="mono">{{ targetEntry?.path }}</span></p>
        <p><b>类型</b>{{ targetEntry?.is_dir ? "目录" : "文件" }}</p>
        <p><b>大小</b>{{ targetEntry?.is_dir ? "—" : fmtSize(targetEntry?.size ?? 0) }}</p>
        <p><b>权限</b><span class="mono">{{ targetEntry?.perms || "—" }}</span></p>
        <p><b>修改时间</b>{{ fmtTime(targetEntry?.mtime ?? 0) }}</p>
      </div>
    </FbModal>

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
