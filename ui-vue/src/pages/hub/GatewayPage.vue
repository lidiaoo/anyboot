<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { call } from "@/api/tauri";
import PairDialog from "@/components/hub/PairDialog.vue";
import LogDialog from "@/components/hub/LogDialog.vue";

/**
 * 远程接入页（原 `ui/hub.html` 的 `#page-gateway`，2792~3355 行）。
 *
 * ## 1:1 还原要点
 *
 * 旧版结构（**逐字照搬**，样式全在 hub-inline.css）：
 * ```
 * .page-head  > div(h2 + .sub) + .gw-ar（自动刷新三合一：数字框 + 下拉 + 暂停）
 * .gw-tabs > 4 × button.gw-tab[data-gwtab=cf|ts|tsn|pair]（带 .gw-tab-dot 状态点）
 * .gw-tabpane[data-gwpane]
 *   .gw-note
 *   .gw-card > .gw-card-head(h3 + .segmented + btn×N + .gw-sw-group>.switch)
 *              + .gw-status + .gw-err + 各配置区
 * ```
 *
 * 命令：`gateway_*` 共 36 个（这里按分页覆盖主流程）。
 */

interface GatewayConfig {
  mode: string;
  namedUrl: string;
  namedTokenSet: boolean;
  autostart: boolean;
  binary: { installed: boolean; version: string; path: string; source: string };
  tunnel: { running: boolean; mode: string; url: string; state: string; error: string };
}

interface TailscaleStatus {
  installed: boolean;
  running: boolean;
  ip: string;
  version: string;
}

interface PairRow {
  id: string;
  label: string;
  url: string;
  revoked: boolean;
}

type TabKey = "cf" | "ts" | "tsn" | "pair";

const activeTab = ref<TabKey>("cf");
const config = ref<GatewayConfig | null>(null);
const tsStatus = ref<TailscaleStatus | null>(null);
const pairs = ref<PairRow[]>([]);
const busy = ref("");
/** 手动指定 cloudflared 路径（旧版 #gwManualRow 的开合） */
const manualOpen = ref(false);
const manualPath = ref("");
/** Tailscale 授权密钥输入（旧版 #tsKey） */
const tsKey = ref("");
/** Tailscale 自启（旧版 #tsAutostart） */
const tsAutostart = ref(false);

function toggleTsAutostart() {
  const next = !tsAutostart.value;
  tsAutostart.value = next;
  void call("gateway_set_autostart", { which: "tailscale", on: next }).catch(() => {});
}

/** Tailscale 日志弹层（旧版 #tsLogOverlay） */
const tsLogOpen = ref(false);
const tsLogs = ref<string[]>([]);

/** 内置引擎日志（旧版 #tsnLogOverlay） */
const tsnLogOpen = ref(false);
const tsnLogs = ref<string[]>([]);

async function openTsnLog() {
  try {
    tsnLogs.value = (await call<string[]>("gateway_tailscale_logs")) ?? [];
  } catch {
    tsnLogs.value = [];
  }
  tsnLogOpen.value = true;
}

function clearTsLogs() {
  void call("gateway_tailscale_logs_clear").catch(() => {});
  tsLogs.value = [];
}

function clearTsnLogs() {
  void call("gateway_tailscale_logs_clear").catch(() => {});
  tsnLogs.value = [];
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    message.value = "已复制到剪贴板";
  } catch {
    /* 剪贴板不可用 */
  }
}

async function openTsLog() {
  try {
    tsLogs.value = (await call<string[]>("gateway_tailscale_logs")) ?? [];
  } catch {
    tsLogs.value = [];
  }
  tsLogOpen.value = true;
}

function tsDownload() {
  void run("下载 Tailscale", () => call("gateway_tailscale_download"), "开始下载…");
}

function tsOpenManual() {
  void call("gateway_tailscale_launch_gui").catch(() => {});
}

function tsSaveKey() {
  const key = tsKey.value.trim();
  void run("保存密钥", () => call("gateway_tailscale_set_authkey", { key }), "密钥已保存");
}

function tailscaleService() {
  void run("切换服务", () => call("gateway_tailscale_service"), "已切换");
}

function tsUninstall() {
  if (!window.confirm("确定卸载 Tailscale？")) return;
  void run("卸载 Tailscale", () => call("gateway_tailscale_uninstall"), "已卸载");
}

