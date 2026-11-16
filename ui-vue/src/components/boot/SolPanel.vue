<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { call } from "@/api/tauri";

/**
 * SOL 远程唤醒页（原 `ui/index.html` 的 `#tab-sol`，835~1050 行）。
 *
 * ## 1:1 结构（旧版 5 个 .setting + 动作区 + 安装包块）
 * ```
 * section.settings#sol-section
 *   p.sol-desc
 *   .setting#sol-state-row     服务状态 + .remote-inline（刷新/特权/重启）
 *   .setting#sol-http-enabled-row
 *   .setting#sol-http-listen-row  （含 .setting-state + input + 保存）
 *   .setting.setting-block#sol-http-token-row
 *     .sol-token-line × 2（当前 token + 复制 / 新 token + 生成 + 应用）
 *   .actions > 一键安装 / 手动安装
 *   small.sol-hint（安装说明，原文）
 *   .setting-block.sol-pkg-block
 *     安装包状态 + .sol-progress（进度条）
 *     .setting 安装目录（input + 选择 + 打开）
 *     .remote-inline（下载 / 选择安装包 / 解压）
 *     small.sol-hint × 2（说明 + 项目链接）
 * ```
 */

interface SolListenEntry { port?: number; [k: string]: unknown }
interface SolStatus {
  installed?: boolean;
  binPath?: string;
  version?: string;
  serviceLoaded?: boolean;
  serviceActive?: boolean;
  serviceDetail?: string;
  processRunning?: boolean;
  listening?: SolListenEntry[];
  configPath?: string;
  configExists?: boolean;
  recentLogs?: string[];
  statusNeedsPrivilege?: boolean;
  [key: string]: unknown;
}

import { useToastStore } from "@/stores/toast";
const { t } = useI18n();
const toast = useToastStore();
const status = ref<SolStatus | null>(null);
const loading = ref(true);
const errorText = ref("");

const listenAddr = ref("");
const newToken = ref("");
const installDir = ref("");
/** 选中的本地安装包路径（旧版 solPkgArchivePath） */
const archivePath = ref("");
const progress = ref(0);
const logs = ref<string[]>([]);
const httpEnabled = ref(false);
const httpToken = ref("");
const installDirPlaceholder = ref("");
const projectVersion = ref("");
/** 「下载」实际会请求的完整地址（含当前所选加速前缀）——页面上明示，便于排查 404 */
const downloadUrl = ref("");
async function refreshProject(): Promise<void> {
  try {
    const info =
      (await call<{ version?: string; latest?: string; downloadUrl?: string }>(
        "sol_project_info",
        { mirror: mirrorArg() },
      )) ?? {};
    projectVersion.value = info.version || info.latest || "";
    downloadUrl.value = info.downloadUrl || "";
  } catch { /* 忽略 */ }
}
/** 旧版 sol_project_open：打开 GitHub 仓库 / Releases 页 */
async function openProject(page: "repo" | "releases") {
  try {
    await call("sol_project_open", { page });
  } catch (error) {
    showToast(String(error));
  }
}
const httpEnabledState = ref("");
/** 状态条消息（旧版 showStatusBar） */
const statusMsg = ref("");
function showToast(text: string) {
  statusMsg.value = text;
  window.setTimeout(() => {
    if (statusMsg.value === text) statusMsg.value = "";
  }, 2600);
}
const configText = ref("");
/** 当前编辑的配置文件路径（旧版 solConfigEditingPath：读回给写用） */
const configPath = ref("");
const configStatus = ref(t("sol.readingCfg"));
async function refreshConfig() {
  configStatus.value = t("sol.readingCfg");
  try {
    const result = await call<{ path?: string; content?: string }>("sol_config_read", { path: null });
    configPath.value = result?.path ?? "";
    configText.value = result?.content ?? "";
    configStatus.value = configPath.value || "—";
  } catch {
    configPath.value = "";
    configText.value = "";
    configStatus.value = t("sol.cfgNotFound");
  }
}
function saveConfig() {
  void call("sol_config_write", { path: configPath.value || "", content: configText.value })
    .then(() => refreshConfig())
    .catch(() => {});
}

