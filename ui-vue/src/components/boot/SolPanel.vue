<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
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
const configStatus = ref("读取中…");
async function refreshConfig() {
  configStatus.value = "读取中…";
  try {
    const result = await call<{ path?: string; content?: string }>("sol_config_read", { path: null });
    configPath.value = result?.path ?? "";
    configText.value = result?.content ?? "";
    configStatus.value = configPath.value || "—";
  } catch {
    configPath.value = "";
    configText.value = "";
    configStatus.value = "未找到配置文件";
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
    logs.value.push(`${nowTime()} 已删除安装包${localPkg.value.archive ? "" : "（含 md5 旁车）"}`);
  } catch (error) {
    logs.value.push(`${nowTime()} 删除失败：${String(error)}`);
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
  if (loading.value) return "正在读取…";
  const s = status.value;
  if (errorText.value) return ;
  if (!s) return "—";
  const parts: string[] = [];
  parts.push(s.installed ? (s.version || "已安装") : "未安装");
  if (s.serviceLoaded) parts.push(s.serviceActive ? "服务运行中" : "服务已停止");
  else if (s.installed) parts.push("服务未注册");
  if (!s.serviceActive && s.processRunning) parts.push("进程在跑（非服务方式）");
  if (s.statusNeedsPrivilege) parts.push("状态需授权查看");
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
      httpEnabledState.value = http.enabled ? "已开启（sol 控制面在跑）" : "已关闭";
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
  // 旧版：sol_project_info 回填项目版本行
  try {
    const info = (await call<{ version?: string; latest?: string }>("sol_project_info")) ?? {};
    projectVersion.value = info.version || info.latest || "";
  } catch { /* 忽略 */ }
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
    const message = (await call<string>("sol_http_set", { path: null, option: "token", value: token })) || "新 token 已写入配置";
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
  { id: "off", label: "不走加速（直连 GitHub）" },
  { id: "ghfast.top", label: "ghfast.top 加速" },
  { id: "gh-proxy.com", label: "gh-proxy.com 加速" },
  { id: "ghproxy.net", label: "ghproxy.net 加速" },
  { id: "mirror.ghproxy.com", label: "mirror.ghproxy.com 加速" },
  { id: "custom", label: "自定义…" },
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
    logs.value.push(`${nowTime()} 自定义加速前缀为空：请填一个 http(s):// 开头的前缀，或选「不走加速」`);
    return;
  }
  downloading.value = true;
  progress.value = 0;
  // 事件只是进度来源，**收尾必须自己兜底**：复用路径秒回、spawn 失败等都可能
  // 不带 done 事件（旧版吞错误后永远「下载中 0%」）。失败原因从 catch 里透出。
  try {
    await call<string>("sol_install_download", {
      dir: null,
      mirror: mirrorValue,
      force: force || undefined,
    });
    progress.value = 100;
  } catch (error) {
    const text = String(error);
    logs.value.push(`${nowTime()} ${cancelRequested.value ? "下载已取消" : `下载失败：${text}`}`);
  } finally {
    setTimeout(() => { downloading.value = false; }, 300);
    void refreshLocalPkg();
  }
}