async function copyTsAddr() {
  const ip = tsStatus.value?.ip;
  if (!ip) return;
  try {
    await navigator.clipboard.writeText(ip);
    message.value = "地址已复制";
  } catch {
    /* 剪贴板不可用 */
  }
}

/** 配对码弹层（旧版 #gwPairOverlay） */
const pairOpen = ref(false);

/** 日志弹层（旧版 #gwLogOverlay） */
const logOpen = ref(false);
const logs = ref<string[]>([]);

async function refreshLogs() {
  try {
    logs.value = (await call<string[]>("gateway_tunnel_logs")) ?? [];
  } catch {
    logs.value = [];
  }
}

function clearLogs() {
  void call("gateway_tunnel_logs_clear").catch(() => {});
  logs.value = [];
}
const message = ref("");
const errorText = ref("");

/** 自动刷新（旧版 `.gw-ar` 三合一控件） */
const autoSec = ref(3);
const autoRunning = ref(true);
let autoTimer: number | null = null;

const TABS: { key: TabKey; label: string; dot?: boolean }[] = [
  { key: "cf", label: "Cloudflare 通道", dot: true },
  { key: "ts", label: "Tailscale", dot: true },
  { key: "tsn", label: "Tailscale (内置)", dot: true },
  { key: "pair", label: "配对管理" },
];

const cfUrl = computed(() => config.value?.tunnel?.url ?? "");
const cfRunning = computed(() => Boolean(config.value?.tunnel?.running));
const cfInstalled = computed(() => Boolean(config.value?.binary?.installed));
/** 是否有上次遗留的孤儿通道（旧版 #gwOrphanBox 的显示条件） */
const orphanRunning = computed(
  () => !cfRunning.value && Boolean(config.value?.tunnel?.url) && config.value?.tunnel?.state === "orphan",
);

watch(logOpen, (open) => {
  if (open) void refreshLogs();
});

async function refreshAll() {
  try {
    config.value = await call<GatewayConfig>("gateway_config");
  } catch (error) {
    errorText.value = String(error);
  }
  try {
    tsStatus.value = await call<TailscaleStatus>("gateway_tailscale_status");
  } catch {
    tsStatus.value = null;
  }
  try {
    pairs.value = (await call<PairRow[]>("gateway_pair_list")) ?? [];
  } catch {
    pairs.value = [];
  }
}

function setupAuto() {
  if (autoTimer !== null) window.clearInterval(autoTimer);
  autoTimer = null;
  if (!autoRunning.value || autoSec.value < 1) return;
  autoTimer = window.setInterval(() => void refreshAll(), autoSec.value * 1000);
}

async function run(label: string, action: () => Promise<unknown>, done: string) {
  if (busy.value) return;
  busy.value = label;
  message.value = "";
  errorText.value = "";
  try {
    await action();
    message.value = done;
    await refreshAll();
  } catch (error) {
    errorText.value = String(error);
  } finally {
    busy.value = "";
  }
}

onMounted(async () => {
  await refreshAll();
  setupAuto();
});

onUnmounted(() => {
  if (autoTimer !== null) window.clearInterval(autoTimer);
});

// ---- Cloudflare ----
function toggleCf() {
  const next = !cfRunning.value;
  void run(
    next ? "开启通道" : "关闭通道",
    () => call(next ? "gateway_tunnel_start" : "gateway_tunnel_stop"),
    next ? "通道已开启" : "通道已关闭",
  );
}

function downloadBinary() {
  void run("下载 cloudflared", () => call("gateway_binary_download"), "开始下载…");
}

function setManualBinary() {
  const path = manualPath.value.trim();
  if (!path) return;
  void run(
    "指定路径",
    () => call("gateway_binary_set_manual", { path }),
    "已指定本地 cloudflared",
  ).then(() => {
    manualOpen.value = false;
  });
}

function killOrphan() {
  void run("清理残留", () => call("gateway_tunnel_kill_orphan"), "已清理残留通道");
}

// ---- Tailscale ----
function toggleTailscale() {
  void run("切换 Tailscale", () => call("gateway_tailscale_toggle"), "已切换");
}

// ---- 配对 ----
function revokePair(id: string) {
  void run("收回", () => call("gateway_pair_revoke", { id }), "已收回");
}

function deletePair(id: string) {
  void run("删除", () => call("gateway_pair_delete", { id }), "已删除");
}
</script>