function clearLogs() {
  logs.value = [];
}
const downloading = ref(false);
/** 点过「取消下载」：后端收尾事件回来时按「已取消」提示，不显示成失败 */
const cancelRequested = ref(false);
/** 最近一次 done 事件的时间戳：done 之后 3 秒内的滞后进度事件直接忽略
 *  （否则「下载完成」之后又闪回「下载中 0%」——用户实测） */
let lastDoneAt = 0;
/** 本地安装包现状（下载/删除/进页面后刷新；状态行的依据） */
interface LocalPkg {
  archive?: string | null;
  size?: number;
  md5?: string | null;
  verified?: boolean;
  extracted?: boolean;
  dir?: string;
}
const localPkg = ref<LocalPkg | null>(null);
const deleteArmed = ref(false);
async function refreshLocalPkg(): Promise<void> {
  try {
    localPkg.value =
      (await call<LocalPkg>("sol_local_package", { dir: installDir.value || null })) ?? null;
  } catch { /* 忽略 */ }
}
/** 人类可读的包大小 */
function fmtSize(bytes?: number): string {
  if (!bytes) return "0 B";
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${bytes} B`;
}
/** 「重新下载」：force=true 跳过复用、先删旧包再下 */
async function redownload(): Promise<void> {
  await download(true);
}
/** 「删除安装包」：两段式确认（先点一下进入待确认，再点才删），避免误触 */
async function deletePackage(): Promise<void> {
  if (!localPkg.value?.archive) return;
  if (!deleteArmed.value) {
    deleteArmed.value = true;
    setTimeout(() => { deleteArmed.value = false; }, 3000);
    return;
  }
  deleteArmed.value = false;
  try {
    localPkg.value =
      (await call<LocalPkg>("sol_delete_package", { dir: installDir.value || null })) ?? null;
    toast.show(t("sol.pkgDeleted"));
    logs.value.push(`${nowTime()} ${localPkg.value.archive ? t("sol.pkgDeleted") : t("sol.pkgDeletedMd5")}`);
  } catch (error) {
    toast.show(t("sol.delFail", { msg: String(error) }), "bad");
    logs.value.push(`${nowTime()} ${t("sol.delFail", { msg: String(error) })}`);
  }
}
/** 服务状态被权限挡住时显示「授权查看状态」（旧版 #sol-status-privileged） */
const privilegedNeeded = ref(false);
async function readPrivilegedStatus() {
  try {
    // 返回可用的 SolStatus（提权读后的真实状态）→ 直接应用并刷新界面
    const st = await call<SolStatus>("sol_status_privileged");
    if (st) {
      status.value = st;
      privilegedNeeded.value = Boolean(st.statusNeedsPrivilege);
    } else {
      privilegedNeeded.value = false;
    }
  } catch {
    /* 忽略 */
  }
}

/** 运行状态一行话（旧版 renderSolStatus）：版本 + 服务 + 进程 + 需授权 */
const stateText = computed(() => {
  if (loading.value) return t("sol.loading");
  const s = status.value;
  if (errorText.value) return ;
  if (!s) return "—";
  const parts: string[] = [];
  parts.push(s.installed ? (s.version || t("sol.installed")) : t("sol.notInstalled"));
  if (s.serviceLoaded) parts.push(s.serviceActive ? t("sol.serviceRunning") : t("sol.serviceStopped"));
  else if (s.installed) parts.push(t("sol.serviceUnregistered"));
  if (!s.serviceActive && s.processRunning) parts.push(t("sol.procRunning"));
  if (s.statusNeedsPrivilege) parts.push(t("sol.needPriv"));
  return parts.join(" · ");
});
/** 运行状态是否异常（旧版 setting-state-error = !installed || !serviceActive） */
const stateError = computed(() => {
  const s = status.value;
  return Boolean(errorText.value) || !s || !s.installed || !s.serviceActive;
});
const stateDetail = computed(() => status.value?.serviceDetail || "");

async function refresh() {
  loading.value = true;
  errorText.value = "";
  try {
    status.value = await call<SolStatus>("sol_status");
    const listenPort = (status.value?.listening ?? []).map((x) => x.port).find((n) => typeof n === "number");
    listenAddr.value = listenPort ? `0.0.0.0:${listenPort}` : "";
    // 安装目录 = 配置文件的目录（configPath 指向 <installDir>/sol.yaml）
    const cfgPath = status.value?.configPath ?? "";
    installDir.value = cfgPath ? cfgPath.replace(/[/\\][^/\\]+$/, "") : "";
    privilegedNeeded.value = Boolean(status.value?.statusNeedsPrivilege);
    // 旧版 refreshSolHttp：sol_http_get 回填 HTTP 开关/监听/当前 token
    try {
      const http = (await call<{ enabled?: boolean; listen?: string; token?: string; token_set?: boolean; tokenSet?: boolean }>("sol_http_get")) ?? {};
      httpEnabled.value = Boolean(http.enabled);
      httpToken.value = http.token ?? "";
      httpEnabledState.value = http.enabled ? t("sol.httpOn") : t("sol.httpOff");
    } catch { /* 忽略 */ }
    // 旧版 refreshSolPackage：sol_local_package 回填本地安装包状态
    await refreshLocalPkg();
  } catch (error) {
    errorText.value = String(error);
    status.value = null;
  } finally {
    loading.value = false;
  }
}

onMounted(async () => {
  await refresh();
  void refreshConfig(); // 旧版 boot：读配置文件（否则「配置文件」面板永远停在读取中/空白）
  // 旧版：sol_default_dir 懒加载默认安装目录（placeholder）
  try {
    const dir = (await call<string>("sol_default_dir")) || "";
    if (dir && !installDir.value) installDirPlaceholder.value = dir;
  } catch { /* 忽略 */ }
  // 旧版：sol_project_info 回填项目版本行 + 实际下载地址
  void refreshProject();
  // 历史日志（旧版 loadSolLogs：sol_logs 回填本会话 [sol] 段）
  try {
    const history = (await call<string[]>("sol_logs")) ?? [];
    for (const entry of history) logs.value.push(String(entry));
  } catch { /* 忽略 */ }
  const { listen } = await import("@tauri-apps/api/event");
  unlisteners.push(
    await listen<{ message?: string; time?: string }>("sol-log", (event) => {
      const { message, time } = event.payload || {};
      logs.value.push(`${time || nowTime()} ${String(message || "")}`);
    }),
  );
  // 下载进度（旧版 sol-download-progress：done→收尾，否则画百分比）
  unlisteners.push(
    await listen<{ bytes?: number; total?: number; done?: boolean; ok?: boolean }>(
      "sol-download-progress",
      (event) => {
        const { bytes, total, done, ok } = event.payload || {};
        if (done) {
          lastDoneAt = Date.now();
          progress.value = ok ? 100 : progress.value;
          setTimeout(() => { downloading.value = false; }, ok ? 1500 : 200);
          return;
        }
        if (Date.now() - lastDoneAt < 3000) return; // done 之后的滞后进度事件，忽略
        downloading.value = true;
        progress.value = total ? Math.round(((bytes ?? 0) / total) * 100) : 0;
      },
    ),
  );
});
onUnmounted(() => { for (const off of unlisteners) off(); });
const unlisteners: (() => void)[] = [];
function nowTime() {
  return new Date().toLocaleTimeString("zh-CN", { hour12: false });
}

async function setHttpEnabled() {
  try {
    await call("sol_http_set", { path: null, option: "enabled", value: String(!httpEnabled.value) });
    await refresh();
  } catch (error) {
    showToast(String(error));
  }
}

function saveListen() {
  void call("sol_http_set", { path: null, option: "listen", value: listenAddr.value.trim() }).catch(() => {});
}

async function copyToken() {
  try {
    await navigator.clipboard.writeText(httpToken.value || "");
  } catch {
    /* 剪贴板不可用 */
  }
}

async function generateToken() {
  // 旧版：后端生成并写入新 token（crypto 自造格式不对）
  try {
    const token = (await call<string>("sol_token_generate")) || "";
    const message = (await call<string>("sol_http_set", { path: null, option: "token", value: token })) || t("sol.newTokenWritten");
    newToken.value = "";
    showToast(message);
    await refresh();
  } catch (error) {
    showToast(String(error));
  }
}

function saveToken() {
  void call("sol_http_set", { path: null, option: "token", value: newToken.value.trim() }).catch(() => {});
}

function oneClick() {
  void call("sol_one_click_install", { dir: null, archive: null }).catch(() => {});
}

function manualInstall() {
  void call("sol_one_click_install_terminal", { dir: null, archive: null }).catch(() => {});
}

/** GitHub 下载加速：默认「不走加速」直连；预设为社区镜像（可用性随时变），
 *  也支持自定义前缀。选择存 localStorage（本机偏好，桌面端用）。 */
const MIRROR_KEY = "solDownloadMirror";
const MIRROR_PRESETS: { id: string; label: string }[] = [
  { id: "off", label: "sol.mirrorOff" },
  { id: "ghfast.top", label: "sol.mirror.ghfast" },
  { id: "gh-proxy.com", label: "sol.mirror.ghProxyCom" },
  { id: "ghproxy.net", label: "sol.mirror.ghproxyNet" },
  { id: "mirror.ghproxy.com", label: "sol.mirror.mirrorGhproxyCom" },
  { id: "custom", label: "sol.mirrorCustom" },
];
const mirror = ref("off");
const mirrorCustom = ref("");
try {
  mirror.value = localStorage.getItem(MIRROR_KEY) || "off";
  mirrorCustom.value = localStorage.getItem(`${MIRROR_KEY}:custom`) || "";
} catch { /* 无 localStorage 环境忽略 */ }
function saveMirror(): void {
  try {
    localStorage.setItem(MIRROR_KEY, mirror.value);
    localStorage.setItem(`${MIRROR_KEY}:custom`, mirrorCustom.value);
  } catch { /* 忽略 */ }
  void refreshProject(); // 地址随镜像变
}
/** 后端 mirror 参数：off → 不传；预设 → 名字；自定义 → 前缀原文（http(s):// 开头） */
function mirrorArg(): string | null {
  if (mirror.value === "off") return null;
  if (mirror.value === "custom") return mirrorCustom.value.trim() || null;
  return mirror.value;
}

async function download(force = false) {
  cancelRequested.value = false;
  const mirrorValue = mirrorArg();
  if (mirror.value === "custom" && !mirrorValue) {
    logs.value.push(`${nowTime()} ${t("sol.mirrorEmpty")}`);
    return;
  }
  downloading.value = true;
  progress.value = 0;
  // 事件只是进度来源，**收尾必须自己兜底**：复用路径秒回、spawn 失败等都可能
  // 不带 done 事件（旧版吞错误后永远「下载中 0%」）。失败原因从 catch 里透出。
  try {
    const path = await call<string>("sol_install_download", {
      dir: null,
      mirror: mirrorValue,
      force: force || undefined,
    });
    progress.value = 100;
    toast.show(t("sol.dlDone", { name: String(path).split(/[\\/]/).pop() ?? "" }));
  } catch (error) {
    const text = String(error);
    if (cancelRequested.value) {
      toast.show(t("sol.dlCanceled"));
    } else {
      toast.show(t("sol.dlFail", { msg: String(text) }), "bad");
    }
    logs.value.push(`${nowTime()} ${cancelRequested.value ? t("sol.dlCanceled") : t("sol.dlFail", { msg: String(text) })}`);
  } finally {
    setTimeout(() => { downloading.value = false; }, 300);
    void refreshLocalPkg();
    void refreshProject();
  }
}

/** 「取消下载」：kill 下载子进程，半截包由后端清理 */
async function cancelDownload(): Promise<void> {
  if (!downloading.value) return;
  cancelRequested.value = true;
  try {
    await call<boolean>("sol_download_cancel");
  } catch (error) {
    toast.show(t("sol.cancelFail", { msg: String(error) }), "bad");
    logs.value.push(`${nowTime()} ${t("sol.cancelFail", { msg: String(error) })}`);
  }
}

function restartService() {
  void call("sol_service_ctl", { action: "restart" }).catch(() => {});
}

async function pickArchive() {
  try {
    const picked = (await call<string | null>("sol_pick_archive")) ?? null;
    if (picked) archivePath.value = picked;
  } catch {
    /* 忽略 */
  }
}

function extract() {
  if (!archivePath.value) return;
  void call("sol_extract", { archive: archivePath.value, dir: installDir.value || null }).catch(() => {});
}

async function openDir() {
  try {
    const opened = (await call<string>("sol_open_dir", { dir: installDir.value || null })) || "";
    if (opened) showToast(t("sol.opened", { path: opened }));
  } catch { /* 忽略 */ }
}

async function pickDir() {
  try {
    const picked = (await call<string | null>("sol_pick_dir", { current: installDir.value || null })) ?? null;
    if (picked) installDir.value = picked; // 双向回显
  } catch { /* 忽略 */ }
}
</script>

<template>
  <section :aria-label="t('sol.sectionAria')" class="settings" id="sol-section">
    <p class="sol-desc">{{ t("sol.intro") }}</p>

    <div class="setting" id="sol-state-row">
      <span class="setting-text">
        <strong>{{ t("sol.runState") }}</strong>
        <small class="setting-state" :class="{ 'setting-state-error': stateError }" id="sol-state-text" :title="stateDetail || undefined">{{ stateText }}</small>
      </span>
      <span class="remote-inline">
        <button type="button" id="sol-refresh" class="secondary" @click="refresh">{{ t("sol.refreshStatus") }}</button>
        <button type="button" id="sol-status-privileged" class="secondary" :hidden="!privilegedNeeded" :title="t('sol.authStatusTitle')" @click="readPrivilegedStatus">{{ t("sol.authStatus") }}</button>
        <button type="button" id="sol-restart-service" class="secondary" @click="restartService">{{ t("sol.restartService") }}</button>
      </span>
    </div>

    <div class="setting" id="sol-http-enabled-row">
      <span class="setting-text">
        <strong>{{ t("sol.httpEnable") }}</strong>
        <small>{{ t("sol.httpEnableNote") }}</small>
        <small class="setting-state" id="sol-http-enabled-state">{{ httpEnabledState }}</small>
      </span>
      <input type="checkbox" id="sol-http-enabled" :checked="httpEnabled" @change="setHttpEnabled" />
    </div>

    <div class="setting" id="sol-http-listen-row">
      <span class="setting-text">
        <strong>{{ t("sol.listenAddr") }}</strong>
        <small>{{ t("sol.listenAddrNote") }}</small>
        <small class="setting-state mono" id="sol-http-listen-state">{{ listenAddr || t("sol.listenPh") }}</small>
      </span>
      <span class="remote-inline">
        <input id="sol-http-listen" v-model="listenAddr" spellcheck="false" type="text" placeholder="0.0.0.0:8080" style="width: 220px" />
        <button type="button" id="sol-http-listen-save" class="secondary" @click="saveListen">{{ t("common.save") }}</button>
      </span>
    </div>

    <div class="setting setting-block" id="sol-http-token-row">
      <span class="setting-text">
        <strong>{{ t("sol.tokenTitle") }}</strong>
        <small class="setting-state" id="sol-http-token-state">{{ t("sol.tokenNote") }}</small>
      </span>
      <div class="sol-token-line">
        <span class="sol-token-value mono" id="sol-http-token-current" :title="t('sol.tokenCurrentTitle')">{{ status?.token || "—" }}</span>
        <button type="button" id="sol-http-token-copy" class="secondary" @click="copyToken">{{ t("common.copy") }}</button>
      </div>
      <div class="sol-token-line">
        <input id="sol-http-token" v-model="newToken" spellcheck="false" type="text" :placeholder="t('sol.newTokenPh')" style="flex: 1 1 220px" />
        <button type="button" id="sol-http-token-generate" class="secondary" @click="generateToken">{{ t("sol.generate") }}</button>
        <button type="button" id="sol-http-token-save" class="secondary" @click="saveToken">{{ t("settings.apply") }}</button>
      </div>
    </div>

    <!-- 动作区 -->
    <div class="actions">
      <button type="button" id="sol-one-click" class="primary-sol" @click="oneClick">{{ t("sol.oneClick") }}</button>
      <button type="button" id="sol-one-click-terminal" class="secondary" @click="manualInstall">{{ t("sol.manualInstall") }}</button>
    </div>

    <small class="sol-hint" id="sol-install-hint">
      ${{ t("sol.installDoc1") }}<br />
      ${{ t("sol.installDoc2") }}<br />
      ${{ t("sol.installDoc3") }}
    </small>

    <!-- 安装包 -->
    <div class="setting-block sol-pkg-block">
      <span class="setting-text">
        <strong>{{ t("sol.pkg") }}</strong>
        <small class="setting-state" id="sol-pkg-state">{{
          downloading
            ? t("sol.downloadingPct", { n: progress })
            : localPkg?.archive
              ? t("sol.downloaded", { size: fmtSize(localPkg.size), md5: localPkg.verified ? t("sol.md5Ok") : t("sol.md5Unchecked"), extracted: localPkg.extracted ? t("sol.extractedTag") : "" })
              : t("sol.notDownloaded")
        }}</small>
      </span>

      <div v-if="downloading" id="sol-progress" class="sol-progress">
        <div class="sol-progress-track"><div id="sol-progress-fill" class="sol-progress-fill" :style="{ width: progress + '%' }" /></div>
        <small id="sol-progress-text" class="sol-progress-text">{{ progress }}%</small>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>{{ t("sol.installDir") }}</strong>
          <small class="setting-state" id="sol-dir-state">{{ installDir || "—" }}</small>
        </span>
        <span class="remote-inline">
          <input id="sol-dir-input" v-model="installDir" spellcheck="false" type="text" :placeholder="installDirPlaceholder || t('sol.installDirPh')" style="width: 280px" />
          <button type="button" id="sol-pick-dir" class="secondary" @click="pickDir">{{ t("sol.pick") }}</button>
          <button type="button" id="sol-open-dir-input" class="secondary" :title="t('sol.openDirTitle')" @click="openDir">{{ t("sol.open") }}</button>
        </span>
      </div>

      <span class="remote-inline">
        <select id="sol-mirror" v-model="mirror" :title="t('sol.mirrorTitle')" @change="saveMirror">
          <option v-for="preset in MIRROR_PRESETS" :key="preset.id" :value="preset.id">{{ t(preset.label) }}</option>
        </select>
        <input
          v-if="mirror === 'custom'"
          id="sol-mirror-custom"
          v-model="mirrorCustom"
          spellcheck="false"
          type="text"
          :placeholder="t('sol.mirrorPh')"
          style="width: 280px"
          @change="saveMirror"
        />
        <button type="button" id="sol-download" class="secondary" :disabled="downloading" @click="download(false)">{{ t("about.download") }}</button>
        <button v-if="downloading" type="button" id="sol-download-cancel" class="secondary" @click="cancelDownload">{{ t("sol.cancelDownload") }}</button>
        <button type="button" id="sol-redownload" class="secondary" :disabled="downloading" :title="t('sol.redownloadTitle')" @click="redownload">{{ t("sol.redownload") }}</button>
        <button
          type="button"
          id="sol-delete-package"
          class="secondary"
          :disabled="downloading || !localPkg?.archive"
          :title="deleteArmed ? t('sol.deleteTitleArmed') : t('sol.deleteTitle')"
          @click="deletePackage"
        >{{ deleteArmed ? t("sol.deleteArmedConfirm") : t("sol.deletePkg") }}</button>
        <button type="button" id="sol-pick-archive" class="secondary" @click="pickArchive">{{ t("sol.pickArchive") }}</button>
        <button type="button" id="sol-extract" class="secondary" disabled :title="t('sol.extractTitle')" @click="extract">
          {{ t("sol.extract") }}
        </button>
      </span>

      <small class="sol-hint" id="sol-pkg-url" :title="t('sol.dlAddrTitle')">
        {{ t("sol.dlAddrLabel") }}<code style="word-break: break-all">{{ downloadUrl || t("sol.resolving") }}</code>
      </small>
      <small class="sol-hint" id="sol-pkg-hint">
        {{ t("sol.pkgHint", { t: "…" }) }}
      </small>
      <small class="sol-hint" id="sol-project-line">
        {{ t("sol.project") }}
        <a href="#" id="sol-project-repo" :title="t('sol.repoTitle')" @click.prevent="openProject('repo')">lidiaoo/sol</a>
        ·
        <a href="#" id="sol-project-releases" :title="t('sol.releasesTitle')" @click.prevent="openProject('releases')">Releases</a>
        ·
        <span id="sol-project-version">{{ projectVersion || status?.version || "…" }}</span>
      </small>
    </div>

    <!-- 配置文件编辑（旧版 .setting-block 里的 sol-config-*） -->
    <div class="setting-block">
      <span class="setting-text">
        <strong>{{ t("sol.configFile") }}</strong>
        <small class="setting-state" id="sol-config-path">{{ configStatus }}</small>
      </span>
      <textarea
        id="sol-config-text"
        v-model="configText"
        class="sol-config-editor"
        spellcheck="false"
        rows="16"
        :placeholder="t('sol.configReadingPh')"
      />
      <span class="remote-inline">
        <button type="button" id="sol-config-refresh" class="secondary" @click="refreshConfig">{{ t("sol.refreshConfig") }}</button>
        <button type="button" id="sol-config-save" class="primary-sol" @click="saveConfig">{{ t("sol.saveConfig") }}</button>
        <button type="button" id="sol-config-reload" class="secondary" @click="refreshConfig">{{ t("sol.discardChanges") }}</button>
      </span>
      <small class="sol-hint">
        {{ t("sol.configDoc") }}
      </small>
    </div>

    <!-- 执行日志（旧版 .sol-log-block） -->
    <div class="sol-log-block">
      <span class="setting-text">
        <strong>{{ t("sol.execLog") }}</strong>
        <small class="setting-state" id="sol-log-count">{{ t("sol.logCount", { n: logs.length }) }}</small>
      </span>
      <pre class="sol-log" id="sol-log">{{ logs.join("\n") || t("sol.noLog") }}</pre>
      <button type="button" id="sol-log-clear" class="secondary" @click="clearLogs">{{ t("sol.clear") }}</button>
    </div>

    <p v-if="errorText" class="sol-hint" style="color: rgb(var(--bad))">{{ errorText }}</p>
  </section>
</template>
