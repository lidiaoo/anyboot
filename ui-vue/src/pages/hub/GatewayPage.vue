<script setup lang="ts">
const toast = useToastStore();
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { call } from "@/api/tauri";
import PairDialog from "@/components/hub/PairDialog.vue";
import LogDialog from "@/components/hub/LogDialog.vue";
import { useToastStore } from "@/stores/toast";

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
  /** 出码时记录的通道类型：tunnel / ts / funnel / tsnet（旧版 pair_kind） */
  kind?: string;
  createdAt?: number;
  lastSeen?: number;
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

/** Tailscale 状态文案（旧版 tsStateText） */
const tsStateText = computed(() => {
  if (!tsStatus.value) return "正在检测 Tailscale…";
  if (!tsStatus.value.installed) return "未安装 Tailscale";
  return tsStatus.value.running ? "已连接" : "未连接";
});

/** Tailscale 安装包下载进度（旧版 #tsProgressRow） */
const tsDownloading = ref(false);
const tsProgress = ref(0);

/** Funnel 公网通道状态（旧版 #tsfBlock，4 种状态：off/starting/needs_authorize/ready/error） */
interface TsfStatus {
  state: string;
  url?: string;
  error?: string;
}
const tsfStatus = ref<TsfStatus | null>(null);
const tsfStateText = computed(() => {
  const s = tsfStatus.value?.state;
  if (s === "starting") return "正在开启 Funnel…";
  if (s === "needs_authorize") return "需要在 Tailscale 官方页授权";
  if (s === "ready") return "Funnel 已开启";
  if (s === "error") return "Funnel 出错";
  return "Funnel 未开启";
});

/** 打开 Funnel 官方授权页（旧版 #tsfAuthBtn） */
function tsFunnelAuth() {
  void call("gateway_tailscale_funnel_enable").catch((e) => toast.show("授权地址无效：" + e, "bad"));
}

/** 命名隧道配置（旧版 #gwNamedToken / #gwNamedUrl） */
const namedToken = ref("");
const namedUrl = ref("");

/** 自启开关（旧版 #gwAutostart） */
function toggleAutostart() {
  const next = !config.value?.autostart;
  void run(
    next ? "开启自启" : "关闭自启",
    () => call("gateway_set_autostart", { channel: "cloudflare", enabled: next }),
    next ? "已开启自启" : "已关闭自启",
  );
}
/** Tailscale 授权密钥输入（旧版 #tsKey） */
const tsKey = ref("");

/** 内置 tsnet 引擎（旧版 tsnet_* 命令族） */
interface TsnStatus {
  /** off / needs_auth / ready / error（旧版 tsn 的四个状态） */
  state?: string;
  installed?: boolean;
  running?: boolean;
  ip?: string;
  url?: string;
  error?: string;
  setupUrl?: string;
}
const tsnStatus = ref<TsnStatus | null>(null);
const tsnKey = ref("");
/** 自启（旧版 #tsnAutostart） */
const tsnAutostart = ref(false);

const tsnRunning = computed(
  () => tsnStatus.value?.state === "ready" || tsnStatus.value?.state === "needs_auth",
);

const tsnStateText = computed(() => {
  const s = tsnStatus.value?.state;
  if (s === "needs_auth") return "需要登录 Tailscale 账号";
  if (s === "ready") return "内置引擎已就绪";
  if (s === "error") return "内置引擎出错";
  return "内置引擎未开启";
});

/** 开关引擎（旧版 #tsnSw change） */
function tsnToggle() {
  if (tsnRunning.value) tsnDisable();
  else tsnEnable();
}

/** 去开通 Funnel（旧版 #tsnSetupBtn，由 sidecar 给的 setupUrl） */
/** 打开 Tailscale 登录页（旧版校验 URL 必须是 login.tailscale.com） */
function copyLoginUrl() {
  if (!loginUrl.value) { toast.show("还没有登录链接 —— 先点连接或重新生成", "bad"); return; }
  void navigator.clipboard.writeText(loginUrl.value).then(() => toast.show("登录链接已复制")).catch(() => toast.show("复制失败", "bad"));
}
function regenLoginUrl() {
  void call("gateway_tailscale_login_regen").catch((e) => toast.show("重新生成失败：" + e, "bad"));
}

function openLoginPage() {
  const url = loginUrl.value;
  if (!/^https:\/\/login\.tailscale\.com\//.test(url)) {
    toast.show("登录地址无效", "bad");
    return;
  }
  void call("open_web_page", { url }).catch((e) => toast.show("打开登录页失败：" + e, "bad"));
}

function tsnOpenSetup() {
  const url = tsnStatus.value?.setupUrl;
  if (!url) { toast.show("开通地址无效", "bad"); return; }
  void call("open_web_page", { url }).catch((e) => toast.show("开通地址无效：" + e, "bad"));
}

function toggleTsnAutostart() {
  const next = !tsnAutostart.value;
  tsnAutostart.value = next;
  void call("gateway_set_autostart", { channel: "tsnet", enabled: next }).catch(() => {});
}
/** Tailscale 登录链接（旧版 #tsLoginUrl） */
const loginUrl = ref("");
/** 事件解绑句柄 */
const unlisteners = ref<(() => void)[]>([]);
/** 二进制下载进度 */
const progress = ref(0);
const downloading = ref(false);

async function refreshTsn() {
  try {
    tsnStatus.value = await call<TsnStatus>("tsnet_status");
  } catch {
    tsnStatus.value = null;
  }
}

function tsnEnable() {
  void run("启用内置引擎", () => call("tsnet_enable"), "内置引擎已启用");
}

function tsnDisable() {
  void run("停用内置引擎", () => call("tsnet_disable"), "内置引擎已停用");
}

function tsnLogout() {
  void run("退出内置引擎", () => call("tsnet_logout"), "已退出登录");
}

function tsnUninstall() {
  void run("卸载内置引擎", () => call("tsnet_uninstall"), "已卸载");
}

function tsnSaveKey() {
  void run("保存密钥", () => call("tsnet_set_authkey", { key: tsnKey.value.trim() }), "密钥已保存");
}

/** Tailscale 自启（旧版 #tsAutostart） */
const tsAutostart = ref(false);

function toggleTsAutostart() {
  const next = !tsAutostart.value;
  tsAutostart.value = next;
  void call("gateway_set_autostart", { channel: "tailscale", enabled: next }).catch(() => {});
}

/** Tailscale 日志弹层（旧版 #tsLogOverlay） */
const tsLogOpen = ref(false);
const tsLogs = ref<string[]>([]);

/** 内置引擎日志（旧版 #tsnLogOverlay） */
const tsnLogOpen = ref(false);
const tsnLogs = ref<string[]>([]);

