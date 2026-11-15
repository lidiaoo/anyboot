<script setup lang="ts">
const toast = useToastStore();
const ui = useUiStateStore();
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { t } from "@/i18n";
import { call, isTauri } from "@/api/tauri";
import PairDialog from "@/components/hub/PairDialog.vue";
import LogDialog from "@/components/hub/LogDialog.vue";
import { useToastStore } from "@/stores/toast";
import { useEscape } from "@/composables/useEscape";
import { useUiStateStore } from "@/stores/uiState";

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
  named_url: string;
  named_token_set: boolean;
  autostart: boolean;
  binary: { installed: boolean; version: string; path: string; source: string };
  tunnel: { running: boolean; mode: string; url: string; state: string; error: string };
}

interface TailscaleStatus {
  installed: boolean;
  /** running / needs_login / stopped / needs_gui / needs_reboot / needs_approval / unknown */
  state?: string;
  ip: string;
  /** MagicDNS 全名（已去尾点） */
  dnsName?: string;
  version: string;
  cliPath?: string;
  /** tailscaled 系统服务：1=运行中 / 0=已停止 / -1=查不了（macOS 恒 -1） */
  serviceRunning?: number;
  /** 是否已保存 AuthKey（密钥本身不回传） */
  authkeySet?: boolean;
  /** windows / macos / linux */
  platform?: string;
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

/**
 * 拆掉 HTTP 桥的 `{"Ok": …}` 包装。
 * 网页/手机端走 `/api/ui-command`，返回 `Result<T,String>` 的命令（funnel enable/disable、
 * toggle 等）在 JSON 里会多包一层 `{"Ok":{…}}`，Tauri IPC 则直接给内层值 —— 不统一拆掉，
 * 网页端就把 `{Ok:{state:'ready'}}` 当成状态对象，字段全 undefined（表现为「点了没反应」）。
 */
function unwrapOk<T>(value: unknown): T | null {
  if (value && typeof value === "object" && "Ok" in (value as Record<string, unknown>)) {
    const inner = (value as { Ok: T }).Ok;
    return (inner ?? null) as T | null;
  }
  if (value && typeof value === "object" && "Err" in (value as Record<string, unknown>)) {
    return null;
  }
  return (value ?? null) as T | null;
}

const activeTab = ref<TabKey>("cf");
// 旧版 ab-hub-gwtab：记住上次所在的远程接入分页
try { activeTab.value = (localStorage.getItem("ab-hub-gwtab") || "cf") as typeof activeTab.value; } catch { /* 忽略 */ }
watch(activeTab, (name) => { try { localStorage.setItem("ab-hub-gwtab", name); } catch { /* 忽略 */ } });
const config = ref<GatewayConfig | null>(null);
const tsStatus = ref<TailscaleStatus | null>(null);
const pairs = ref<PairRow[]>([]);
const busy = ref("");
/** 手动指定 cloudflared 路径（旧版 #gwManualRow 的开合） */
const manualOpen = ref(false);
const manualPath = ref("");

/**
 * 后端 `gateway_tailscale_status` 只有 `state` + `serviceRunning`，从来没有 `running` 字段
 * （旧版 hub.html:13402 是拿 `state === 'running' && ip` 现算的）。移植时误信了不存在的
 * `running`，整张卡就永远停在「未连接」——主开关永远拨不上去、「服务」开关永远走 stop、
 * 地址行永不出现、4 个自救卡片（停止/GUI/待重启/待允许）被写死隐藏。
 */
const tsState = computed(() => tsStatus.value?.state ?? "");
/** 真·连上：装了 + state=running + 拿到 100.x 地址（旧版同一判据） */
const tsRunning = computed(
  () => Boolean(tsStatus.value?.installed) && tsState.value === "running" && Boolean(tsStatus.value?.ip),
);
/** 副开关「服务」的数据源：serviceRunning >= 0 才代表这个平台查得到服务 */
const tsSvcKnown = computed(() => {
  const n = tsStatus.value?.serviceRunning;
  return Boolean(tsStatus.value?.installed) && typeof n === "number" && n >= 0;
});
const tsSvcOn = computed(() => tsSvcKnown.value && tsStatus.value?.serviceRunning === 1);
/** 服务已确认没跑 + 状态未知 → 主开关拨上去也没意义（后端会直接失败） */
const tsSvcDead = computed(() => tsState.value === "unknown" && tsStatus.value?.serviceRunning === 0);
/** 这三种状态下 CLI 根本起不来，主开关必须禁用，否则点了只会报错 */
const tsSwLocked = computed(() =>
  ["needs_gui", "needs_reboot", "needs_approval"].includes(tsState.value),
);
/** 主开关忙碌：等授权期间保持「开」，别被自动刷新弹回去（旧版 tsUpIntent） */
const tsBusy = ref(false);
const tsUpIntent = ref(false);
const tsSvcBusy = ref(false);

/** Tailscale 状态文案（旧版 tsStateText 的 8 个分支） */
const tsStateText = computed(() => {
  const st = tsStatus.value;
  if (!st) return t("gw.tsDetecting");
  if (!st.installed) return t("gw.tsNotInstalled");
  if (tsRunning.value) return t("gw.tsRunning") + (st.dnsName ? " · " + st.dnsName : "");
  switch (tsState.value) {
    case "needs_login":
      return t("gw.tsNeedLogin");
    case "stopped":
      return t("gw.tsStopped");
    case "needs_gui":
      return t("gw.tsAppNotRunning");
    case "needs_reboot":
      return t("gw.tsNeedReboot");
    case "needs_approval":
      return t("gw.tsNetExt");
    default:
      return st.serviceRunning === 0 ? t("gw.tsDaemonDown") : t("gw.tsUnknown");
  }
});

/** 未安装态说明（mac 是 .pkg 系统安装器要管理员授权，win 是 setup.exe，linux 是 tarball） */
const tsDlHint = computed(() =>
  tsStatus.value?.platform === "macos"
    ? t("gw.tsMissingInstall")
    : t("gw.tsMissingDmg"),
);

/** 已保存 AuthKey 时把提示换成「留空不动 / 清空即删除」，避免用户以为要重填 */
const tsKeyPlaceholder = computed(() =>
  tsStatus.value?.authkeySet ? t("gw.tsAuthkeySavedField") : t("gw.tsAuthkeyEmptyField"),
);
const tsKeyHint = computed(() =>
  tsStatus.value?.authkeySet
    ? t("gw.tsAuthkeySavedHint") : t("gw.tsAuthkeyHint"),
);
/** 登录卡片可见性：待登录恒显示；拿到链接后也留着（up 阻塞期间状态还没转成 needs_login） */
const tsLoginVisible = computed(
  () => Boolean(tsStatus.value?.installed) && (tsState.value === "needs_login" || Boolean(loginUrl.value)),
);

/** Tailscale 安装包下载进度（旧版 #tsProgressRow） */
const tsDownloading = ref(false);
const tsProgress = ref(0);

/** 本机 HTTP 服务端口（旧版 localEndpoint.port；拿不到就按默认 12300 拼地址） */
const httpPort = ref(12300);
/** 局域网直连地址：只有真连上才给（旧版 tsAddr，端口弱化显示） */
const tsAddr = computed(() => (tsRunning.value ? `http://${tsStatus.value?.ip}` : ""));
/** 本机服务自检（旧版 tsHealth：防 12300 被保留段占用导致静默 bind 失败） */
const tsHealthOk = ref<boolean | null>(null);
async function tsHealth() {
  tsHealthOk.value = null;
  try {
    await call<boolean>("local_service_health");
    tsHealthOk.value = true;
  } catch {
    tsHealthOk.value = false;
  }
}

/** Funnel 公网通道状态（旧版 #tsfBlock，4 种状态：off/starting/needs_authorize/ready/error） */
interface TsfStatus {
  state: string;
  url?: string;
  /** 人话说明（失败原因 / 授权提示）—— 后端字段名是 message，不是 error */
  message?: string;
  /** 官方授权页（needs_authorize 时的 setupUrl，白名单 tailscale.com/s/*） */
  setupUrl?: string;
  errorCode?: string;
}
const tsfStatus = ref<TsfStatus | null>(null);
/** 开/关进行中（旧版 tsfBusy：开关转 loading、state 显示成 starting） */
const tsfBusy = ref(false);
/** 实际展示的状态：忙碌期一律 starting，否则取后端值 */
const tsfState = computed(() => (tsfBusy.value ? "starting" : (tsfStatus.value?.state ?? "off")));
const tsfStateText = computed(() => {
  const st = tsfStatus.value;
  const state = tsfState.value;
  if (state === "starting") return t("gw.funnelStarting");
  if (state === "needs_authorize") return st?.message || t("gw.funnelNeedAuth");
  if (state === "ready") return st?.url ? t("gw.funnelReady") : t("gw.funnelReadyNoUrl") + (st?.message ? ": " + st.message : "");
  if (state === "unavailable") return st?.message || t("gw.funnelUnavailable");
  if (state === "error") return t("gw.funnelError") + (st?.message ? ": " + st.message : "");
  return t("gw.funnelOff");
});

/** 命名隧道配置（旧版 #gwNamedToken / #gwNamedUrl） */
const namedToken = ref("");
const namedTokenSet = ref(false);
const namedUrl = ref("");

// 回填命名隧道表单（旧版 renderRemote：named_url + named_token_set 占位/LogoutRow）
watch(config, (cfg) => {
  if (!cfg) return;
  namedUrl.value = cfg.named_url || "";
  namedTokenSet.value = Boolean(cfg.named_token_set);
});

/** 自启开关（旧版 #gwAutostart） */
function toggleAutostart() {
  const next = !config.value?.autostart;
  void run(
    next ? t("gw.autostartOn") : t("gw.autostartOff"),
    () => call("gateway_set_autostart", { channel: "tunnel", enabled: next }),
    next ? t("gw.autostartDoneOn") : t("gw.autostartDoneOff"),
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
  /** 内置引擎登录链接（后端从 tsnet 输出抠出的 login.tailscale.com 链接），序列化 loginUrl */
  loginUrl?: string;
}
const tsnStatus = ref<TsnStatus | null>(null);
const tsnKey = ref("");
/** 自启（旧版 #tsnAutostart） */
const tsnAutostart = ref(Boolean((ui.settings as unknown as { tsnetAutostart?: boolean })?.tsnetAutostart));

const tsnRunning = computed(
  () => tsnStatus.value?.state === "ready" || tsnStatus.value?.state === "needs_auth",
);

const tsnStateText = computed(() => {
  const s = tsnStatus.value?.state;
  if (s === "needs_auth") return t("gw.tsnetNeedAuth");
  if (s === "ready") return t("gw.tsnetReady");
  if (s === "error") return t("gw.tsnetError");
  return t("gw.tsnetOff");
});

/** 开关引擎（旧版 #tsnSw change） */
function tsnToggle() {
  if (tsnRunning.value) tsnDisable();
  else tsnEnable();
}

/** 去开通 Funnel（旧版 #tsnSetupBtn，由 sidecar 给的 setupUrl） */
/** 打开 Tailscale 登录页（旧版校验 URL 必须是 login.tailscale.com） */
function copyLoginUrl() {
  const url = loginLink();
  if (!url) { toast.show(t("gw.noLoginLink"), "bad"); return; }
  const done = () => toast.show(t("gw.loginLinkCopied"));
  try {
    if (navigator.clipboard && window.isSecureContext) { void navigator.clipboard.writeText(url).then(done).catch(() => toast.show("复制失败", "bad")); return; }
  } catch { /* fallthrough */ }
  try { const ta=document.createElement("textarea"); ta.value=url; ta.style.position="fixed"; ta.style.opacity="0"; document.body.appendChild(ta); ta.select(); document.execCommand("copy"); document.body.removeChild(ta); done(); } catch { toast.show("复制失败，请手动复制：" + url, "bad"); }
}
function regenLoginUrl() {
  // 旧版：把开关拨到「开」再走一遍 up（等同关开一次），拿新链接
  if (tsBusy.value) {
    toast.show(t("gw.busyOp"), "bad");
    return;
  }
  tsSetUp(true);
}

// 登录链接受 login.tailscale.com 白名单校验（旧版双保险）。内置引擎优先用后端
// tsnet_status 返回的 setupUrl（web/PC 都能拿，不依赖 Tauri 事件）；事件捕获的 loginUrl 作兜底。
function loginLink(): string {
  const cands = [tsnStatus.value?.loginUrl, loginUrl.value].filter(Boolean) as string[];
  return cands.find((u) => /^https:\/\/login\.tailscale\.com\//.test(u)) ?? "";
}
async function openLoginPage() {
  const url = loginLink();
  if (!url) { toast.show(t("gw.noValidLink"), "bad"); return; }
  const { isTauri } = await import("@/api/tauri");
  if (!isTauri()) { const w = window.open(url, "_blank"); if (!w) toast.show(t("hub.toast.popupBlocked"), "bad"); return; }
  void call("remote_open_page", { url }).catch((e) => toast.show("打开登录页失败：" + e, "bad"));
}

function tsnOpenSetup() {
  const url = tsnStatus.value?.setupUrl;
  if (!url) { toast.show(t("gw.invalidUrl"), "bad"); return; }
  void call("remote_open_page", { url }).catch((e) => toast.show("开通地址无效：" + e, "bad"));
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
  void run(t("gw.enableBuiltin"), () => call("tsnet_enable"), t("gw.enableBuiltinDone"));
}

function tsnDisable() {
  void run(t("gw.disableBuiltin"), () => call("tsnet_disable"), t("gw.disableBuiltinDone"));
}

function tsnLogout() {
  void run(t("gw.logoutBuiltin"), () => call("tsnet_logout"), t("gw.logoutBuiltinDone"));
}

function tsnUninstall() {
  void run(t("gw.uninstallBuiltin"), () => call("tsnet_uninstall"), t("gw.uninstallBuiltinDone"));
}

function tsnSaveKey() {
  void run(t("gw.saveKey"), () => call("tsnet_set_authkey", { key: tsnKey.value.trim() }), t("gw.saveKeyDone"));
}

/** Tailscale 自启（旧版 #tsAutostart） */
const tsAutostart = ref(Boolean((ui.settings as unknown as { tailscaleAutostart?: boolean })?.tailscaleAutostart));
watch(
  () => ui.settings,
  (s) => {
    const st = s as unknown as { tailscaleAutostart?: boolean; tsnetAutostart?: boolean } | null;
    if (st && typeof st.tailscaleAutostart === "boolean") tsAutostart.value = st.tailscaleAutostart;
    if (st && typeof st.tsnetAutostart === "boolean") tsnAutostart.value = st.tsnetAutostart;
  },
  { deep: true },
);

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
    message.value = t("gw.copied");
    toast.show(t("gw.copied"));
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
  void run(t("gw.downloadTs"), () => call("gateway_tailscale_download"), t("gw.dlStarted")).finally(() => {
    tsDownloading.value = false;
  });
}

/** 打开官网下载页（旧版 #tsManualLink → remote_open_page，window.open 只会开新 webview） */
function tsOpenManual() {
  void openExternal("https://tailscale.com/download");
}

/** macOS GUI 没常驻：拉起 Tailscale.app，等 4 秒自动重探（旧版 tsGuiLaunchTimer） */
let tsGuiTimer: number | null = null;
function tsLaunchGui() {
  void call("gateway_tailscale_launch_gui")
    .then(() => {
      toast.show(t("gw.openingTsApp"));
      if (tsGuiTimer !== null) window.clearTimeout(tsGuiTimer);
      tsGuiTimer = window.setTimeout(() => void refreshAll(), 4000);
    })
    .catch((e) => toast.show(String(e || "打开失败"), "bad"));
}

function tsSaveKey() {
  const key = tsKey.value.trim();
  void run(t("gw.saveKey"), () => call("gateway_tailscale_set_authkey", { key }), t("gw.saveKeyDone"));
}

/**
 * 副开关：启停 tailscaled 系统服务。
 * 判据必须是**复选框自己的状态**（旧版用 this.checked）—— 之前按 `running`（一个后端
 * 根本不存在的字段）算 action，永远是 stop，开关点了等于反着来。
 */
function tailscaleService(ev: Event) {
  const el = ev.target as HTMLInputElement;
  const on = el.checked;
  if (tsSvcBusy.value) {
    el.checked = !on;
    return;
  }
  tsSvcBusy.value = true;
  void call("gateway_tailscale_service", { action: on ? "start" : "stop" })
    .then(async () => {
      tsSvcBusy.value = false;
      toast.show(on ? t("gw.daemonStarted") : t("gw.daemonStopped"));
      await refreshAll();
    })
    .catch(async (e) => {
      tsSvcBusy.value = false;
      el.checked = !on;
      await refreshAll();
      toast.show(t("gw.svcFail", { msg: String(e) }), "bad");
    });
}

async function copyTsAddr() {
  const addr = tsRunning.value ? `${tsAddr.value}:${httpPort.value}` : "";
  if (!addr) {
    toast.show(t("gw.noCopyAddr"), "bad");
    return;
  }
  try {
    await navigator.clipboard.writeText(addr);
    toast.show(t("gw.tsAddrCopied", { addr }));
  } catch {
    toast.show(t("gw.copyFailPerm"), "bad");
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

/**
 * 从自由文本里抠出可点链接。
 *
 * 典型场景：点 Tailscale 直连的开关 → `tailscale up` 在 stderr 里打出
 * `To authenticate, visit: https://login.tailscale.com/a/xxxx` → 120 秒没人授权
 * 后整段文本被当成错误提示铺在页脚。以前那只是一行纯文字：链接不能点、
 * body 上的 `user-select: none` 又让人连复制都做不到，只能照着屏幕手敲。
 *
 * 尾部标点要剥掉（"…/a/abcd." 这种句号收尾很常见），否则点开会 404。
 */
const URL_RE = /https?:\/\/[^\s<>"'`）】]+/g;
function stripTrailingPunct(url: string): string {
  return url.replace(/[.,;:!?、。；：！？…"'”’]+$/, "");
}
function extractUrls(text: string): string[] {
  if (!text) return [];
  const out: string[] = [];
  for (const raw of text.match(URL_RE) ?? []) {
    const url = stripTrailingPunct(raw);
    if (url && !out.includes(url)) out.push(url);
  }
  return out;
}

/** 错误/提示行按「文本 / 链接」切段，链接单独渲染成 <a> */
const errorSegments = computed<{ text?: string; url?: string }[]>(() => {
  const text = errorText.value;
  if (!text) return [];
  const segs: { text?: string; url?: string }[] = [];
  let last = 0;
  for (const match of text.matchAll(URL_RE)) {
    const start = match.index ?? 0;
    const url = stripTrailingPunct(match[0]);
    if (!url) continue;
    if (start > last) segs.push({ text: text.slice(last, start) });
    segs.push({ url });
    last = start + url.length; // 被剥掉的尾标点留到下一段文本里
  }
  if (last < text.length) segs.push({ text: text.slice(last) });
  return segs;
});

/** 用系统浏览器打开外部链接（Tauri 走 remote_open_page，网页端开新窗） */
async function openExternal(url: string) {
  if (!/^https?:\/\//i.test(url)) return;
  const { isTauri } = await import("@/api/tauri");
  if (isTauri()) {
    void call("remote_open_page", { url }).catch((e) => toast.show("打开失败：" + e, "bad"));
    return;
  }
  const w = window.open(url, "_blank", "noopener");
  if (!w) toast.show(t("gw.popupBlockedNew"), "bad");
}

/**
 * 错误文本里出现 Tailscale 授权链接就顺手点亮登录卡片。
 * 事件通道（tailscale-login-url）只在 Tauri 里可用，网页端拿不到 —— 文本兜底。
 */
watch(errorText, (text) => {
  const url = extractUrls(text).find((u) => /^https:\/\/login\.tailscale\.com\//i.test(u));
  if (url) loginUrl.value = url;
});
/** 授权完成（连上了）就把旧链接收起来，避免一直挂着一张过期卡片 */
watch(tsRunning, (running) => {
  if (running) loginUrl.value = "";
});

/** 自动刷新（旧版 `.gw-ar` 三合一控件） */
const autoSec = ref(3);
/** gwAutoSec 的上一档值（Escape 还原用，旧版 gwAutoSec） */
const autoSecPrev = ref(3);
watch(autoSec, (v) => { if (Number.isFinite(v) && v >= 1) autoSecPrev.value = v; });
const autoRunning = ref(true);
/** 下拉菜单开合（旧版 #gwAutoMenu） */
const autoMenuOpen = ref(false);
useEscape(autoMenuOpen, () => { autoMenuOpen.value = false; });
let autoTimer: number | null = null;

const TABS: { key: TabKey; label: string; dot?: boolean }[] = [
  { key: "cf", label: "gw.tabCf", dot: true },
  { key: "ts", label: "Tailscale", dot: true },
  { key: "tsn", label: "gw.tabTsn", dot: true },
  { key: "pair", label: "gw.tabPair" },
];

const cfUrl = computed(() => config.value?.tunnel?.url ?? "");
const cfRunning = computed(() => Boolean(config.value?.tunnel?.running));
// 三个 tab 的状态点：真实反映各通道是否在运行（之前 tsn 分支漏接、永远 off）。
function tabDotState(key: string): "on" | "off" {
  if (key === "cf") return cfRunning.value ? "on" : "off";
  if (key === "ts") return tsRunning.value ? "on" : "off";
  if (key === "tsn") return tsnRunning.value ? "on" : "off";
  return "off";
}
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
  // 连上才做本机服务自检（旧版在每次渲染的 running 分支里调 tsHealth）
  if (tsRunning.value) void tsHealth();
  else tsHealthOk.value = null;
  // 内置引擎(Tailscale 内置)状态也要跟着自动刷新，否则登录后「需要登录/已启用」不会自己变
  await refreshTsn();
  await refreshFunnel();
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
    toast.show(t("gw.failOf", { label, msg: String(error) }), "bad");
  } finally {
    busy.value = "";
  }
}

onMounted(async () => {
  // 本机 HTTP 端口（Tailscale 直连地址要拼 :port；旧版取 localEndpoint.port）
  try {
    const ep = await call<{ port?: number }>("local_http_endpoint");
    if (ep && typeof ep.port === "number" && ep.port > 0) httpPort.value = ep.port;
  } catch {
    /* 拿不到就按默认 12300 */
  }
  // 【修复】浏览器/手机端(非 Tauri)：没有 tauri event/listen，原有的 listen 调用会抛错
  // 中断 onMounted → 后面 refreshAll 不执行 → 三个通道 dot 全灰。这里非 Tauri 直接拉数据。
  if (!isTauri()) {
    await refreshAll();
    await refreshFunnel();
    await refreshTsn();
    return;
  }
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
        ? t("gw.cloudflaredReady", { v: event.payload?.version ?? "" })
        : t("gw.dlFail", { msg: event.payload?.error ?? t("gw.unknownErr") });
      void refreshAll();
    }),
  );
  // 通道状态变化 → 重拉快照
  offs.push(await listen("gateway-tunnel-state", () => void refreshAll()));
  // Tailscale 安装进度 / 完成 / 登录链接
  offs.push(await listen("tailscale-progress", () => void refreshAll()));
  offs.push(
    await listen<{ ok?: boolean; path?: string; error?: string }>("tailscale-done", (event) => {
      const done = event.payload || {};
      tsDownloading.value = false;
      if (!done.ok) {
        toast.show(t("gw.tsDlFail", { msg: String(done.error || t("gw.unknownErr")) }));
        void refreshAll();
        return;
      }
      // 旧版：下载完成 → 自动启动安装器（需系统授权确认）
      toast.show(t("gw.dlDoneInstalling"));
      void call("gateway_tailscale_install", { path: done.path })
        .then(() => toast.show(t("gw.installerStarted")))
        .catch((error) => toast.show(`安装器启动失败：${error}，可手动运行 ${done.path}`));
      void refreshAll();
    }),
  );
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
    next ? t("gw.enableChannel") : t("gw.disableChannel"),
    () => call(
      next ? "gateway_tunnel_start" : "gateway_tunnel_stop",
      next
        ? { mode: config.value?.mode === "named" ? "named" : "quick", namedToken: namedToken.value, namedUrl: namedUrl.value }
        : undefined,
    ),
    next ? t("gw.channelEnabled") : t("gw.channelDisabled"),
  );
}

function downloadBinary() {
  void run(t("gw.downloadCloudflared"), () => call("gateway_binary_download"), t("gw.dlStarted"));
}

function setManualBinary() {
  const path = manualPath.value.trim();
  if (!path) return;
  void run(
    t("gw.customPath"),
    () => call("gateway_binary_set_manual", { path }),
    t("gw.customPathSet"),
  ).then(() => {
    manualOpen.value = false;
  });
}

/** 二进制状态 / 卸载 / 清除凭据 / 保存配置（旧版 gateway_* 系列） */

function uninstallBinary() {
  void run(t("gw.uninstallCloudflared"), () => call("gateway_binary_uninstall"), t("gw.uninstalled"));
}

function clearCredentials() {
  void run(t("gw.clearCred"), () => call("gateway_clear_credentials"), t("gw.credCleared"));
}

/** 保存通道配置（旧版 persistConfig，hub.html:13408） */
function persistConfig(mode: string) {
  void run(
    t("gw.saveConfig"),
    () =>
      call("gateway_save_config", {
        mode,
        namedUrl: namedUrl.value,
        namedToken: namedToken.value,
        autostart: Boolean(config.value?.autostart),
      }),
    t("gw.configSaved"),
  );
}

/** 切模式（旧版 selectMode(next, save=true)） */
function selectMode(next: "quick" | "named") {
  if (config.value) config.value.mode = next;
  persistConfig(next);
}


function tsLogout() {
  void run(t("gw.logoutTs"), () => call("gateway_tailscale_logout"), t("gw.loggedOut"));
}

function tsExtensionSettings() {
  void call("gateway_tailscale_open_extension_settings")
    .then(() => toast.show(t("gw.openedSysSettings")))
    .catch((e) => toast.show(String(e || "打开失败"), "bad"));
}

/**
 * 开 / 关 Funnel。
 * 旧版语义（hub.html:13846）三条必须保留，否则就是「点了没反应」：
 * 1. **把返回的状态存下来**（原来 `run()` 把结果丢了，卡片永远显示未开启）；
 * 2. 失败 / 未就绪时把开关弹回去，别留一个假的「开」；
 * 3. needs_authorize 时点开关直接去官方授权页（不是再跑一次 enable）。
 */
async function tsFunnelToggle(on: boolean, ev?: Event) {
  const el = ev?.target as HTMLInputElement | undefined;
  const revert = () => {
    if (el) el.checked = !on;
  };
  if (tsfBusy.value) {
    revert();
    return;
  }
  if (on && tsfStatus.value?.state === "needs_authorize") {
    revert();
    tsFunnelAuth();
    return;
  }
  tsfBusy.value = true;
  try {
    const raw = await call<unknown>(on ? "gateway_tailscale_funnel_enable" : "gateway_tailscale_funnel_disable");
    const st = unwrapOk<TsfStatus>(raw);
    if (st) tsfStatus.value = st;
    const state = tsfStatus.value?.state;
    if (on) {
      if (state === "ready") toast.show(tsfStatus.value?.url ? t("gw.funnelOnUrl", { url: tsfStatus.value.url }) : t("gw.funnelReady"));
      else if (state === "needs_authorize") {
        revert();
        toast.show(tsfStatus.value?.message || t("gw.funnelNeedAuthWeb"), "bad");
      } else {
        revert();
        toast.show(t("gw.funnelOnFail", { msg: tsfStatus.value?.message || t("gw.unknownErr") }), "bad");
      }
    } else if (state === "off") {
      toast.show(t("gw.funnelOffDone"));
    } else {
      revert();
      toast.show(t("gw.funnelOffFail", { msg: tsfStatus.value?.message || t("gw.unknownErr") }), "bad");
    }
  } catch (e) {
    revert();
    toast.show(t("gw.funnelToggleFail", { action: on ? t("gw.enableChannel") : t("gw.disableChannel"), msg: String(e) }), "bad");
  } finally {
    tsfBusy.value = false;
  }
}

/** 官方授权页（后端已白名单，前端再校验一次） */
function tsFunnelAuth() {
  const url = tsfStatus.value?.setupUrl ?? "";
  if (!/^https:\/\/tailscale\.com\/s\//.test(url)) {
    toast.show(t("gw.authUrlInvalid"), "bad");
    return;
  }
  void openExternal(url);
}

/** Funnel 当前状态（每次刷新都重新取；**必须回写**，否则卡片永远显示未开启） */
async function refreshFunnel() {
  try {
    const st = unwrapOk<TsfStatus>(await call("gateway_tailscale_funnel_status"));
    if (st && typeof st === "object") tsfStatus.value = st;
  } catch {
    /* 未安装 / 旧后端：保持现状，不打扰 */
  }
}

function tsUninstallNow() {
  void run(t("gw.uninstallTs"), () => call("gateway_tailscale_uninstall"), t("gw.uninstalled"));
}



/** 残留通道：沿用 / 清理多余 */
function adoptOrphan() {
  const pid = orphans.value[0]?.pid;
  if (!pid) { toast.show(t("gw.noOrphanAdopt"), "bad"); return; }
  void run(t("gw.adoptOrphan"), () => call("gateway_tunnel_adopt", { pid }), t("gw.adopted"));
}


function killOrphan() {
  const pid = orphans.value[0]?.pid;
  if (!pid) { toast.show(t("gw.noOrphanKill"), "bad"); return; }
  void run(t("gw.cleanOrphan"), () => call("gateway_tunnel_kill_orphan", { pid }), t("gw.cleanedOrphan"));
}

// ---- Tailscale ----
/**
 * 主开关：tailscale up / down（登录级）。
 * 旧版语义（hub.html:13529）三件不能省：
 * 1. 用复选框自己的状态当意图，不用「当前是否 running」反推（待登录时 running 恒 false，
 *    反推会导致「想断开却再 up 一次」）；
 * 2. 忙碌期间保持「开」（tsUpIntent），别被 3 秒自动刷新弹回去；
 * 3. 拨动即清掉上一轮的登录链接，等新事件（否则旧链接会一直挂着）。
 */
function toggleTailscale(ev: Event) {
  const el = ev.target as HTMLInputElement;
  const on = el.checked;
  if (tsBusy.value) {
    el.checked = !on;
    return;
  }
  tsSetUp(on, () => {
    el.checked = !on;
  });
}

/** up / down 的实际执行体（主开关与「重新生成链接」共用；旧版共用同一个 change 处理器） */
function tsSetUp(on: boolean, revert?: () => void) {
  tsBusy.value = true;
  tsUpIntent.value = on;
  loginUrl.value = "";
  if (on && tsState.value === "needs_login") toast.show(t("gw.tsLoginLaunching"));
  void call<TailscaleStatus>("gateway_tailscale_toggle", { up: on })
    .then(async (st) => {
      tsBusy.value = false;
      tsUpIntent.value = false;
      if (st && typeof st === "object") tsStatus.value = st;
      await refreshAll();
      toast.show(on ? t("gw.tsConnected") : t("gw.tsDisconnected"));
    })
    .catch(async (e) => {
      tsBusy.value = false;
      tsUpIntent.value = false;
      revert?.();
      await refreshAll();
      const msg = String(e);
      // 「待授权」不是失败：后端已把链接推进卡片，别用红色报错糊用户一脸
      if (/^待授权/.test(msg)) toast.show(msg.replace(/^待授权（需在浏览器完成登录）：/, t("gw.tsLoginNeededPrefix")));
      else toast.show(t("gw.tsToggleFail", { msg: String(msg) }), "bad");
    });
}

// ---- 配对 ----
/** 重新出码（旧版 openPairReissue → gateway_pair_reissue） */
function reissuePair(id: string) {
  void run(t("gw.reissue"), () => call("gateway_pair_reissue", { id }), t("gw.reissued"));
}

/** 配对行 meta 文案（旧版 renderPairClients） */
function pairMeta(pair: PairRow): string {
  const bits: string[] = [];
  if (pair.url) bits.push(t("gw.pairAddr", { url: pair.url.replace(/^https?:\/\//, "") }));
  bits.push(
    pair.revoked
      ? t("gw.revokedAt", { t: fmtTime(pair.createdAt) })
      : t("gw.lastSeenAt", { t: fmtTime(pair.lastSeen), t2: fmtTime(pair.createdAt) }),
  );
  return bits.join(" · ");
}

/** 来源 chip（旧版按 pair_kind 判定文案与 class） */
function pairSrcChip(pair: PairRow): { cls: string; text: string; tip: string } {
  const labels: Record<string, string> = {
    tunnel: "gw.kindTunnel",
    ts: "gw.kindTs",
    funnel: "gw.kindFunnel",
    tsnet: "gw.kindTsnet",
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
    text = t("gw.kindTunnel");
  } else {
    text = t("gw.noAddrShort");
  }
  return { cls, text, tip: text + (pair.url ? ` · ${pair.url}` : " · " + t("gw.noAddrAtIssue")) };
}

function fmtTime(ts?: number): string {
  if (!ts) return "—";
  return new Date(ts).toLocaleString("zh-CN", { hour12: false });
}



function revokePair(id: string) {
  void run(t("gw.revoke"), () => call("gateway_pair_revoke", { id }), t("gw.revoked"));
}

function deletePair(id: string) {
  void run(t("gw.deletePair"), () => call("gateway_pair_delete", { id }), t("gw.deletedPair"));
}
</script>

<template>
  <div class="page-head" style="display: flex; align-items: center; gap: 10px; flex-wrap: wrap">
    <h2>{{ $t("gw.title") }}</h2>

    <!-- 自动刷新三合一（旧版 #gwAuto） -->
    <div class="gw-ar" id="gwAuto" :title="$t('gw.autoRefreshTitle')">
      <span class="gw-ar-label">{{ $t("gw.autoRefresh") }}</span>
      <div class="gw-ar-combo">
        <input
          id="gwAutoSec"
          v-model.number="autoSec"
          type="number"
          min="1"
          max="3600"
          step="1"
          inputmode="numeric"
          :aria-label="$t('gw.arIntervalAria')"
          @change="setupAuto"
          @keydown.enter.prevent="($event.target as HTMLInputElement).blur()"
          @keydown.escape.prevent="autoSec = autoSecPrev; ($event.target as HTMLInputElement).blur()"
        />
        <span class="gw-ar-unit">{{ $t("gw.seconds") }}</span>
        <button
          type="button"
          id="gwAutoCaret"
          class="gw-ar-caret"
          :aria-label="$t('gw.pickIntervalAria')"
          aria-haspopup="listbox"
          :aria-expanded="autoMenuOpen"
          @click="autoMenuOpen = !autoMenuOpen"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M6 9l6 6 6-6" />
          </svg>
        </button>
        <!-- 常用刷新间隔（旧版 #gwAutoMenu） -->
        <div v-if="autoMenuOpen" id="gwAutoMenu" class="gw-ar-menu" role="listbox" :aria-label="$t('gw.intervalAria')">
          <button
            v-for="sec in [1, 3, 5, 10, 30]"
            :key="sec"
            type="button"
            role="option"
            :aria-selected="autoSec === sec"
            @click="autoSec = sec; autoMenuOpen = false; setupAuto()"
          >
            {{ $t("gw.secSuffix", { n: sec }) }}
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
        <span id="gwAutoToggleText">{{ autoRunning ? $t("gw.pause") : $t("gw.resume") }}</span>
      </button>
    </div>
  </div>

  <!-- 分页标签（旧版 #gwTabs，4 个分页带状态点） -->
  <div class="gw-tabs" id="gwTabs" role="tablist" :aria-label="$t('gw.tabsAria')">
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
        :data-st="tabDotState(tab.key)"
      />
      {{ $t(tab.label) }}
    </button>
  </div>

  <!-- ===== Cloudflare 分页 ===== -->
  <div v-show="activeTab === 'cf'" class="gw-tabpane">
    <div class="gw-note" id="gwWarnQuick" :hidden="config?.mode === 'named'">
      {{ $t("gw.hintQuickTunnel") }}
    </div>

    <div class="gw-card">
      <div class="gw-card-head">
        <h3>{{ $t("gw.localChannel") }}</h3>
        <div class="segmented" id="gwModeTabs" role="tablist" :aria-label="$t('gw.modeTabsAria')">
          <button role="tab" data-mode="quick" :aria-selected="config?.mode !== 'named'" @click="selectMode('quick')">
            {{ $t("gw.quickTunnel") }}
          </button>
          <button role="tab" data-mode="named" :aria-selected="config?.mode === 'named'" @click="selectMode('named')">
            {{ $t("gw.namedTunnel") }}
          </button>
        </div>
        <button v-if="cfInstalled" type="button" id="gwUninstallBtn" class="btn" :disabled="!!busy" @click="uninstallBinary">{{ $t("gw.uninstall") }}</button>
        <button type="button" id="gwLogBtn" class="btn" @click="openGwLog">{{ $t("gw.log") }}</button>
        <button type="button" id="gwRefreshBtn" class="btn" @click="refreshAll">{{ $t("gw.redetect") }}</button>
        <span class="gw-sw-group" :title="$t('gw.channelToggleTitle')">
          <span class="gw-sw-cap">{{ $t("gw.channel") }}</span>
          <label class="switch" :class="{ loading: !!busy }" id="gwToggleWrap">
            <input type="checkbox" id="gwToggleSw" :checked="cfRunning || !!busy" :disabled="!!busy" @change="toggleCf" />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
      </div>

      <div class="gw-status">
        <span class="pulse" id="gwPulse" :hidden="!cfRunning" />
        <span id="gwStateText">{{ cfRunning ? $t("gw.channelOn") : $t("gw.channelOff") }}</span>
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
          {{ $t("common.copy") }}
        </button>
        <button type="button" id="gwPairBtn" class="btn" :hidden="!cfUrl" @click="pairOpen = true">
          {{ $t("gw.genPairAddr") }}
        </button>
        <span class="mono gw-meta" id="gwMeta">{{ config?.tunnel?.state || "" }}</span>
      </div>
      <div id="gwErrRow" class="gw-err" :hidden="!config?.tunnel?.error">{{ config?.tunnel?.error }}</div>

      <!-- 残留通道（旧版 #gwOrphanBox）：沿用或终止 -->
      <div id="gwOrphanBox" :hidden="!orphanRunning" style="margin-top: 12px">
        <p class="field-hint" style="margin: 0 0 6px">
          {{ $t("gw.hintOrphan") }}
        </p>
        <div id="gwOrphanList" class="gw-clients">
          <div class="gw-client">
            <span class="mono">{{ config?.tunnel?.url || $t("gw.noAddr") }}</span>
            <span style="flex: 1" />
            <button type="button" class="link-btn" @click="adoptOrphan">{{ $t("gw.adopt") }}</button>
            <button type="button" class="link-btn" :disabled="!!busy" @click="killOrphan">{{ $t("gw.terminate") }}</button>
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
            :placeholder="namedTokenSet ? $t('gw.namedTokenPhSaved') : $t('gw.namedTokenPh')"
            autocomplete="off"
          />
        </div>
        <div class="field">
          <label for="gwNamedUrl">{{ $t("gw.namedUrl") }}</label>
          <input
            id="gwNamedUrl"
            v-model="namedUrl"
            type="text"
            class="mono"
            placeholder="https://anyboot.example.com"
            autocomplete="off"
          />
          <p class="field-hint">{{ $t("gw.namedUrlHint") }}</p>
        </div>
      </div>

      <label class="gw-auto">
        <input type="checkbox" id="gwAutostart" :checked="config?.autostart" @change="toggleAutostart" />
        <span class="gw-auto-t">{{ $t("gw.autoStartCloudflared") }}</span>
        <span class="gw-auto-h">{{ $t("gw.autoStartCloudflaredHint") }}</span>
      </label>

      <div class="gw-binary">
        <span id="gwBinaryText">
          {{
            !config
              ? $t("gw.cfChecking")
              : cfInstalled
                ? $t("gw.cfReady", { v: config?.binary?.version || "" })
                : $t("gw.cfNotReady")
          }}
        </span>
        <span style="flex: 1" />
        <button v-if="!cfInstalled" type="button" id="gwDownloadBtn" class="btn" :disabled="!!busy" @click="downloadBinary">
          {{ $t("gw.downloadCloudflared") }}
        </button>
        <button v-if="!cfInstalled" type="button" id="gwManualBtn" class="btn" @click="manualOpen = !manualOpen">
          {{ $t("gw.specifyLocalPath") }}
        </button>
        <button v-if="cfInstalled" type="button" class="btn" :disabled="!!busy" @click="uninstallBinary">{{ $t("gw.uninstall") }}</button>
      </div>

      <!-- 清除登录凭据行（旧版 #gwLogoutRow：仅命名隧道 token 存在时显示） -->
      <div v-if="namedTokenSet" id="gwLogoutRow" class="gw-logout-row">
        <button type="button" class="link-btn" id="gwLogoutBtn" :disabled="!!busy" @click="clearCredentials">{{ $t("gw.clearCredentials") }}</button>
        <span class="field-hint">{{ $t("gw.clearCredHint") }}</span>
      </div>
      <!-- 下载进度（旧版 #gwProgressRow） -->
      <div id="gwProgressRow" :hidden="!downloading" class="gw-progress">
        <div class="gw-progress-bar"><div id="gwProgressBar" :style="{ width: progress + '%' }" /></div>
        <span class="mono" id="gwProgressText">{{ downloading ? $t("gw.downloading", { n: progress }) : "" }}</span>
      </div>

      <!-- 清除登录凭据（旧版 #gwLogoutRow）：命名隧道的 Tunnel Token 就是登录态 -->
      <div
        id="gwLogoutRow"
        :hidden="config?.mode !== 'named'"
        style="display: flex; gap: 8px; align-items: center; margin-top: 10px; flex-wrap: wrap"
       v-if="namedTokenSet">
        <button type="button" id="gwLogoutBtn" class="link-btn" @click="clearCredentials">{{ $t("gw.clearCredentials") }}</button>
        <span class="field-hint" style="margin: 0">
          {{ $t("gw.clearCredHint") }}
        </span>
      </div>

      <!-- 指定本地路径（旧版 #gwManualRow） -->
      <div id="gwManualRow" :hidden="!manualOpen" class="gw-binary">
        <input
          id="gwManualPath"
          v-model="manualPath"
          type="text"
          class="mono"
          :placeholder="$t('gw.cloudflaredPathPh')"
          spellcheck="false"
        />
        <button type="button" id="gwManualCancel" class="btn" @click="manualOpen = false">{{ $t("common.cancel") }}</button>
        <button
          type="button"
          id="gwManualOk"
          class="btn primary"
          :disabled="!manualPath.trim() || !!busy"
          @click="setManualBinary"
        >
          {{ $t("gw.confirm") }}
        </button>
      </div>
    </div>
  </div>

  <!-- ===== Tailscale 分页（旧版 hub.html:2931–3129，65 个 id） ===== -->
  <div v-show="activeTab === 'ts'" class="gw-tabpane">
    <div class="gw-card" id="tsCard">
      <div class="gw-card-head">
        <h3>{{ $t("gw.tsDirect") }}</h3>
        <span class="tsf-badge" id="tsBadge" :hidden="!tsRunning">{{ $t("gw.tsBadge") }}</span>
        <span style="flex: 1" />
        <span
          class="gw-sw-group"
          id="tsMainSwGroup"
          :hidden="!tsStatus?.installed"
          :title="$t('gw.connectToggleTitle')"
        >
          <span class="gw-sw-cap">{{ $t("gw.connect") }}</span>
          <label class="switch" id="tsMainSwWrap" :class="{ loading: tsBusy }">
            <input
              type="checkbox"
              id="tsMainSw"
              :checked="tsRunning || (tsBusy && tsUpIntent)"
              :disabled="tsBusy || tsSvcDead || tsSwLocked"
              @change="toggleTailscale"
            />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
        <span
          class="gw-sw-group"
          id="tsSvcSwGroup"
          :hidden="!tsSvcKnown"
          :title="$t('gw.serviceToggleTitle')"
        >
          <span class="gw-sw-cap">{{ $t("gw.service") }}</span>
          <label class="switch" id="tsSvcSwWrap" :class="{ loading: tsSvcBusy }">
            <input type="checkbox" id="tsSvcSw" :checked="tsSvcOn" :disabled="tsSvcBusy || tsBusy" @change="tailscaleService" />
            <span class="trk" aria-hidden="true" />
          </label>
        </span>
        <button v-if="tsStatus?.installed" type="button" class="btn" id="tsUninstallBtn" @click="openUninstall($t('gw.tsUninstallTitle'), $t('gw.tsUninstallNote'))">
          {{ $t("gw.uninstall") }}
        </button>
        <button type="button" class="btn" id="tsLogBtn" @click="() => openTsLog()">{{ $t("gw.log") }}</button>
        <button type="button" class="btn" id="tsRefreshBtn" @click="refreshAll">{{ $t("gw.redetect") }}</button>
      </div>

      <div class="gw-status" id="tsStateRow">
        <span class="pulse" id="tsPulse" :hidden="!tsRunning" />
        <span id="tsStateText">{{ tsStateText }}</span>
        <span class="mono gw-meta" id="tsMeta">{{ tsStatus?.version ? "CLI " + tsStatus.version : "" }}</span>
      </div>

      <!-- 未安装态：#tsDlRow -->
      <div id="tsDlRow" :hidden="tsStatus?.installed">
        <p class="field-hint" id="tsDlHint" style="margin: 8px 0 0">{{ tsDlHint }}</p>
        <div class="gw-binary">
          <span id="tsDlText">{{ $t("gw.pkgNotDownloaded") }}</span>
          <span style="flex: 1" />
          <button type="button" class="btn" id="tsDownloadBtn" :disabled="!!busy" @click="tsDownload">{{ $t("gw.downloadTs") }}</button>
          <button type="button" class="btn" id="tsManualLink" @click="tsOpenManual">{{ $t("gw.openManual") }}</button>
        </div>
        <div class="gw-progress" id="tsProgressRow" :hidden="!tsDownloading">
          <div class="gw-progress-bar"><div id="tsProgressBar" :style="{ width: tsProgress + '%' }" /></div>
          <span class="mono" id="tsProgressText">{{ tsDownloading ? $t("gw.downloading", { n: tsProgress }) : "" }}</span>
        </div>
      </div>

      <!-- 未连接态：#tsKeyRow（AuthKey 配置） -->
      <div id="tsKeyRow" :hidden="!tsStatus?.installed || tsRunning">
        <div class="field" style="margin-top: 10px">
          <label for="tsKey">{{ $t("gw.authKeyLabel") }}</label>
          <div style="display: flex; gap: 8px; align-items: center">
            <input
              id="tsKey"
              v-model="tsKey"
              type="password"
              class="mono"
              style="flex: 1"
              :placeholder="tsKeyPlaceholder"
              autocomplete="off"
            />
            <button type="button" class="btn" id="tsKeySave" :disabled="!!busy" @click="tsSaveKey">{{ $t("common.save") }}</button>
          </div>
          <p class="field-hint" id="tsKeyHint">{{ tsKeyHint }}</p>
        </div>
      </div>

      <!-- 需登录态：#tsLoginRow（拿到授权链接就显示；事件没到也能从错误文本兜底） -->
      <div id="tsLoginRow" :hidden="!tsLoginVisible">
        <p class="field-hint" style="margin: 8px 0 0">
          {{ $t("gw.hintNeedLoginSys") }}
          {{ $t("gw.authDoneNote") }}
        </p>
        <div style="display: flex; gap: 8px; align-items: center; margin-top: 8px">
          <button type="button" class="btn primary" id="tsLoginOpen" @click="openLoginPage">{{ $t("gw.openLoginPage") }}</button>
          <button type="button" class="link-btn" id="tsLoginCopy" @click="copyLoginUrl">{{ $t("gw.copyLoginUrl") }}</button>
          <button type="button" class="link-btn" id="tsLoginRegen" @click="regenLoginUrl">{{ $t("gw.regenerateLink") }}</button>
        </div>
        <p class="field-hint mono" id="tsLoginUrl" style="margin: 8px 0 0; user-select: all; overflow-wrap: anywhere; word-break: break-all">
          <a :href="loginUrl" class="gw-url" :title="$t('gw.openInBrowser', { url: loginUrl })" @click.prevent="openExternal(loginUrl)">{{ loginUrl }}</a>
        </p>
        <p class="field-hint" style="margin: 6px 0 0">
          {{ $t("gw.hintLinkValidSys") }}
          {{ $t("gw.hintLinkCopySys") }}
        </p>
      </div>

      <!-- 运行中态：#tsAddrRow -->
      <div id="tsAddrRow" :hidden="!tsRunning">
        <div class="gw-status" style="margin-top: 8px">
          <span class="field-hint" style="margin: 0">{{ $t("gw.tsLanAddr") }}</span>
          <span class="mono ts-addr" id="tsAddr">{{ tsAddr }}<span class="ts-port">:{{ httpPort }}</span></span>
          <span style="flex: 1" />
          <button type="button" class="link-btn" id="tsCopyAddr" @click="copyTsAddr">{{ $t("common.copy") }}</button>
          <button type="button" class="btn" id="tsPairBtn" @click="pairOpen = true">{{ $t("gw.genPairAddr") }}</button>
        </div>
        <div class="gw-status" style="margin-top: 6px">
          <span
            id="tsHealthDot"
            :style="{
              display: 'inline-flex',
              width: '8px',
              height: '8px',
              borderRadius: '50%',
              flex: 'none',
              background: tsHealthOk === true ? 'rgb(var(--ok))' : tsHealthOk === false ? 'rgb(var(--warn))' : 'rgb(var(--fg-faint))',
            }"
          />
          <span
            id="tsHealthText"
            :style="{ fontSize: '12px', color: tsHealthOk === false ? 'rgb(var(--warn))' : 'rgb(var(--fg-dim))' }"
          >{{
            tsHealthOk === true
              ? $t("gw.selfCheckOk", { port: httpPort })
              : tsHealthOk === false
                ? $t("gw.selfCheckFail")
                : $t("gw.selfCheckRunning", { port: httpPort })
          }}</span>
        </div>
        <p class="field-hint" style="margin-top: 8px; color: rgb(var(--warn))">
          {{ $t("gw.hintTsnetOnly") }}
        </p>
      </div>

      <!-- 停止态：#tsStoppedRow -->
      <div id="tsStoppedRow" :hidden="!tsStatus?.installed || tsState !== 'stopped'">
        <p class="field-hint" style="margin: 8px 0 0">
          {{ $t("gw.hintTsStopped") }}
        </p>
      </div>

      <!-- GUI 未启动态：#tsGuiRow -->
      <div id="tsGuiRow" :hidden="!tsStatus?.installed || tsState !== 'needs_gui'">
        <p class="field-hint" style="margin: 8px 0 6px">
          {{ $t("gw.hintTsGuiDown") }}
        </p>
        <button type="button" class="btn" id="tsLaunchGuiBtn" @click="tsLaunchGui">{{ $t("gw.openTsApp") }}</button>
      </div>

      <!-- 网络扩展待重启态：#tsRebootRow -->
      <div id="tsRebootRow" :hidden="!tsStatus?.installed || tsState !== 'needs_reboot'">
        <p class="field-hint" style="margin: 8px 0 6px; color: rgb(var(--warn))">
          {{ $t("gw.hintTsOldExt") }}
        </p>
        <button type="button" class="btn" id="tsRebootRecheck" @click="refreshAll">{{ $t("gw.rebootRecheck") }}</button>
      </div>

      <!-- 网络扩展待授权态：#tsApprovalRow -->
      <div id="tsApprovalRow" :hidden="!tsStatus?.installed || tsState !== 'needs_approval'">
        <p class="field-hint" style="margin: 8px 0 6px; color: rgb(var(--warn))">
          {{ $t("gw.hintTsExtNotAllowed") }}
        </p>
        <button type="button" class="btn" id="tsApprovalBtn" @click="tsExtensionSettings">{{ $t("gw.openExtSettings") }}</button>
      </div>

      <!-- Funnel 公网通道区块：#tsfBlock（4 种状态） -->
      <div id="tsfBlock" :hidden="!tsStatus?.installed">
        <div class="tsf-sep">
          <div class="gw-card-head" style="margin: 0">
            <h3 style="font-size: 13px">{{ $t("gw.funnelPubTitle") }}</h3>
            <span class="tsf-badge" id="tsfBadge" :hidden="tsfState !== 'ready'">{{ $t("gw.funnelPubBadge") }}</span>
            <span style="flex: 1" />
            <span class="gw-sw-group" :title="$t('gw.funnelToggleTitle')">
              <label class="switch" id="tsfSwWrap" :class="{ loading: tsfBusy }">
                <input
                  type="checkbox"
                  id="tsfSw"
                  :checked="tsfState === 'ready'"
                  :disabled="tsfBusy || tsfState === 'unavailable'"
                  @change="tsFunnelToggle(($event.target as HTMLInputElement).checked, $event)"
                />
                <span class="trk" aria-hidden="true" />
              </label>
            </span>
          </div>
          <div class="gw-status" id="tsfStateRow" style="margin-top: 8px">
            <span class="pulse" id="tsfPulse" :hidden="tsfState !== 'starting'" />
            <span id="tsfStateText">{{ tsfStateText }}</span>
          </div>

          <!-- off 态说明 -->
          <p class="field-hint" id="tsfOffRow" :hidden="tsfState !== 'off'" style="margin: 8px 0 0">
            {{ $t("gw.hintFunnelPublish") }}
          </p>

          <!-- ready 态：公网地址行 -->
          <div id="tsfAddrRow" :hidden="tsfState !== 'ready'">
            <div class="gw-status" style="margin-top: 8px">
              <span class="field-hint" style="margin: 0">{{ $t("gw.publicAddr") }}</span>
              <span class="mono ts-addr" id="tsfAddr">{{ tsfStatus?.url || "" }}</span>
              <span style="flex: 1" />
              <button type="button" class="link-btn" id="tsfCopy" @click="copyText(tsfStatus?.url || '')">{{ $t("common.copy") }}</button>
              <button type="button" class="btn" id="tsfPairBtn" @click="pairOpen = true">{{ $t("gw.genPairAddr") }}</button>
            </div>
            <p class="field-hint" style="margin-top: 8px; color: rgb(var(--warn))">
              {{ $t("gw.funnelWarn") }}
            </p>
          </div>

          <!-- needs_authorize 态 -->
          <div id="tsfAuthRow" :hidden="tsfState !== 'needs_authorize'">
            <p class="field-hint" style="margin: 8px 0 0">
              {{ $t("gw.hintFunnelAuth") }}
            </p>
            <div style="display: flex; gap: 8px; align-items: center; margin-top: 8px">
              <button type="button" class="btn primary" id="tsfAuthBtn" @click="tsFunnelAuth">{{ $t("gw.goAuthPage") }}</button>
              <button type="button" class="link-btn" id="tsfRetryBtn" @click="tsFunnelToggle(true)">{{ $t("gw.retryAfterAuth") }}</button>
            </div>
          </div>

          <!-- error 态 -->
          <div id="tsfErrRow" :hidden="tsfState !== 'error'">
            <p class="field-hint" id="tsfErrText" style="margin: 8px 0 0">{{ tsfStatus?.message || "" }}</p>
            <button type="button" class="link-btn" id="tsfRetryBtn2" style="margin-top: 4px" @click="tsFunnelToggle(true)">{{ $t("gw.retry") }}</button>
          </div>
        </div>
      </div>

      <!-- 清除登录凭据：#tsLogoutRow -->
      <div id="tsLogoutRow" style="display: flex; gap: 8px; align-items: center; margin-top: 10px; flex-wrap: wrap">
        <button type="button" class="link-btn" id="tsLogoutBtn" @click="tsLogout">{{ $t("gw.clearCredentials") }}</button>
        <span class="field-hint" style="margin: 0">
          {{ $t("gw.hintTsLogout") }}
        </span>
      </div>

      <!-- 自启 -->
      <label class="gw-auto">
        <input type="checkbox" id="tsAutostart" :checked="tsAutostart" @change="toggleTsAutostart" />
        <span class="gw-auto-t">{{ $t("gw.autoConnectStartup") }}</span>
        <span class="gw-auto-h">{{ $t("gw.autoConnectStartupHint") }}</span>
      </label>
    </div>
  </div>

  <!-- ===== Tailscale（内置）分页（旧版 hub.html:3130–3225，27 个 id） ===== -->
  <div v-show="activeTab === 'tsn'" class="gw-tabpane">
    <div class="gw-card" id="tsnCard">
      <div class="gw-card-head">
        <h3>{{ $t("gw.tsnTitle") }}</h3>
        <span class="tsf-badge" id="tsnBadge" :hidden="tsnStatus?.state !== 'ready'">{{ $t("gw.tsnBadge") }}</span>
        <span style="flex: 1" />
        <span class="gw-sw-group" :title="$t('gw.tsnEngineTitle')">
          <span class="gw-sw-cap">{{ $t("gw.engine") }}</span>
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
        <button type="button" class="btn" id="tsnUninstallBtn" @click="tsnUninstall">{{ $t("gw.uninstall") }}</button>
        <button type="button" class="btn" id="tsnLogBtn" @click="() => openTsnLog()">{{ $t("gw.log") }}</button>
        <button type="button" class="btn" id="tsnRefreshBtn" @click="refreshTsn">{{ $t("gw.redetect") }}</button>
      </div>

      <div class="gw-status">
        <span class="pulse" id="tsnPulse" :hidden="!tsnRunning" />
        <span id="tsnStateText">{{ tsnStateText }}</span>
      </div>

      <!-- off 态：说明 + AuthKey 配置 -->
      <div id="tsnOffRow" :hidden="tsnStatus?.state !== 'off'">
        <p class="field-hint" style="margin: 8px 0 0">
          {{ $t("gw.hintTsnPublish") }}
        </p>
        <div class="field" style="margin-top: 10px">
          <label for="tsnKey">{{ $t("gw.authKeyLabel") }}</label>
          <div style="display: flex; gap: 8px; align-items: center">
            <input
              id="tsnKey"
              v-model="tsnKey"
              type="password"
              class="mono"
              style="flex: 1"
              :placeholder="$t('gw.tsAuthkeyEmptyField')"
              autocomplete="off"
            />
            <button type="button" class="btn" id="tsnKeySave" :disabled="!!busy" @click="tsnSaveKey">{{ $t("common.save") }}</button>
          </div>
          <p class="field-hint">
            {{ $t("gw.hintTsnAuthKey") }}
          </p>
        </div>
      </div>

      <!-- needs_auth 态 -->
      <div id="tsnAuthRow" :hidden="tsnStatus?.state !== 'needs_auth'">
        <p class="field-hint" style="margin: 8px 0 0">
          {{ $t("gw.hintTsnLogin") }}
        </p>
        <div style="display: flex; gap: 8px; align-items: center; margin-top: 8px">
          <button type="button" class="btn primary" id="tsnLoginBtn" @click="openLoginPage">{{ $t("gw.openLoginPage") }}</button>
          <button type="button" class="link-btn" id="tsnLoginCopy" @click="copyLoginUrl">{{ $t("gw.copyLoginUrl") }}</button>
          <button type="button" class="link-btn" id="tsnLoginRegen" @click="regenLoginUrl">{{ $t("gw.regenerateLink") }}</button>
        </div>
        <p class="field-hint mono" id="tsnLoginUrl" style="margin: 8px 0 0; user-select: all; overflow-wrap: anywhere; word-break: break-all">
          <a :href="loginUrl" class="gw-url" :title="$t('gw.openInBrowser', { url: loginUrl })" @click.prevent="openExternal(loginUrl)">{{ loginUrl }}</a>
        </p>
        <p class="field-hint" style="margin: 6px 0 0">
          {{ $t("gw.tsnLinkValid") }}
          {{ $t("gw.tsnLinkCopy") }}
        </p>
      </div>

      <!-- ready 态：公网地址行 -->
      <div id="tsnAddrRow" :hidden="tsnStatus?.state !== 'ready'">
        <div class="gw-status" style="margin-top: 8px">
          <span class="field-hint" style="margin: 0">{{ $t("gw.publicAddr") }}</span>
          <span class="mono ts-addr" id="tsnAddr">{{ tsnStatus?.url || "" }}</span>
          <span style="flex: 1" />
          <button type="button" class="link-btn" id="tsnCopy" @click="copyText(tsnStatus?.url || '')">{{ $t("common.copy") }}</button>
          <button type="button" class="btn" id="tsnPairBtn" @click="pairOpen = true">{{ $t("gw.genPairAddr") }}</button>
        </div>
        <p class="field-hint" style="margin-top: 8px; color: rgb(var(--warn))">
          ⚠️ 此地址公网可达，安全依赖配对令牌，请勿外传配对码。目标机睡眠后同样失联。
        </p>
      </div>

      <!-- error 态 -->
      <div id="tsnErrRow" :hidden="tsnStatus?.state !== 'error'">
        <p class="field-hint" id="tsnErrText" style="margin: 8px 0 0">{{ tsnStatus?.error || "" }}</p>
        <div style="display: flex; gap: 8px; align-items: center; margin-top: 6px; flex-wrap: wrap">
          <button type="button" class="link-btn" id="tsnRetryBtn" @click="refreshTsn">{{ $t("gw.retry") }}</button>
          <button type="button" class="link-btn" id="tsnSetupBtn" :hidden="!tsnStatus?.setupUrl" @click="tsnOpenSetup">{{ $t("gw.goEnableFunnel") }}</button>
        </div>
      </div>

      <!-- 清除登录凭据 -->
      <div style="display: flex; gap: 8px; align-items: center; margin-top: 10px; flex-wrap: wrap">
        <button type="button" class="link-btn" id="tsnLogoutBtn" @click="tsnLogout">{{ $t("gw.clearCredentials") }}</button>
        <span class="field-hint" style="margin: 0">
          {{ $t("gw.tsnLogoutHint") }}
        </span>
      </div>

      <!-- 自启 -->
      <label class="gw-auto">
        <input type="checkbox" id="tsnAutostart" :checked="tsnAutostart" @change="toggleTsnAutostart" />
        <span class="gw-auto-t">{{ $t("gw.autostartEngine") }}</span>
        <span class="gw-auto-h">{{ $t("gw.autostartEngineHint") }}</span>
      </label>
    </div>
  </div>

  <!-- ===== 配对管理分页 ===== -->
  <!-- ===== 配对管理分页（旧版 hub.html:3225–3252） ===== -->
  <div v-show="activeTab === 'pair'" class="gw-tabpane">
    <!-- 配对管理卡 -->
    <div class="gw-card">
      <div class="gw-card-head">
        <h3>{{ $t("gw.tabPair") }}</h3>
        <button type="button" class="btn primary" id="gwIssueBtn" @click="pairOpen = true">{{ $t("gw.genPairCode") }}</button>
      </div>
      <p class="field-hint" style="margin-top: 0">
        {{ $t("gw.pairIntro") }}
      </p>
      <div id="gwClientList" class="gw-clients">
        <div v-if="!pairs.length" class="gw-clients-empty">
          {{ $t("gw.pairNone") }}
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
          <button type="button" class="link-btn" @click="reissuePair(pair.id)">{{ $t("gw.reissue") }}</button>
          <span v-if="pair.revoked" class="gw-client-revoked">{{ $t("gw.revoked") }}</span>
          <button v-else type="button" class="link-btn" @click="revokePair(pair.id)">{{ $t("gw.revoke") }}</button>
          <button type="button" class="link-btn" @click="deletePair(pair.id)">{{ $t("gw.deletePair") }}</button>
        </div>
      </div>
    </div>

    <!-- 连别的电脑卡（旧版是纯说明，入口指向「电脑列表 → 编辑电脑」） -->
    <div class="gw-card">
      <div class="gw-card-head"><h3>{{ $t("gw.connectOther") }}</h3></div>
      <p class="field-hint" style="margin-top: 0">
        {{ $t("gw.connectOtherHint") }}
      </p>
    </div>
  </div>

  <PairDialog :open="pairOpen" @close="pairOpen = false; refreshAll()" />

  <!-- 卸载确认（旧版 #gwUniOverlay，hub.html:3294）：勾上「保留本机数据」只删程序 -->
  <div v-if="uninstallOpen" id="gwUniOverlay" class="token-modal-overlay" @click.self="uninstallOpen = false">
    <div id="gwUniDialog" class="token-modal gw-uni-modal" role="dialog" aria-modal="true" aria-labelledby="gwUniTitle">
      <h2 id="gwUniTitle">{{ $t("gw.uninstall") }}</h2>
      <p id="gwUniWhat" class="gw-pair-warn">{{ uninstallWhat }}</p>
      <label class="gw-uni-keep">
        <input id="gwUniKeep" v-model="keepData" type="checkbox" />
        <span>
          <b>{{ $t("gw.keepData") }}</b>
          <span id="gwUniKeepHint" class="field-hint" style="display: block; margin-top: 3px">{{ keepHint }}</span>
        </span>
      </label>
      <p id="gwUniWarn" class="field-hint" style="margin: 10px 0 0">
        {{ $t("gw.keepDataHint") }}
      </p>
      <div style="display: flex; gap: 8px; justify-content: flex-end; margin-top: 16px">
        <button type="button" id="gwUniCancel" class="btn" @click="uninstallOpen = false">{{ $t("common.cancel") }}</button>
        <button type="button" id="gwUniOk" class="btn primary" @click="confirmUninstall(keepData)">{{ $t("gw.confirmUninstall") }}</button>
      </div>
    </div>
  </div>

  <!-- 三个日志弹层（旧版 #gwLogOverlay / #tsLogOverlay / #tsnLogOverlay 同构） -->
  <LogDialog
    :open="logOpen"
    id-prefix="gwLog"
    :title="$t('gw.logChannel')"
    :lines="logs"
    @copy="copyText"
    @clear="clearLogs"
    @close="logOpen = false; stopLogTimer()"
  />
  <LogDialog
    :open="tsLogOpen"
    id-prefix="tsLog"
    :title="$t('gw.logTs')"
    :lines="tsLogs"
    @copy="copyText"
    @clear="clearTsLogs"
    @close="tsLogOpen = false; stopLogTimer()"
  />
  <LogDialog
    :open="tsnLogOpen"
    id-prefix="tsnLog"
    button-style="modal"
    :title="$t('gw.logTsn')"
    :lines="tsnLogs"
    @copy="copyText"
    @clear="clearTsnLogs"
    @close="tsnLogOpen = false; stopLogTimer()"
  />

  <p v-if="message" class="gw-msgline">{{ message }}</p>
  <!-- 错误/提示行：链接单独渲染成可点，整段允许鼠标选中复制（body 默认禁选） -->
  <p v-if="errorText" class="gw-errline">
    <template v-for="(seg, i) in errorSegments" :key="i">
      <a
        v-if="seg.url"
        :href="seg.url"
        class="gw-url"
        :title="$t('gw.openInBrowser', { url: seg.url })"
        @click.prevent="openExternal(seg.url)"
      >{{ seg.url }}</a>
      <template v-else>{{ seg.text }}</template>
    </template>
  </p>
</template>

<!--
  样式**不写在这里**：`.page-head` / `.gw-ar` / `.gw-tabs` / `.gw-tab` /
  `.gw-tabpane` / `.gw-note` / `.gw-card` / `.gw-card-head` / `.gw-sw-group` /
  `.switch` / `.gw-status` / `.gw-err` / `.gw-auto` / `.gw-binary` / `.gw-clients`
  全部由 `styles/vendor/hub-inline.css` 定义。
-->