<template>
  <div class="page-head" style="display: flex; align-items: center; gap: 10px; flex-wrap: wrap">
    <div>
      <h2>远程接入</h2>
      <span class="sub"
        >把本机的 AnyBoot 挂上公网隧道或虚拟局域网 —— 配对后的手机 / 其他电脑出门也能访问这台电脑</span
      >
    </div>

    <!-- 自动刷新三合一（旧版 #gwAuto） -->
    <div class="gw-ar" title="自动刷新这一页的状态（单位：秒）">
      <span class="gw-ar-label">自动刷新</span>
      <div class="gw-ar-combo">
        <input
          v-model.number="autoSec"
          type="number"
          min="1"
          max="3600"
          step="1"
          inputmode="numeric"
          aria-label="自动刷新间隔，单位秒"
          @change="setupAuto"
        />
        <span class="gw-ar-unit">秒</span>
      </div>
      <button
        type="button"
        class="gw-ar-toggle"
        :aria-pressed="!autoRunning"
        @click="autoRunning = !autoRunning; setupAuto()"
      >
        <span>{{ autoRunning ? "暂停" : "继续" }}</span>
      </button>
    </div>
  </div>

  <!-- 分页标签（旧版 #gwTabs，4 个分页带状态点） -->
  <div class="gw-tabs" role="tablist" aria-label="远程接入分页">
    <button
      v-for="tab in TABS"
      :key="tab.key"
      type="button"
      class="gw-tab"
      role="tab"
      :aria-selected="activeTab === tab.key"
      @click="activeTab = tab.key"
    >
      <span
        v-if="tab.dot"
        class="gw-tab-dot"
        :data-st="
          tab.key === 'cf'
            ? cfRunning
              ? 'on'
              : 'off'
            : tab.key === 'ts'
              ? tsStatus?.running
                ? 'on'
                : 'off'
              : 'off'
        "
      />
      {{ tab.label }}
    </button>
  </div>

  <!-- ===== Cloudflare 分页 ===== -->
  <div v-show="activeTab === 'cf'" class="gw-tabpane">
    <div class="gw-note">
      快速隧道的地址在通道重启后会变化 —— 重启电脑 / 通道后，已配对的客户端要重新配对一次。
      要固定地址请改用「命名隧道」（需 Cloudflare 账号 + 托管域名），或用下方 Tailscale
      直连（装好即用、地址长期固定、不出公网）。
    </div>

    <div class="gw-card">
      <div class="gw-card-head">
        <h3>本机通道</h3>
        <div class="segmented" role="tablist" aria-label="通道模式">
          <button role="tab" :aria-selected="config?.mode !== 'named'">快速隧道</button>
          <button role="tab" :aria-selected="config?.mode === 'named'">命名隧道</button>
        </div>
        <button type="button" class="btn" @click="logOpen = true">日志</button>
        <button type="button" class="btn" @click="refreshAll">重新检测</button>
        <span class="gw-sw-group" title="开启 / 关闭 Cloudflare 通道">
          <span class="gw-sw-cap">通道</span>
          <label class="switch">
            <input type="checkbox" :checked="cfRunning" @change="toggleCf" />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
      </div>

      <div class="gw-status">
        <span class="pulse" :hidden="!cfRunning" />
        <span>{{ cfRunning ? "通道已开启" : "通道未开启" }}</span>
        <a v-if="cfUrl" :href="cfUrl" class="mono gw-url" target="_blank" rel="noopener">{{ cfUrl }}</a>
        <span style="flex: 1" />
        <span v-if="config" class="mono gw-meta">{{ config.tunnel?.state || "" }}</span>
      </div>

      <div v-if="config?.tunnel?.error" class="gw-err">{{ config.tunnel.error }}</div>

      <!-- 残留通道（旧版 #gwOrphanBox）：上次异常退出留下的孤儿 cloudflared -->
      <div v-if="orphanRunning" style="margin-top: 12px">
        <p class="field-hint" style="margin: 0 0 6px">
          检测到上次异常退出留下的残留通道 —— 「沿用」地址不变（客户端无需重新配对），
          「终止」后点上方「开启通道」会创建新地址。
        </p>
        <div class="gw-clients">
          <div class="gw-client">
            <span class="mono">{{ config?.tunnel?.url || "（无地址）" }}</span>
            <span style="flex: 1" />
            <button type="button" class="link-btn" @click="refreshAll">沿用</button>
            <button type="button" class="link-btn" :disabled="!!busy" @click="killOrphan">终止</button>
          </div>
        </div>
      </div>

      <label class="gw-auto">
        <input type="checkbox" :checked="config?.autostart" disabled />
        <span class="gw-auto-t">启动软件时自动开启通道</span>
        <span class="gw-auto-h">cloudflared 随 AnyBoot 一起拉起</span>
      </label>

      <div class="gw-binary">
        <span>
          {{
            cfInstalled
              ? `cloudflared ${config?.binary?.version || ""} 已就位`
              : "cloudflared 未就位 —— 下载（自动校验 SHA-256）或指定本地已有的可执行文件"
          }}
        </span>
        <span style="flex: 1" />
        <button v-if="!cfInstalled" type="button" class="btn" :disabled="!!busy" @click="downloadBinary">
          下载 cloudflared
        </button>
        <button v-if="!cfInstalled" type="button" class="btn" @click="manualOpen = !manualOpen">
          指定本地路径
        </button>
      </div>

      <!-- 指定本地路径（旧版 #gwManualRow） -->
      <div v-if="manualOpen" class="gw-binary">
        <input
          v-model="manualPath"
          type="text"
          class="mono"
          placeholder="cloudflared 的完整路径"
          spellcheck="false"
        />
        <button type="button" class="btn" @click="manualOpen = false">取消</button>
        <button
          type="button"
          class="btn primary"
          :disabled="!manualPath.trim() || !!busy"
          @click="setManualBinary"
        >
          确定
        </button>
      </div>
    </div>
  </div>

  <!-- ===== Tailscale 分页（旧版 hub.html:2958 起） ===== -->
  <div v-show="activeTab === 'ts'" class="gw-tabpane">
    <div class="gw-card">
      <div class="gw-card-head">
        <h3>Tailscale 直连</h3>
        <span v-if="tsStatus?.installed" class="tsf-badge">已安装{{ tsStatus?.ip ? ` · ${tsStatus.ip}` : "" }}</span>
        <span style="flex: 1" />
        <span v-if="tsStatus?.installed" class="gw-sw-group" title="连接 / 断开 Tailscale（登录级，等于 tailscale up / down）">
          <span class="gw-sw-cap">连接</span>
          <label class="switch">
            <input type="checkbox" :checked="tsStatus?.running" @change="toggleTailscale" />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
        <span v-if="tsStatus?.installed" class="gw-sw-group" title="启动 / 停止 tailscaled 系统服务">
          <span class="gw-sw-cap">服务</span>
          <label class="switch">
            <input type="checkbox" :checked="tsStatus?.running" @change="tailscaleService" />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
        <button v-if="tsStatus?.installed" type="button" class="btn" @click="tsUninstall">卸载</button>
        <button type="button" class="btn" @click="openTsLog">日志</button>
        <button type="button" class="btn" @click="refreshAll">重新检测</button>
      </div>

      <div class="gw-status">
        <span class="pulse" :hidden="!tsStatus?.running" />
        <span>{{ tsStatus?.running ? "已连接" : tsStatus?.installed ? "未连接" : "未安装 Tailscale" }}</span>
        <span style="flex: 1" />
        <span v-if="tsStatus?.version" class="mono gw-meta">{{ tsStatus.version }}</span>
      </div>

      <!-- 未安装：下载 / 官网 -->
      <div v-if="!tsStatus?.installed">
        <p class="field-hint" style="margin: 8px 0 0">
          在这一台电脑上安装 Tailscale，手机 / 其他电脑加入同一网络后就能用它固定的
          100.x 地址访问本机 —— 不出公网、地址长期不变。
        </p>
        <div class="gw-binary">
          <span>安装包未下载</span>
          <button type="button" class="btn" :disabled="!!busy" @click="tsDownload">下载 Tailscale</button>
          <button type="button" class="btn" @click="tsOpenManual">打开官网手动下载</button>
        </div>
        <div class="gw-progress" hidden>
          <div class="gw-progress-bar"><div /></div>
          <span class="mono" />
        </div>

        <!-- 自启（旧版 #tsAutostart） -->
        <label class="gw-auto">
          <input type="checkbox" :checked="tsAutostart" @change="toggleTsAutostart" />
          <span class="gw-auto-t">启动软件时自动连接 Tailscale</span>
          <span class="gw-auto-h">等价于自动点「连接」</span>
        </label>
      </div>

      <!-- 已安装未连接：授权密钥 / 登录 -->
      <template v-if="tsStatus?.installed && !tsStatus?.running">
        <div style="margin-top: 10px">
          <div style="display: flex; gap: 8px; align-items: center">
            <input
              v-model="tsKey"
              type="password"
              class="mono"
              style="flex: 1"
              placeholder="留空则走浏览器登录"
              autocomplete="off"
            />
            <button type="button" class="btn" :disabled="!!busy" @click="tsSaveKey">保存</button>
          </div>
          <p class="field-hint">
            在 Tailscale 管理控制台 → Settings → Keys 里签发一次性的设备授权密钥；填了它连接时免浏览器登录。
          </p>
        </div>
      </template>

      <!-- 已连接：地址 + 自检 -->
      <template v-if="tsStatus?.running">
        <div class="gw-status" style="margin-top: 8px">
          <span class="mono ts-addr">{{ tsStatus?.ip || "—" }}</span>
          <button type="button" class="link-btn" @click="copyTsAddr">复制</button>
          <button type="button" class="btn" @click="pairOpen = true">生成此地址的配对码</button>
        </div>
        <div class="gw-status" style="margin-top: 6px">
          <span
            style="display: inline-flex; width: 8px; height: 8px; border-radius: 50%; background: rgb(var(--fg-faint)); flex: none"
          />
          <span style="font-size: 12px; color: rgb(var(--fg-dim))">正在自检本机服务…</span>
        </div>
      </template>
    </div>
  </div>

  <!-- ===== Tailscale 内置分页 ===== -->
  <div v-show="activeTab === 'tsn'" class="gw-tabpane">
    <div class="gw-card">
      <div class="gw-card-head">
        <h3>Tailscale（内置）</h3>
        <span style="flex: 1" />
        <button type="button" class="btn" @click="openTsnLog">日志</button>
        <button type="button" class="btn" @click="refreshAll">重新检测</button>
      </div>
      <div class="gw-status">
        <span>内置 tsnet 模式 —— 不需要安装系统 Tailscale</span>
      </div>
    </div>
  </div>

  <!-- ===== 配对管理分页 ===== -->
  <div v-show="activeTab === 'pair'" class="gw-tabpane">
    <div class="gw-card">
      <div class="gw-card-head">
        <h3>配对管理</h3>
        <span style="flex: 1" />
        <button type="button" class="btn primary" @click="pairOpen = true">签发新配对码</button>
      </div>

      <div v-if="pairs.length" class="gw-clients">
        <div v-for="pair in pairs" :key="pair.id" class="gw-client">
          <span class="mono">{{ pair.label || pair.id }}</span>
          <span class="mono gw-meta">{{ pair.url || "—" }}</span>
          <span style="flex: 1" />
          <button type="button" class="link-btn" @click="revokePair(pair.id)">收回</button>
          <button type="button" class="link-btn" @click="deletePair(pair.id)">删除</button>
        </div>
      </div>
      <p v-else class="field-hint">还没有签发过配对码</p>
    </div>
  </div>

  <PairDialog :open="pairOpen" @close="pairOpen = false; refreshAll()" />

  <!-- 三个日志弹层（旧版 #gwLogOverlay / #tsLogOverlay / #tsnLogOverlay 同构） -->
  <LogDialog
    :open="logOpen"
    title="通道日志"
    :lines="logs"
    @copy="copyText"
    @clear="clearLogs"
    @close="logOpen = false"
  />
  <LogDialog
    :open="tsLogOpen"
    title="Tailscale 日志"
    :lines="tsLogs"
    @copy="copyText"
    @clear="clearTsLogs"
    @close="tsLogOpen = false"
  />
  <LogDialog
    :open="tsnLogOpen"
    title="内置引擎日志"
    :lines="tsnLogs"
    @copy="copyText"
    @clear="clearTsnLogs"
    @close="tsnLogOpen = false"
  />

  <p v-if="message" style="font-size: 12.5px; color: rgb(var(--ok))">{{ message }}</p>
  <p v-if="errorText" style="font-size: 12.5px; color: rgb(var(--bad))">{{ errorText }}</p>
</template>

<!--
  样式**不写在这里**：`.page-head` / `.gw-ar` / `.gw-tabs` / `.gw-tab` /
  `.gw-tabpane` / `.gw-note` / `.gw-card` / `.gw-card-head` / `.gw-sw-group` /
  `.switch` / `.gw-status` / `.gw-err` / `.gw-auto` / `.gw-binary` / `.gw-clients`
  全部由 `styles/vendor/hub-inline.css` 定义。
-->