async function openTsnLog(silent = false) {
  try {
    tsnLogs.value = (await call<string[]>("tsnet_logs")) ?? [];
  } catch {
    tsnLogs.value = [];
  }
  tsnLogOpen.value = true;
  if (!silent) startLogTimer();
}

function clearTsLogs() {
  void call("gateway_tailscale_logs_clear").catch(() => {});
  tsLogs.value = [];
}

function clearTsnLogs() {
  void call("tsnet_logs_clear").catch(() => {});
  tsnLogs.value = [];
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    message.value = "已复制到剪贴板";
    toast.show("已复制到剪贴板");
  } catch {
    /* 剪贴板不可用 */
  }
}

async function openTsLog(silent = false) {
  try {
    tsLogs.value = (await call<string[]>("gateway_tailscale_logs")) ?? [];
  } catch {
    tsLogs.value = [];
  }
  tsLogOpen.value = true;
  if (!silent) startLogTimer();
}

function tsDownload() {
  tsDownloading.value = true;
  tsProgress.value = 0;
  void run("下载 Tailscale", () => call("gateway_tailscale_download"), "开始下载…").finally(() => {
    tsDownloading.value = false;
  });
}

function tsOpenManual() {
  void call("gateway_tailscale_launch_gui").catch(() => {});
}

function tsSaveKey() {
  const key = tsKey.value.trim();
  void run("保存密钥", () => call("gateway_tailscale_set_authkey", { key }), "密钥已保存");
}

function tailscaleService() {
  const action = tsStatus.value?.running ? "stop" : "start";
  void run("切换服务", () => call("gateway_tailscale_service", { action }), "已切换");
}

async function copyTsAddr() {
  const ip = tsStatus.value?.ip;
  if (!ip) return;
  try {
    await navigator.clipboard.writeText(ip);
    message.value = "地址已复制";
    toast.show("地址已复制");
  } catch {
    /* 剪贴板不可用 */
  }
}

/** 配对码弹层（旧版 #gwPairOverlay） */
const pairOpen = ref(false);

/** 卸载确认弹层（旧版 #gwUniOverlay，三张卡共用） */
const uninstallOpen = ref(false);
const uninstallWhat = ref("");
const keepData = ref(false);
const keepHint = ref("");

function openUninstall(what: string, hint = "") {
  uninstallWhat.value = what;
  keepHint.value = hint;
  keepData.value = false;
  uninstallOpen.value = true;
}

function confirmUninstall(checked: boolean) {
  uninstallOpen.value = false;
  void checked;
  tsUninstallNow();
}

/** 日志弹层（旧版 #gwLogOverlay） */
const logOpen = ref(false);
const logs = ref<string[]>([]);

let logTimer: number | undefined;
function stopLogTimer() {
  if (logTimer) { window.clearInterval(logTimer); logTimer = undefined; }
}
function startLogTimer() {
  stopLogTimer();
  logTimer = window.setInterval(() => {
    if (!logOpen.value && !tsLogOpen.value && !tsnLogOpen.value) { stopLogTimer(); return; }
    if (logOpen.value) void refreshLogs();
    if (tsLogOpen.value) void openTsLog(true);
    if (tsnLogOpen.value) void openTsnLog(true);
  }, 2000);
}

async function openGwLog() {
  logOpen.value = true;
  await refreshLogs();
  startLogTimer();
}

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
/** 下拉菜单开合（旧版 #gwAutoMenu） */
const autoMenuOpen = ref(false);
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
/** 残留通道 pid 列表（旧版 gateway_tunnel_orphans 返回） */
interface OrphanTunnel { pid: number; url?: string; }
const orphans = ref<OrphanTunnel[]>([]);
async function refreshOrphans() {
  try {
    orphans.value = (await call<OrphanTunnel[]>("gateway_tunnel_orphans")) ?? [];
  } catch {
    orphans.value = [];
  }
}
/** 是否有上次遗留的孤儿通道（旧版 #gwOrphanBox 的显示条件） */
const orphanRunning = computed(
  () => orphans.value.length > 0 || (!cfRunning.value && Boolean(config.value?.tunnel?.url) && config.value?.tunnel?.state === "orphan"),
);

watch(logOpen, (open) => {
  if (open) void refreshLogs();
});

async function refreshAll() {
  void refreshOrphans();
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
    toast.show(done);
    await refreshAll();
  } catch (error) {
    errorText.value = String(error);
    toast.show(label + "失败：" + error, "bad");
  } finally {
    busy.value = "";
  }
}

onMounted(async () => {
  // 旧版监听的事件（补齐）
  const { listen } = await import("@tauri-apps/api/event");
  const offs: (() => void)[] = [];
  // 二进制下载进度 / 完成
  offs.push(
    await listen<{ received?: number; total?: number; percent?: number; stage?: string }>(
      "gateway-binary-progress",
      (event) => {
        const p = event.payload ?? {};
        progress.value = p.total ? Math.round(((p.received ?? 0) / p.total) * 100) : (p.percent ?? 0);
        downloading.value = true;
      },
    ),
  );
  offs.push(
    await listen<{ ok?: boolean; version?: string; error?: string }>("gateway-binary-done", (event) => {
      downloading.value = false;
      message.value = event.payload?.ok
        ? `cloudflared ${event.payload?.version ?? ""} 就绪`
        : `下载失败：${event.payload?.error ?? "未知错误"}`;
      void refreshAll();
    }),
  );
  // 通道状态变化 → 重拉快照
  offs.push(await listen("gateway-tunnel-state", () => void refreshAll()));
  // Tailscale 安装进度 / 完成 / 登录链接
  offs.push(await listen("tailscale-progress", () => void refreshAll()));
  offs.push(await listen("tailscale-done", () => void refreshAll()));
  offs.push(
    await listen<{ url?: string }>("tailscale-login-url", (event) => {
      if (event.payload?.url) loginUrl.value = event.payload.url;
    }),
  );
  unlisteners.value = offs;
  await refreshAll();
  await refreshFunnel();
  await refreshTsn();
  setupAuto();
});

