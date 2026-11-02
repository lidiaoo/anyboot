<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
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

interface SolStatus {
  installed?: boolean;
  running?: boolean;
  port?: number;
  token?: string;
  installDir?: string;
  version?: string;
  [key: string]: unknown;
}

const status = ref<SolStatus | null>(null);
const loading = ref(true);
const errorText = ref("");

const listenAddr = ref("");
const newToken = ref("");
const installDir = ref("");
const progress = ref(0);
const logs = ref<string[]>([]);
const configText = ref("");

async function refreshConfig() {
  try {
    configText.value = (await call<string>("sol_config_read", { path: null })) ?? "";
  } catch {
    configText.value = "";
  }
}

function saveConfig() {
  void call("sol_config_write", { path: "", content: configText.value }).catch(() => {});
}

function clearLogs() {
  logs.value = [];
}
const downloading = ref(false);
/** 服务状态被权限挡住时显示「授权查看状态」（旧版 #sol-status-privileged） */
const privilegedNeeded = ref(false);
async function readPrivilegedStatus() {
  try {
    await call("sol_status_privileged");
    privilegedNeeded.value = false;
  } catch {
    /* 忽略 */
  }
}

const stateText = computed(() => {
  if (loading.value) return "正在读取…";
  if (!status.value?.installed) return "未安装";
  return status.value.running ? "运行中" : "已停止";
});

async function refresh() {
  loading.value = true;
  errorText.value = "";
  try {
    status.value = await call<SolStatus>("sol_status");
    listenAddr.value = status.value?.port ? `0.0.0.0:${status.value.port}` : "";
    installDir.value = status.value?.installDir ?? "";
  } catch (error) {
    errorText.value = String(error);
    status.value = null;
  } finally {
    loading.value = false;
  }
}

onMounted(refresh);

function setHttpEnabled() {
  void call("sol_http_set", { path: null, option: "enabled", value: String(!status.value?.running) }).catch(() => {});
}

function saveListen() {
  void call("sol_http_set", { path: null, option: "listen", value: listenAddr.value.trim() }).catch(() => {});
}

async function copyToken() {
  try {
    await navigator.clipboard.writeText(status.value?.token ?? "");
  } catch {
    /* 剪贴板不可用 */
  }
}

function generateToken() {
  const bytes = new Uint8Array(24);
  crypto.getRandomValues(bytes);
  newToken.value = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
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

function download() {
  downloading.value = true;
  progress.value = 0;
  void call("sol_install_download", { dir: null }).catch(() => {});
}

function pickArchive() {
  void call("sol_project_open").catch(() => {});
}

function extract() {
  void call("sol_extract").catch(() => {});
}

function pickDir() {
  void call("pick_grub_dir").then((dir) => {
    if (dir) installDir.value = String(dir);
  });
}
</script>

<template>
  <section aria-label="SOL 远程唤醒" class="settings" id="sol-section">
    <p class="sol-desc">
      用 SOL（Serial-over-LAN）远程唤醒 / 控制服务器 —— 需要目标机 BMC 支持 SOL。
    </p>

    <div class="setting" id="sol-state-row">
      <span class="setting-text">
        <strong>服务状态</strong>
        <small>SOL 辅助服务是否就绪</small>
        <small class="setting-state" id="sol-state-text">{{ stateText }}</small>
      </span>
      <span class="remote-inline">
        <button type="button" id="sol-refresh" class="secondary" @click="refresh">刷新</button>
        <button type="button" class="secondary" @click="call('sol_status_privileged')">检查权限</button>
        <button type="button" id="sol-status-privileged" class="secondary" :hidden="!privilegedNeeded" title="服务状态被系统权限挡住时才出现" @click="readPrivilegedStatus">授权查看状态</button>
        <button type="button" id="sol-restart-service" class="secondary" @click="call('sol_project_open')">重启服务</button>
      </span>
    </div>

    <div class="setting" id="sol-http-enabled-row">
      <span class="setting-text">
        <strong>通过 HTTP 启用</strong>
        <small>允许手机 / 浏览器经本机 HTTP 服务发起 SOL 唤醒</small>
        <small class="setting-state" id="sol-http-enabled-state">{{ status?.running ? "已启用" : "未启用" }}</small>
      </span>
      <input type="checkbox" id="sol-http-enabled" :checked="Boolean(status?.running)" @change="setHttpEnabled" />
    </div>

    <div class="setting" id="sol-http-listen-row">
      <span class="setting-text">
        <strong>监听地址</strong>
        <small>SOL 服务监听的地址与端口</small>
        <small class="setting-state mono" id="sol-http-listen-state">{{ listenAddr || "—" }}</small>
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
        <small class="setting-state" id="sol-pkg-state">{{ downloading ? `下载中 ${progress}%` : "未下载" }}</small>
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
          <input id="sol-dir-input" v-model="installDir" spellcheck="false" type="text" placeholder="~/sol（默认）" style="width: 280px" />
          <button type="button" id="sol-pick-dir" class="secondary" @click="pickDir">选择…</button>
          <button type="button" id="sol-open-dir-input" class="secondary" title="打开当前填写的目录" @click="call('sol_open_dir', {})">打开</button>
        </span>
      </div>

      <span class="remote-inline">
        <button type="button" id="sol-download" class="secondary" @click="download">下载</button>
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
        <a href="#" id="sol-project-repo" title="打开 GitHub 仓库">lidiaoo/sol</a>
        ·
        <a href="#" id="sol-project-releases" title="打开 Releases 页面">Releases</a>
        ·
        <span id="sol-project-version">{{ status?.version || "…" }}</span>
      </small>
    </div>

    <!-- 配置文件编辑（旧版 .setting-block 里的 sol-config-*） -->
    <div class="setting-block">
      <span class="setting-text">
        <strong>配置文件</strong>
        <small class="setting-state" id="sol-config-path">{{ status?.configPath || "—" }}</small>
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
      <pre class="sol-log" id="sol-log">{{ logs.join("\n") || "（暂无输出）" }}</pre>
      <button type="button" id="sol-log-clear" class="secondary" @click="clearLogs">清空</button>
    </div>

    <p v-if="errorText" class="sol-hint" style="color: rgb(var(--bad))">{{ errorText }}</p>
  </section>
</template>