/** 「取消下载」：kill 下载子进程，半截包由后端清理 */
async function cancelDownload(): Promise<void> {
  if (!downloading.value) return;
  cancelRequested.value = true;
  try {
    await call<boolean>("sol_download_cancel");
  } catch (error) {
    logs.value.push(`${nowTime()} 取消失败：${String(error)}`);
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
    if (opened) showToast(`已打开 ${opened}`);
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
  <section aria-label="SOL 远程唤醒" class="settings" id="sol-section">
    <p class="sol-desc">
      sol 监听本机网卡的 Wake-on-LAN 魔法包：手机 / 别的电脑发一个包，这台机器就按规则执行关机、重启、睡眠等动作（本机已配端口 10=睡眠、11=关机、12=重启）。
    </p>

    <div class="setting" id="sol-state-row">
      <span class="setting-text">
        <strong>运行状态</strong>
        <small class="setting-state" :class="{ 'setting-state-error': stateError }" id="sol-state-text" :title="stateDetail || undefined">{{ stateText }}</small>
      </span>
      <span class="remote-inline">
        <button type="button" id="sol-refresh" class="secondary" @click="refresh">刷新状态</button>
        <button type="button" id="sol-status-privileged" class="secondary" :hidden="!privilegedNeeded" title="服务状态被系统权限挡住时才出现" @click="readPrivilegedStatus">授权查看状态</button>
        <button type="button" id="sol-restart-service" class="secondary" @click="restartService">重启服务</button>
      </span>
    </div>

    <div class="setting" id="sol-http-enabled-row">
      <span class="setting-text">
        <strong>通过 HTTP 启用</strong>
        <small>允许手机 / 浏览器经本机 HTTP 服务发起 SOL 唤醒</small>
        <small class="setting-state" id="sol-http-enabled-state">{{ httpEnabledState }}</small>
      </span>
      <input type="checkbox" id="sol-http-enabled" :checked="httpEnabled" @change="setHttpEnabled" />
    </div>

    <div class="setting" id="sol-http-listen-row">
      <span class="setting-text">
        <strong>监听地址</strong>
        <small>SOL 服务监听的地址与端口</small>
        <small class="setting-state mono" id="sol-http-listen-state">{{ listenAddr || "host:port，如 127.0.0.1:8080；改完自动热重载" }}</small>
      </span>
      <span class="remote-inline">
        <input id="sol-http-listen" v-model="listenAddr" spellcheck="false" type="text" placeholder="0.0.0.0:8080" style="width: 220px" />
        <button type="button" id="sol-http-listen-save" class="secondary" @click="saveListen">保存</button>
      </span>
    </div>

    <div class="setting setting-block" id="sol-http-token-row">
      <span class="setting-text">
        <strong>访问令牌</strong>
        <small class="setting-state" id="sol-http-token-state">手机 / 浏览器访问 SOL 接口时要用它</small>
      </span>
      <div class="sol-token-line">
        <span class="sol-token-value mono" id="sol-http-token-current" title="当前配置里的 token（只读），点右边「复制」拷贝">{{ status?.token || "—" }}</span>
        <button type="button" id="sol-http-token-copy" class="secondary" @click="copyToken">复制</button>
      </div>
      <div class="sol-token-line">
        <input id="sol-http-token" v-model="newToken" spellcheck="false" type="text" placeholder="新 token（≥8 位），留空不改动" style="flex: 1 1 220px" />
        <button type="button" id="sol-http-token-generate" class="secondary" @click="generateToken">生成</button>
        <button type="button" id="sol-http-token-save" class="secondary" @click="saveToken">应用</button>
      </div>
    </div>

    <!-- 动作区 -->
    <div class="actions">
      <button type="button" id="sol-one-click" class="primary-sol" @click="oneClick">一键安装（静默）</button>
      <button type="button" id="sol-one-click-terminal" class="secondary" @click="manualInstall">手动安装（调终端）</button>
    </div>

    <small class="sol-hint" id="sol-install-hint">
      安装 = 下载 → <b>清空安装目录</b> → 解压到安装目录 → 官方 install.sh（静默 = 无人值守自动确认；手动 = 呼出终端由你回答脚本问句）。<b>解压前会清空安装目录，但保留 sol.yaml 配置文件</b>（端口、token 等设置不丢）；包内容直接铺在安装目录下，不保留 sol-v* 子目录。配置 sol.yaml 就在安装目录里；卸载保留它以便回滚。<br />
      已经解压好（比如用「选择安装包…」自己挑的包）时，「手动安装（调终端）」直接在终端里跑官方脚本，不再重新下载。<br />
      落点：三端全部在用户目录 —— Linux/macOS <code>~/sol</code>（二进制 <code>bin/sol</code>）；Windows <code>%USERPROFILE%\sol</code>（二进制 <code>sol.exe</code> 同目录）。
    </small>

    <!-- 安装包 -->
    <div class="setting-block sol-pkg-block">
      <span class="setting-text">
        <strong>安装包</strong>
        <small class="setting-state" id="sol-pkg-state">{{
          downloading
            ? `下载中 ${progress}%`
            : localPkg?.archive
              ? `已下载 · ${fmtSize(localPkg.size)}${localPkg.verified ? " · md5 一致" : " · md5 未校验"}${localPkg.extracted ? " · 已解压" : ""}`
              : "未下载"
        }}</small>
      </span>

      <div v-if="downloading" id="sol-progress" class="sol-progress">
        <div class="sol-progress-track"><div id="sol-progress-fill" class="sol-progress-fill" :style="{ width: progress + '%' }" /></div>
        <small id="sol-progress-text" class="sol-progress-text">{{ progress }}%</small>
      </div>

      <div class="setting">
        <span class="setting-text">
          <strong>安装目录</strong>
          <small class="setting-state" id="sol-dir-state">{{ installDir || "—" }}</small>
        </span>
        <span class="remote-inline">
          <input id="sol-dir-input" v-model="installDir" spellcheck="false" type="text" :placeholder="installDirPlaceholder || '~/sol（默认）'" style="width: 280px" />
          <button type="button" id="sol-pick-dir" class="secondary" @click="pickDir">选择…</button>
          <button type="button" id="sol-open-dir-input" class="secondary" title="打开当前填写的目录" @click="openDir">打开</button>
        </span>
      </div>

      <span class="remote-inline">
        <select id="sol-mirror" v-model="mirror" title="GitHub 下载加速（默认直连）" @change="saveMirror">
          <option v-for="preset in MIRROR_PRESETS" :key="preset.id" :value="preset.id">{{ preset.label }}</option>
        </select>
        <input
          v-if="mirror === 'custom'"
          id="sol-mirror-custom"
          v-model="mirrorCustom"
          spellcheck="false"
          type="text"
          placeholder="自定义前缀，如 https://ghfast.top/（拼在 GitHub URL 前）"
          style="width: 280px"
          @change="saveMirror"
        />
        <button type="button" id="sol-download" class="secondary" :disabled="downloading" @click="download(false)">下载</button>
        <button v-if="downloading" type="button" id="sol-download-cancel" class="secondary" @click="cancelDownload">取消下载</button>
        <button type="button" id="sol-redownload" class="secondary" :disabled="downloading" title="跳过「已有包复用」，删旧包重新下" @click="redownload">重新下载</button>
        <button
          type="button"
          id="sol-delete-package"
          class="secondary"
          :disabled="downloading || !localPkg?.archive"
          :title="deleteArmed ? '再点一次确认删除（含 md5 旁车，不动已解压内容）' : '删除已下载的安装包'"
          @click="deletePackage"
        >{{ deleteArmed ? "确认删除？" : "删除安装包" }}</button>
        <button type="button" id="sol-pick-archive" class="secondary" @click="pickArchive">选择安装包…</button>
        <button type="button" id="sol-extract" class="secondary" disabled title="解压前会先清空安装目录（保留 sol.yaml 配置文件），再把包内容直接铺到安装目录，不再保留 sol-v* 子目录" @click="extract">
          解压到安装目录
        </button>
      </span>

      <small class="sol-hint" id="sol-pkg-hint">
        从 GitHub release 下载当前平台的 sol-<span id="sol-pkg-target">…</span> 包；也可以点「选择安装包…」从文件管理器挑一个已有包（.tar.gz / .zip），解压完回到上面点「手动安装（调终端）」即可安装。
      </small>
      <small class="sol-hint" id="sol-project-line">
        项目：
        <a href="#" id="sol-project-repo" title="打开 GitHub 仓库" @click.prevent="openProject('repo')">lidiaoo/sol</a>
        ·
        <a href="#" id="sol-project-releases" title="打开 Releases 页面" @click.prevent="openProject('releases')">Releases</a>
        ·
        <span id="sol-project-version">{{ projectVersion || status?.version || "…" }}</span>
      </small>
    </div>

    <!-- 配置文件编辑（旧版 .setting-block 里的 sol-config-*） -->
    <div class="setting-block">
      <span class="setting-text">
        <strong>配置文件</strong>
        <small class="setting-state" id="sol-config-path">{{ configStatus }}</small>
      </span>
      <textarea
        id="sol-config-text"
        v-model="configText"
        class="sol-config-editor"
        spellcheck="false"
        rows="16"
        placeholder="正在读取配置…"
      />
      <span class="remote-inline">
        <button type="button" id="sol-config-refresh" class="secondary" @click="refreshConfig">刷新配置</button>
        <button type="button" id="sol-config-save" class="primary-sol" @click="saveConfig">保存配置</button>
        <button type="button" id="sol-config-reload" class="secondary" @click="refreshConfig">放弃修改</button>
      </span>
      <small class="sol-hint">
        保存后 sol 自动热重载（改坏了也不会顶掉正在跑的那份）；原文件自动备份为 *.bak-anyboot。配置在安装时会自动放宽为 0644（本机所有登录用户可读），所以读取不再需要密码；配置里有控制面 token，请勿在不信任的多用户机器上使用。
      </small>
    </div>

    <!-- 执行日志（旧版 .sol-log-block） -->
    <div class="sol-log-block">
      <span class="setting-text">
        <strong>执行日志</strong>
        <small class="setting-state" id="sol-log-count">{{ logs.length }} 行</small>
      </span>
      <pre class="sol-log" id="sol-log">{{ logs.join("\n") || "（还没有日志）" }}</pre>
      <button type="button" id="sol-log-clear" class="secondary" @click="clearLogs">清空</button>
    </div>

    <p v-if="errorText" class="sol-hint" style="color: rgb(var(--bad))">{{ errorText }}</p>
  </section>
</template>