onUnmounted(() => {
  if (autoTimer !== null) window.clearInterval(autoTimer);
  for (const off of unlisteners.value) off();
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

/** 二进制状态 / 卸载 / 清除凭据 / 保存配置（旧版 gateway_* 系列） */
function checkBinary() {
  void run("检查 cloudflared", () => call("gateway_binary_status"), "已检查");
}

function uninstallBinary() {
  void run("卸载 cloudflared", () => call("gateway_binary_uninstall"), "已卸载");
}

function clearCredentials() {
  void run("清除登录凭据", () => call("gateway_clear_credentials"), "已清除凭据");
}

/** 保存通道配置（旧版 persistConfig，hub.html:13408） */
function persistConfig(mode: string) {
  void run(
    "保存配置",
    () =>
      call("gateway_save_config", {
        mode,
        named_url: namedUrl.value,
        named_token: namedToken.value,
        autostart: Boolean(config.value?.autostart),
      }),
    "配置已保存",
  );
}

/** 切模式（旧版 selectMode(next, save=true)） */
function selectMode(next: "quick" | "named") {
  if (config.value) config.value.mode = next;
  persistConfig(next);
}


function tsLogout() {
  void run("退出 Tailscale 登录", () => call("gateway_tailscale_logout"), "已退出登录");
}

function tsExtensionSettings() {
  void call("gateway_tailscale_open_extension_settings").catch(() => {});
}

function tsFunnelToggle(on: boolean) {
  if (on) {
    void run("开启 Funnel", () => call("gateway_tailscale_funnel_enable"), "Funnel 已开启");
  } else {
    void run("关闭 Funnel", () => call("gateway_tailscale_funnel_disable"), "Funnel 已关闭");
  }
}

/** Funnel 当前状态（进入卡片时查一次） */
async function refreshFunnel() {
  try {
    await call("gateway_tailscale_funnel_status");
  } catch {
    /* 未安装时忽略 */
  }
}

function tsUninstallNow() {
  void run("卸载 Tailscale", () => call("gateway_tailscale_uninstall"), "已卸载");
}



/** 残留通道：沿用 / 清理多余 */
function adoptOrphan() {
  const pid = orphans.value[0]?.pid;
  if (!pid) { toast.show("没有可沿用的残留通道", "bad"); return; }
  void run("沿用残留通道", () => call("gateway_tunnel_adopt", { pid }), "已沿用");
}


function killOrphan() {
  const pid = orphans.value[0]?.pid;
  if (!pid) { toast.show("没有可终止的残留通道", "bad"); return; }
  void run("清理残留", () => call("gateway_tunnel_kill_orphan", { pid }), "已清理残留通道");
}

// ---- Tailscale ----
function toggleTailscale() {
  const up = !tsStatus.value?.running;
  void run("切换 Tailscale", () => call("gateway_tailscale_toggle", { up }), "已切换");
}

// ---- 配对 ----
/** 重新出码（旧版 openPairReissue → gateway_pair_reissue） */
function reissuePair(id: string) {
  void run("重新出码", () => call("gateway_pair_reissue", { id }), "已重新出码");
}

/** 配对行 meta 文案（旧版 renderPairClients） */
function pairMeta(pair: PairRow): string {
  const bits: string[] = [];
  if (pair.url) bits.push("配对地址 " + pair.url.replace(/^https?:\/\//, ""));
  bits.push(
    pair.revoked
      ? `已吊销 · 配对于 ${fmtTime(pair.createdAt)}`
      : `最近访问 ${fmtTime(pair.lastSeen)} · 配对于 ${fmtTime(pair.createdAt)}`,
  );
  return bits.join(" · ");
}

/** 来源 chip（旧版按 pair_kind 判定文案与 class） */
function pairSrcChip(pair: PairRow): { cls: string; text: string; tip: string } {
  const labels: Record<string, string> = {
    tunnel: "Cloudflare 通道",
    ts: "Tailscale 直连",
    funnel: "Funnel（系统 Tailscale）",
    tsnet: "Funnel（内置引擎）",
  };
  const label = labels[pair.kind ?? ""];
  let cls = "";
  let text = "";
  if (label) {
    cls = pair.kind === "tunnel" ? "src-tunnel" : "src-ts";
    text = label;
  } else if (pair.url && /ts\.net|100\./.test(pair.url)) {
    cls = "src-ts";
    text = "Tailscale";
  } else if (pair.url) {
    cls = "src-tunnel";
    text = "Cloudflare 通道";
  } else {
    text = "不带地址";
  }
  return { cls, text, tip: text + (pair.url ? ` · ${pair.url}` : " · 出码时没有可用地址") };
}

function fmtTime(ts?: number): string {
  if (!ts) return "—";
  return new Date(ts).toLocaleString("zh-CN", { hour12: false });
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
    <div class="gw-ar" id="gwAuto" title="自动刷新这一页的状态（单位：秒）">
      <span class="gw-ar-label">自动刷新</span>
      <div class="gw-ar-combo">
        <input
          id="gwAutoSec"
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
        <button
          type="button"
          id="gwAutoCaret"
          class="gw-ar-caret"
          aria-label="选择常用刷新间隔"
          aria-haspopup="listbox"
          :aria-expanded="autoMenuOpen"
          @click="autoMenuOpen = !autoMenuOpen"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </button>
        <!-- 常用刷新间隔（旧版 #gwAutoMenu） -->
        <div v-if="autoMenuOpen" id="gwAutoMenu" class="gw-ar-menu" role="listbox" aria-label="常用刷新间隔">
          <button
            v-for="sec in [1, 3, 5, 10, 30]"
            :key="sec"
            type="button"
            role="option"
            :aria-selected="autoSec === sec"
            @click="autoSec = sec; autoMenuOpen = false; setupAuto()"
          >
            {{ sec }} 秒
          </button>
        </div>
      </div>
      <button
        type="button"
        id="gwAutoToggle"
        class="gw-ar-toggle"
        :aria-pressed="!autoRunning"
        @click="autoRunning = !autoRunning; setupAuto()"
      >
        <svg
          id="gwAutoIconPause"
          viewBox="0 0 24 24"
          fill="currentColor"
          aria-hidden="true"
          :style="{ display: autoRunning ? undefined : 'none' }"
        >
          <rect x="7" y="5" width="3.4" height="14" rx="1.2" />
          <rect x="13.6" y="5" width="3.4" height="14" rx="1.2" />
        </svg>
        <svg
          id="gwAutoIconPlay"
          viewBox="0 0 24 24"
          fill="currentColor"
          aria-hidden="true"
          :style="{ display: autoRunning ? 'none' : undefined }"
        >
          <path d="M8 5.6v12.8a1 1 0 0 0 1.52.85l9.6-6.4a1 1 0 0 0 0-1.7l-9.6-6.4A1 1 0 0 0 8 5.6z" />
        </svg>
        <span id="gwAutoToggleText">{{ autoRunning ? "暂停" : "继续" }}</span>
      </button>
    </div>
  </div>

  <!-- 分页标签（旧版 #gwTabs，4 个分页带状态点） -->
  <div class="gw-tabs" id="gwTabs" role="tablist" aria-label="远程接入分页">
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
        :id="tab.key === 'cf' ? 'gwTabDotCf' : tab.key === 'ts' ? 'gwTabDotTs' : 'gwTabDotTsn'"
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
    <div class="gw-note" id="gwWarnQuick" :hidden="config?.mode === 'named'">
      快速隧道的地址在通道重启后会变化 —— 重启电脑 / 通道后，已配对的客户端要重新配对一次。
      要固定地址请改用「命名隧道」（需 Cloudflare 账号 + 托管域名），或用下方 Tailscale
      直连（装好即用、地址长期固定、不出公网）。
    </div>

    <div class="gw-card">
      <div class="gw-card-head">
        <h3>本机通道</h3>
        <div class="segmented" id="gwModeTabs" role="tablist" aria-label="通道模式">
          <button role="tab" data-mode="quick" :aria-selected="config?.mode !== 'named'" @click="selectMode('quick')">
            快速隧道
          </button>
          <button role="tab" data-mode="named" :aria-selected="config?.mode === 'named'" @click="selectMode('named')">
            命名隧道
          </button>
        </div>
        <button v-if="cfInstalled" type="button" id="gwUninstallBtn" class="btn" :disabled="!!busy" @click="uninstallBinary">卸载</button>
        <button type="button" id="gwLogBtn" class="btn" @click="openGwLog">日志</button>
        <button type="button" id="gwRefreshBtn" class="btn" @click="refreshAll">重新检测</button>
        <span class="gw-sw-group" title="开启 / 关闭 Cloudflare 通道">
          <span class="gw-sw-cap">通道</span>
          <label class="switch" :class="{ loading: !!busy }" id="gwToggleWrap">
            <input type="checkbox" id="gwToggleSw" :checked="cfRunning || !!busy" :disabled="!!busy" @change="toggleCf" />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
      </div>

      <div class="gw-status">
        <span class="pulse" id="gwPulse" :hidden="!cfRunning" />
        <span id="gwStateText">{{ cfRunning ? "通道已开启" : "通道未开启" }}</span>
        <a
          id="gwUrl"
          :href="cfUrl || undefined"
          :hidden="!cfUrl"
          class="mono gw-url"
          target="_blank"
          rel="noopener"
          >{{ cfUrl }}</a
        >
        <!-- 地址左、动作右：复制与「生成此地址的配对码」统一右对齐（与 Tailscale 卡同款） -->
        <span style="flex: 1" />
        <button type="button" id="gwCopyUrl" class="link-btn" :hidden="!cfUrl" @click="copyText(cfUrl)">
          复制
        </button>
        <button type="button" id="gwPairBtn" class="btn" :hidden="!cfUrl" @click="pairOpen = true">
          生成此地址的配对码
        </button>
        <span class="mono gw-meta" id="gwMeta">{{ config?.tunnel?.state || "" }}</span>
      </div>
      <div id="gwErrRow" class="gw-err" :hidden="!config?.tunnel?.error">{{ config?.tunnel?.error }}</div>

      <!-- 残留通道（旧版 #gwOrphanBox）：沿用或终止 -->
      <div id="gwOrphanBox" :hidden="!orphanRunning" style="margin-top: 12px">
        <p class="field-hint" style="margin: 0 0 6px">
          检测到上次异常退出留下的残留通道 —— 「沿用」地址不变（客户端无需重新配对），
          「终止」后点上方「开启通道」会创建新地址。
        </p>
        <div id="gwOrphanList" class="gw-clients">
          <div class="gw-client">
            <span class="mono">{{ config?.tunnel?.url || "（无地址）" }}</span>
            <span style="flex: 1" />
            <button type="button" class="link-btn" @click="adoptOrphan">沿用</button>
            <button type="button" class="link-btn" :disabled="!!busy" @click="killOrphan">终止</button>
          </div>
        </div>
      </div>

      <!-- named 配置区（旧版 #gwNamedForm，quick 模式隐藏） -->
      <div id="gwNamedForm" :hidden="config?.mode !== 'named'" style="margin-top: 12px">
        <div class="field">
          <label for="gwNamedToken">Tunnel Token</label>
          <input
            id="gwNamedToken"
            v-model="namedToken"
            type="password"
            class="mono"
            placeholder="Cloudflare Zero Trust 创建隧道后给出的 Token"
            autocomplete="off"
          />
        </div>
        <div class="field">
          <label for="gwNamedUrl">对外访问地址（可选）</label>
          <input
            id="gwNamedUrl"
            v-model="namedUrl"
            type="text"
            class="mono"
            placeholder="https://anyboot.example.com"
            autocomplete="off"
          />
          <p class="field-hint">填了它，配对串里就用这个地址；留空则配对串里不带地址，客户端要自己填。</p>
        </div>
      </div>

      <label class="gw-auto">
        <input type="checkbox" id="gwAutostart" :checked="config?.autostart" @change="toggleAutostart" />
        <span class="gw-auto-t">启动软件时自动开启通道</span>
        <span class="gw-auto-h">cloudflared 随 AnyBoot 一起拉起</span>
      </label>

      <div class="gw-binary">
        <span id="gwBinaryText">
          {{
            !config
              ? "正在检查 cloudflared…"
              : cfInstalled
                ? `cloudflared ${config?.binary?.version || ""} 已就位`
                : "cloudflared 未就位 —— 下载（自动校验 SHA-256）或指定本地已有的可执行文件"
          }}
        </span>
        <span style="flex: 1" />
        <button v-if="!cfInstalled" type="button" id="gwDownloadBtn" class="btn" :disabled="!!busy" @click="downloadBinary">
          下载 cloudflared
        </button>
        <button v-if="!cfInstalled" type="button" id="gwManualBtn" class="btn" @click="manualOpen = !manualOpen">
          指定本地路径
        </button>
        <button type="button" class="btn" :disabled="!!busy" @click="checkBinary">检查</button>
        <button v-if="cfInstalled" type="button" class="btn" :disabled="!!busy" @click="uninstallBinary">卸载</button>
        <button type="button" class="btn" :disabled="!!busy" @click="clearCredentials">清除登录凭据</button>
      </div>

      <!-- 下载进度（旧版 #gwProgressRow） -->
      <div id="gwProgressRow" :hidden="!downloading" class="gw-progress">
        <div class="gw-progress-bar"><div id="gwProgressBar" :style="{ width: progress + '%' }" /></div>
        <span class="mono" id="gwProgressText">{{ downloading ? `下载 ${progress}%` : "" }}</span>
      </div>

      <!-- 清除登录凭据（旧版 #gwLogoutRow）：命名隧道的 Tunnel Token 就是登录态 -->
      <div
        id="gwLogoutRow"
        :hidden="config?.mode !== 'named'"
        style="display: flex; gap: 8px; align-items: center; margin-top: 10px; flex-wrap: wrap"
      >
        <button type="button" id="gwLogoutBtn" class="link-btn" @click="clearCredentials">清除登录凭据</button>
        <span class="field-hint" style="margin: 0">
          停止通道并删除保存的命名隧道 Token 与对外地址，模式回落「快速隧道」；cloudflared 程序保留。
        </span>
      </div>

      <!-- 指定本地路径（旧版 #gwManualRow） -->
      <div id="gwManualRow" :hidden="!manualOpen" class="gw-binary">
        <input
          id="gwManualPath"
          v-model="manualPath"
          type="text"
          class="mono"
          placeholder="cloudflared.exe 的完整路径"
          spellcheck="false"
        />
        <button type="button" id="gwManualCancel" class="btn" @click="manualOpen = false">取消</button>
        <button
          type="button"
          id="gwManualOk"
          class="btn primary"
          :disabled="!manualPath.trim() || !!busy"
          @click="setManualBinary"
        >
          确定
        </button>
      </div>
    </div>
  </div>

  <!-- ===== Tailscale 分页（旧版 hub.html:2931–3129，65 个 id） ===== -->
  <div v-show="activeTab === 'ts'" class="gw-tabpane">
    <div class="gw-card" id="tsCard">
      <div class="gw-card-head">
        <h3>Tailscale 直连</h3>
        <span class="tsf-badge" id="tsBadge" :hidden="!tsStatus?.installed">已安装 · 局域网 100.x</span>
        <span style="flex: 1" />
        <span
          class="gw-sw-group"
          id="tsMainSwGroup"
          :hidden="!tsStatus?.installed"
          title="连接 / 断开 Tailscale（登录级，等于 tailscale up / down；待登录时点击会拉起浏览器登录）"
        >
          <span class="gw-sw-cap">连接</span>
          <label class="switch" id="tsMainSwWrap">
            <input type="checkbox" id="tsMainSw" :checked="tsStatus?.running" @change="toggleTailscale" />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
        <span
          class="gw-sw-group"
          id="tsSvcSwGroup"
          :hidden="!tsStatus?.installed"
          title="启动 / 停止 tailscaled 系统服务（Windows；需要管理员权限）"
        >
          <span class="gw-sw-cap">服务</span>
          <label class="switch" id="tsSvcSwWrap">
            <input type="checkbox" id="tsSvcSw" :checked="tsStatus?.running" @change="tailscaleService" />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
        <button v-if="tsStatus?.installed" type="button" class="btn" id="tsUninstallBtn" @click="openUninstall('将卸载 Tailscale 程序本体', '不注销、不清 AuthKey，重装后仍是同一台已授权设备（管理台里的节点记录也保留）。')">
          卸载
        </button>
        <button type="button" class="btn" id="tsLogBtn" @click="() => openTsLog()">日志</button>
        <button type="button" class="btn" id="tsRefreshBtn" @click="refreshAll">重新检测</button>
      </div>

      <div class="gw-status" id="tsStateRow">
        <span class="pulse" id="tsPulse" :hidden="!tsStatus?.running" />
        <span id="tsStateText">{{ tsStateText }}</span>
        <span class="mono gw-meta" id="tsMeta">{{ tsStatus?.version || "" }}</span>
      </div>

      <!-- 未安装态：#tsDlRow -->
      <div id="tsDlRow" :hidden="tsStatus?.installed">
        <p class="field-hint" id="tsDlHint" style="margin: 8px 0 0">
          没检测到 Tailscale —— 它给两台设备组一个加密虚拟局域网（100.x 地址，不出公网、地址长期固定）。
          下载完成后安装器会自动弹出，按提示装好并登录，再点「重新检测」。
        </p>
        <div class="gw-binary">
          <span id="tsDlText">安装包未下载</span>
          <span style="flex: 1" />
          <button type="button" class="btn" id="tsDownloadBtn" :disabled="!!busy" @click="tsDownload">下载 Tailscale</button>
          <button type="button" class="btn" id="tsManualLink" @click="tsOpenManual">打开官网手动下载</button>
        </div>
        <div class="gw-progress" id="tsProgressRow" :hidden="!tsDownloading">
          <div class="gw-progress-bar"><div id="tsProgressBar" :style="{ width: tsProgress + '%' }" /></div>
          <span class="mono" id="tsProgressText">{{ tsDownloading ? `下载 ${tsProgress}%` : "" }}</span>
        </div>
      </div>

      <!-- 未连接态：#tsKeyRow（AuthKey 配置） -->
      <div id="tsKeyRow" :hidden="!tsStatus?.installed || tsStatus?.running">
        <div class="field" style="margin-top: 10px">
          <label for="tsKey">AuthKey（可选，推荐填）</label>
          <div style="display: flex; gap: 8px; align-items: center">
            <input
              id="tsKey"
              v-model="tsKey"
              type="password"
              class="mono"
              style="flex: 1"
              placeholder="留空则走浏览器登录"
              autocomplete="off"
            />
            <button type="button" class="btn" id="tsKeySave" :disabled="!!busy" @click="tsSaveKey">保存</button>
          </div>
          <p class="field-hint" id="tsKeyHint">
            在 Tailscale 管理控制台 → Settings → Keys 里签发一次性的设备授权密钥；填了它连接时免浏览器登录。
          </p>
        </div>
      </div>

      <!-- 需登录态：#tsLoginRow -->
      <div id="tsLoginRow" :hidden="true">
        <p class="field-hint" style="margin: 8px 0 0">
          需要登录 Tailscale 账号 —— 点下面按钮在浏览器完成授权（一次即可，凭据会记住）。
          登录完成后自动继续，不用再点开关。
        </p>
        <div style="display: flex; gap: 8px; align-items: center; margin-top: 8px">
          <button type="button" class="btn primary" id="tsLoginOpen" @click="openLoginPage">打开登录页</button>
          <button type="button" class="link-btn" id="tsLoginCopy" @click="copyLoginUrl">复制登录链接</button>
          <button type="button" class="link-btn" id="tsLoginRegen" @click="regenLoginUrl">重新生成链接</button>
        </div>
        <p class="field-hint mono" id="tsLoginUrl" style="margin: 8px 0 0; user-select: all; overflow-wrap: anywhere; word-break: break-all">{{ loginUrl }}</p>
        <p class="field-hint" style="margin: 6px 0 0">
          ⏱️ 登录链接约 <b>10 分钟</b>内有效；过期或点了没反应，就点<b>重新生成链接</b>或把开关<b>关开一次</b>。
          链接也可以复制到手机/其它设备的浏览器打开，授权完这边自动继续。
        </p>
      </div>

      <!-- 运行中态：#tsAddrRow -->
      <div id="tsAddrRow" :hidden="!tsStatus?.running">
        <div class="gw-status" style="margin-top: 8px">
          <span class="field-hint" style="margin: 0">局域网直连地址</span>
          <span class="mono ts-addr" id="tsAddr">{{ tsStatus?.ip || "" }}</span>
          <span style="flex: 1" />
          <button type="button" class="link-btn" id="tsCopyAddr" @click="copyTsAddr">复制</button>
          <button type="button" class="btn" id="tsPairBtn" @click="pairOpen = true">生成此地址的配对码</button>
        </div>
        <div class="gw-status" style="margin-top: 6px">
          <span
            id="tsHealthDot"
            style="display: inline-flex; width: 8px; height: 8px; border-radius: 50%; background: rgb(var(--fg-faint)); flex: none"
          />
          <span id="tsHealthText" style="font-size: 12px; color: rgb(var(--fg-dim))">正在自检本机服务…</span>
        </div>
        <p class="field-hint" style="margin-top: 8px; color: rgb(var(--warn))">
          ⚠️ 此地址仅同一 tailnet 内设备（登录同一 Tailscale 账号）可达，安全依赖配对令牌，请勿外传配对码。
          目标机睡眠后同样失联 —— 唤醒仍需局域网内常开的跳板机代发广播包。
        </p>
      </div>

      <!-- 停止态：#tsStoppedRow -->
      <div id="tsStoppedRow" :hidden="true">
        <p class="field-hint" style="margin: 8px 0 0">
          Tailscale 处于停止状态 —— 在系统托盘 / 菜单栏把它连上后点「重新检测」。
        </p>
      </div>

      <!-- GUI 未启动态：#tsGuiRow -->
      <div id="tsGuiRow" :hidden="true">
        <p class="field-hint" style="margin: 8px 0 6px">
          Tailscale 已安装但 GUI 没有运行 —— 点下面打开 Tailscale.app（菜单栏出现图标即就绪），稍等几秒后自动重新检测。
        </p>
        <button type="button" class="btn" id="tsLaunchGuiBtn" @click="tsOpenManual">打开 Tailscale.app</button>
      </div>

      <!-- 网络扩展待重启态：#tsRebootRow -->
      <div id="tsRebootRow" :hidden="true">
        <p class="field-hint" style="margin: 8px 0 6px; color: rgb(var(--warn))">
          Tailscale 已经装好，但系统里那个旧的网络扩展还在「等待重启卸载」——
          新扩展因此无法激活，连接会一直失败。<b>重启 Mac</b> 后它会自动换成新扩展，
          回来点「重新检测」即可。（重装没用，别反复卸装）
        </p>
        <button type="button" class="btn" id="tsRebootRecheck" @click="refreshAll">已重启，重新检测</button>
      </div>

      <!-- 网络扩展待授权态：#tsApprovalRow -->
      <div id="tsApprovalRow" :hidden="true">
        <p class="field-hint" style="margin: 8px 0 6px; color: rgb(var(--warn))">
          Tailscale 已安装，但系统里的<b>网络扩展还没被允许</b> —— 批准之前它连不上网络，点连接一定失败。
          点下面打开「系统设置 → 通用 → 登录项与扩展 → 网络扩展」，把 <b>Tailscale</b> 打开
          （可能需要指纹 / 密码），回来点「重新检测」即可。
        </p>
        <button type="button" class="btn" id="tsApprovalBtn" @click="tsExtensionSettings">打开网络扩展设置</button>
      </div>

      <!-- Funnel 公网通道区块：#tsfBlock（4 种状态） -->
      <div id="tsfBlock" :hidden="!tsStatus?.installed">
        <div class="tsf-sep">
          <div class="gw-card-head" style="margin: 0">
            <h3 style="font-size: 13px">Funnel 公网通道</h3>
            <span class="tsf-badge" id="tsfBadge" :hidden="!tsfStatus?.url">公网 · 经 Tailscale 中继</span>
            <span style="flex: 1" />
            <span class="gw-sw-group" title="开启 / 关闭 Funnel 公网通道；需授权时点击会直接打开官方授权页">
              <label class="switch" id="tsfSwWrap">
                <input type="checkbox" id="tsfSw" :checked="tsfStatus?.state === 'ready'" @change="tsFunnelToggle(($event.target as HTMLInputElement).checked)" />
                <span class="trk" aria-hidden="true" />
              </label>
            </span>
          </div>
          <div class="gw-status" id="tsfStateRow" style="margin-top: 8px">
            <span class="pulse" id="tsfPulse" :hidden="tsfStatus?.state !== 'starting'" />
            <span id="tsfStateText">{{ tsfStateText }}</span>
          </div>

          <!-- off 态说明 -->
          <p class="field-hint" id="tsfOffRow" :hidden="tsfStatus?.state !== 'off'" style="margin: 8px 0 0">
            把本机服务经 Tailscale 官方中继发布成公网地址（https://机器名.ts.net）——
            手机<b>不装 Tailscale</b> 也能访问，地址固定；国内网络可能不稳，流量有带宽限制。
          </p>

          <!-- ready 态：公网地址行 -->
          <div id="tsfAddrRow" :hidden="tsfStatus?.state !== 'ready'">
            <div class="gw-status" style="margin-top: 8px">
              <span class="field-hint" style="margin: 0">公网地址</span>
              <span class="mono ts-addr" id="tsfAddr">{{ tsfStatus?.url || "" }}</span>
              <span style="flex: 1" />
              <button type="button" class="link-btn" id="tsfCopy" @click="copyText(tsfStatus?.url || '')">复制</button>
              <button type="button" class="btn" id="tsfPairBtn" @click="pairOpen = true">生成此地址的配对码</button>
            </div>
            <p class="field-hint" style="margin-top: 8px; color: rgb(var(--warn))">
              ⚠️ 此地址公网可达，安全依赖配对令牌，请勿外传配对码。目标机睡眠后同样失联。
            </p>
          </div>

          <!-- needs_authorize 态 -->
          <div id="tsfAuthRow" :hidden="tsfStatus?.state !== 'needs_authorize'">
            <p class="field-hint" style="margin: 8px 0 0">
              需要在 Tailscale 官方页允许 Funnel 并启用 HTTPS 证书（需 Owner / Admin 账号），一次授权长期有效。
            </p>
            <div style="display: flex; gap: 8px; align-items: center; margin-top: 8px">
              <button type="button" class="btn primary" id="tsfAuthBtn" @click="tsFunnelAuth">去官方授权页</button>
              <button type="button" class="link-btn" id="tsfRetryBtn" @click="refreshFunnel">授权完成，重试</button>
            </div>
          </div>

          <!-- error 态 -->
          <div id="tsfErrRow" :hidden="tsfStatus?.state !== 'error'">
            <p class="field-hint" id="tsfErrText" style="margin: 8px 0 0">{{ tsfStatus?.error || "" }}</p>
            <button type="button" class="link-btn" id="tsfRetryBtn2" style="margin-top: 4px" @click="refreshFunnel">重试</button>
          </div>
        </div>
      </div>

      <!-- 清除登录凭据：#tsLogoutRow -->
      <div id="tsLogoutRow" style="display: flex; gap: 8px; align-items: center; margin-top: 10px; flex-wrap: wrap">
        <button type="button" class="link-btn" id="tsLogoutBtn" @click="tsLogout">清除登录凭据</button>
        <span class="field-hint" style="margin: 0">
          执行 tailscale logout：断开并清除本机登录凭据<b>与保存的 AuthKey</b>，下次连接需重新授权；Tailscale 管理台里的设备记录需自行移除。
        </span>
      </div>

      <!-- 自启 -->
      <label class="gw-auto">
        <input type="checkbox" id="tsAutostart" :checked="tsAutostart" @change="toggleTsAutostart" />
        <span class="gw-auto-t">启动软件时自动连接 Tailscale</span>
        <span class="gw-auto-h">等价于自动点「连接」</span>
      </label>
    </div>
  </div>

  <!-- ===== Tailscale（内置）分页（旧版 hub.html:3130–3225，27 个 id） ===== -->
  <div v-show="activeTab === 'tsn'" class="gw-tabpane">
    <div class="gw-card" id="tsnCard">
      <div class="gw-card-head">
        <h3>Tailscale (内置)</h3>
        <span class="tsf-badge" id="tsnBadge" :hidden="tsnStatus?.state !== 'ready'">免安装 · 经 Tailscale 中继</span>
        <span style="flex: 1" />
        <span class="gw-sw-group" title="开启 / 关闭内置 Tailscale 引擎">
          <span class="gw-sw-cap">引擎</span>
          <label class="switch" id="tsnSwWrap">
            <input
              type="checkbox"
              id="tsnSw"
              :checked="tsnStatus?.state === 'ready' || tsnStatus?.state === 'needs_auth'"
              @change="tsnToggle"
            />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
        <button type="button" class="btn" id="tsnUninstallBtn" @click="tsnUninstall">卸载</button>
        <button type="button" class="btn" id="tsnLogBtn" @click="() => openTsnLog()">日志</button>
        <button type="button" class="btn" id="tsnRefreshBtn" @click="refreshTsn">重新检测</button>
      </div>

      <div class="gw-status">
        <span class="pulse" id="tsnPulse" :hidden="!tsnRunning" />
        <span id="tsnStateText">{{ tsnStateText }}</span>
      </div>

      <!-- off 态：说明 + AuthKey 配置 -->
      <div id="tsnOffRow" :hidden="tsnStatus?.state !== 'off'">
        <p class="field-hint" style="margin: 8px 0 0">
          本机<b>无需安装 Tailscale</b>——内置引擎把本机服务发布成公网地址
          （https://anyboot.….ts.net），手机不装 Tailscale 也能访问，地址固定；
          国内网络首次连接协调服务器可能不稳，流量经 Tailscale 中继有带宽限制。
        </p>
        <div class="field" style="margin-top: 10px">
          <label for="tsnKey">AuthKey（可选，推荐填）</label>
          <div style="display: flex; gap: 8px; align-items: center">
            <input
              id="tsnKey"
              v-model="tsnKey"
              type="password"
              class="mono"
              style="flex: 1"
              placeholder="留空则走浏览器登录"
              autocomplete="off"
            />
            <button type="button" class="btn" id="tsnKeySave" :disabled="!!busy" @click="tsnSaveKey">保存</button>
          </div>
          <p class="field-hint">
            在 Tailscale 管理控制台 → Settings → Keys 里签发一次性的设备授权密钥；填了它开启引擎时免浏览器登录。
          </p>
        </div>
      </div>

      <!-- needs_auth 态 -->
      <div id="tsnAuthRow" :hidden="tsnStatus?.state !== 'needs_auth'">
        <p class="field-hint" style="margin: 8px 0 0">
          需要登录 Tailscale 账号 —— 点下面按钮在浏览器完成授权（一次即可，凭据会记住）。
          登录完成后引擎自动继续，不用再点开关。
          登录后引擎要<b>初始化环境资源</b>（节点配置、HTTPS 证书、公网 DNS 等），通常需要几分钟——
          请<b>每隔几分钟回来看一眼</b>状态或日志，就绪后这里会显示、日志里会打印<b>公网地址</b>。
        </p>
        <div style="display: flex; gap: 8px; align-items: center; margin-top: 8px">
          <button type="button" class="btn primary" id="tsnLoginBtn" @click="openLoginPage">打开登录页</button>
          <button type="button" class="link-btn" id="tsnLoginCopy" @click="copyLoginUrl">复制登录链接</button>
          <button type="button" class="link-btn" id="tsnLoginRegen" @click="regenLoginUrl">重新生成链接</button>
        </div>
        <p class="field-hint mono" id="tsnLoginUrl" style="margin: 8px 0 0; user-select: all; overflow-wrap: anywhere; word-break: break-all">{{ loginUrl }}</p>
        <p class="field-hint" style="margin: 6px 0 0">
          ⏱️ 登录链接约 <b>10 分钟</b>内有效；过期或点了没反应，就把引擎<b>关开一次</b>重新生成。
          链接也可以复制到手机/其它设备的浏览器打开，授权完这边自动继续。
        </p>
      </div>

      <!-- ready 态：公网地址行 -->
      <div id="tsnAddrRow" :hidden="tsnStatus?.state !== 'ready'">
        <div class="gw-status" style="margin-top: 8px">
          <span class="field-hint" style="margin: 0">公网地址</span>
          <span class="mono ts-addr" id="tsnAddr">{{ tsnStatus?.url || "" }}</span>
          <span style="flex: 1" />
          <button type="button" class="link-btn" id="tsnCopy" @click="copyText(tsnStatus?.url || '')">复制</button>
          <button type="button" class="btn" id="tsnPairBtn" @click="pairOpen = true">生成此地址的配对码</button>
        </div>
        <p class="field-hint" style="margin-top: 8px; color: rgb(var(--warn))">
          ⚠️ 此地址公网可达，安全依赖配对令牌，请勿外传配对码。目标机睡眠后同样失联。
        </p>
      </div>

      <!-- error 态 -->
      <div id="tsnErrRow" :hidden="tsnStatus?.state !== 'error'">
        <p class="field-hint" id="tsnErrText" style="margin: 8px 0 0">{{ tsnStatus?.error || "" }}</p>
        <div style="display: flex; gap: 8px; align-items: center; margin-top: 6px; flex-wrap: wrap">
          <button type="button" class="link-btn" id="tsnRetryBtn" @click="refreshTsn">重试</button>
          <button type="button" class="link-btn" id="tsnSetupBtn" :hidden="!tsnStatus?.setupUrl" @click="tsnOpenSetup">去开通 Funnel</button>
        </div>
      </div>

      <!-- 清除登录凭据 -->
      <div style="display: flex; gap: 8px; align-items: center; margin-top: 10px; flex-wrap: wrap">
        <button type="button" class="link-btn" id="tsnLogoutBtn" @click="tsnLogout">清除登录凭据</button>
        <span class="field-hint" style="margin: 0">
          停引擎并删除本机登录凭据，下次开启需重新授权；Tailscale 管理台里的设备记录需自行移除。
        </span>
      </div>

      <!-- 自启 -->
      <label class="gw-auto">
        <input type="checkbox" id="tsnAutostart" :checked="tsnAutostart" @change="toggleTsnAutostart" />
        <span class="gw-auto-t">启动软件时自动开启内置引擎</span>
        <span class="gw-auto-h">凭据已记住，通常直接就绪</span>
      </label>
    </div>
  </div>

  <!-- ===== 配对管理分页 ===== -->
  <!-- ===== 配对管理分页（旧版 hub.html:3225–3252） ===== -->
  <div v-show="activeTab === 'pair'" class="gw-tabpane">
    <!-- 配对管理卡 -->
    <div class="gw-card">
      <div class="gw-card-head">
        <h3>配对管理</h3>
        <button type="button" class="btn primary" id="gwIssueBtn" @click="pairOpen = true">生成配对码</button>
      </div>
      <p class="field-hint" style="margin-top: 0">
        配对码 = 这台电脑的隧道地址 + 一枚专属令牌，每个客户端一枚、可单独吊销。
        通道没开启时也能生成（隧道地址留空，客户端拿到后自己填）。
        快速隧道每次重启软件都会换新地址 —— 旧记录里标的「配对地址」还能对账：
        客户端「最近访问」时间比换址晚，说明它已用新地址在连；一直没动，就是还在用旧地址（或没在连）。
        配对串只在生成时显示一次（服务端只存哈希，事后查不到）——
        扫码失败或对方删了配对，在下面列表点「重新出码」换枚新令牌重扫即可，不用重新添加。
      </p>
      <div id="gwClientList" class="gw-clients">
        <div v-if="!pairs.length" class="gw-clients-empty">
          还没有配对的客户端 —— 点「生成配对码」给手机或另一台电脑发一枚令牌。
        </div>
        <div v-for="pair in pairs" :key="pair.id" class="gw-client">
          <div style="flex: 1; min-width: 0">
            <div class="gw-client-name">{{ pair.label || pair.id }}</div>
            <div class="gw-client-meta" :title="pair.url || ''">{{ pairMeta(pair) }}</div>
          </div>
          <span
            :class="['ts-chip', pairSrcChip(pair).cls]"
            :title="pairSrcChip(pair).tip"
          >{{ pairSrcChip(pair).text }}</span>
          <button type="button" class="link-btn" @click="reissuePair(pair.id)">重新出码</button>
          <span v-if="pair.revoked" class="gw-client-revoked">已吊销</span>
          <button type="button" class="link-btn" @click="deletePair(pair.id)">删除</button>
        </div>
      </div>
    </div>

    <!-- 连别的电脑卡（旧版是纯说明，入口指向「电脑列表 → 编辑电脑」） -->
    <div class="gw-card">
      <div class="gw-card-head"><h3>连别的电脑</h3></div>
      <p class="field-hint" style="margin-top: 0">
        出门时访问列表里的其他电脑：到「电脑列表 → 编辑电脑」粘贴对方的配对码，
        或点「从对方获取」在线拉取（对方需在同一局域网且本机服务在线，并会弹确认框）。
        对方在它自己的「远程接入」页生成配对码。
      </p>
    </div>
  </div>

  <PairDialog :open="pairOpen" @close="pairOpen = false; refreshAll()" />

  <!-- 卸载确认（旧版 #gwUniOverlay，hub.html:3294）：勾上「保留本机数据」只删程序 -->
  <div v-if="uninstallOpen" id="gwUniOverlay" class="token-modal-overlay" @click.self="uninstallOpen = false">
    <div id="gwUniDialog" class="token-modal gw-uni-modal" role="dialog" aria-modal="true" aria-labelledby="gwUniTitle">
      <h2 id="gwUniTitle">卸载</h2>
      <p id="gwUniWhat" class="gw-pair-warn">{{ uninstallWhat }}</p>
      <label class="gw-uni-keep">
        <input id="gwUniKeep" v-model="keepData" type="checkbox" />
        <span>
          <b>保留本机数据</b>（登录凭据 / 配置 / 状态目录）
          <span id="gwUniKeepHint" class="field-hint" style="display: block; margin-top: 3px">{{ keepHint }}</span>
        </span>
      </label>
      <p id="gwUniWarn" class="field-hint" style="margin: 10px 0 0">
        不勾选（默认）= 连同本机登录凭据一起清掉，下次安装或重开需要重新授权。
      </p>
      <div style="display: flex; gap: 8px; justify-content: flex-end; margin-top: 16px">
        <button type="button" id="gwUniCancel" class="btn" @click="uninstallOpen = false">取消</button>
        <button type="button" id="gwUniOk" class="btn primary" @click="confirmUninstall(keepData)">确认卸载</button>
      </div>
    </div>
  </div>

  <!-- 三个日志弹层（旧版 #gwLogOverlay / #tsLogOverlay / #tsnLogOverlay 同构） -->
  <LogDialog
    :open="logOpen"
    id-prefix="gwLog"
    title="通道日志"
    :lines="logs"
    @copy="copyText"
    @clear="clearLogs"
    @close="logOpen = false; stopLogTimer()"
  />
  <LogDialog
    :open="tsLogOpen"
    id-prefix="tsLog"
    title="Tailscale 日志"
    :lines="tsLogs"
    @copy="copyText"
    @clear="clearTsLogs"
    @close="tsLogOpen = false; stopLogTimer()"
  />
  <LogDialog
    :open="tsnLogOpen"
    id-prefix="tsnLog"
    button-style="modal"
    title="内置引擎日志"
    :lines="tsnLogs"
    @copy="copyText"
    @clear="clearTsnLogs"
    @close="tsnLogOpen = false; stopLogTimer()"
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
