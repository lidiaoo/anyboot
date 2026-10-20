// 本页面有三个运行环境：
// 1. Tauri 主窗口：window.__TAURI__ 注入，命令/事件走 IPC（原路径，不变）；
// 2. 任意浏览器（含 hub「电脑」页 iframe、手机 WebView）：由内置 HTTP 服务
//    （默认 12300）托管 —— 命令走 POST /api/ui-command、状态走轮询
//    GET /api/ui-state（token 鉴权），设置改动**真正作用于本机应用**。
//    只有当页面由无后端的静态服务托管（探测 /health 失败）时才降级为纯预览。
// 判据不能只看 __TAURI__ 是否存在：withGlobalTauri 的初始化脚本会被 WebView2
// 注入**所有 frame**，hub「电脑」页的 iframe（http://127.0.0.1:12300）里也有
// __TAURI__ —— 走 IPC 会被 Tauri 的远程 origin ACL 全部拒绝（capabilities 没
// 配 remote.urls），页面就「任何数据都拿不到」。所以必须再认宿主 origin：
// 只有 Tauri 自己的 webview 顶层（tauri:// 或 http://tauri.localhost）才算 IPC 模式。
const IS_TAURI_HOST =
  location.protocol === "tauri:" || location.hostname === "tauri.localhost";
const IS_BROWSER =
  typeof window.__TAURI__ === "undefined" || !IS_TAURI_HOST;
const tauri = window.__TAURI__ ?? {};

// 远程访问令牌：URL ?token= 优先（顺手存起来，方便扫码/点链接直达），
// 其次 cookie（anyboot_token，一年有效），最后兼容迁移旧版 localStorage。
const REMOTE_TOKEN_COOKIE = "anyboot_token";
const REMOTE_TOKEN_KEY = "anyboot.remote-token";

function readCookie(name) {
  for (const pair of document.cookie.split(";")) {
    const index = pair.indexOf("=");
    if (index < 0) continue;
    if (pair.slice(0, index).trim() === name) {
      try {
        return decodeURIComponent(pair.slice(index + 1));
      } catch {
        return pair.slice(index + 1);
      }
    }
  }
  return "";
}

function writeCookie(name, value) {
  // 一年有效；SameSite=Strict 不跨站携带；HttpOnly 做不了（JS 写的），
  // 但 token 本来就是给这台浏览器用的共享口令，风险与 localStorage 等价。
  document.cookie = `${name}=${encodeURIComponent(value)}; path=/; max-age=31536000; SameSite=Strict`;
}

let remoteToken = "";
try {
  const fromUrl = new URLSearchParams(window.location.search).get("token");
  const stored =
    readCookie(REMOTE_TOKEN_COOKIE) ||
    localStorage.getItem(REMOTE_TOKEN_KEY) ||
    "";
  if (fromUrl && fromUrl.trim()) {
    remoteToken = fromUrl.trim();
    writeCookie(REMOTE_TOKEN_COOKIE, remoteToken);
  } else if (stored) {
    remoteToken = stored;
    // 旧版本存过 localStorage 的迁移进 cookie（用户要求：cookie 保存）
    writeCookie(REMOTE_TOKEN_COOKIE, stored);
  }
} catch {
  // cookie/localStorage 不可用（隐私模式等）：退回仅本次会话有效的空 token
}

/// 浏览器远程模式是否可用（页面由 AnyBoot 内置 HTTP 服务托管时为 true）
let remoteApiAvailable = false;
/// 快照事件的处理函数：listen 注册时记下来，推送/轮询拿到新快照时调它
let uiStateHandler = null;
/// 快照轮询定时器（只允许存在一个）
let uiStatePollTimer = null;
/// 探测失败后的重试定时器（见 scheduleRemoteApiReprobe）
let probeRetryTimer = null;

// ---------- 传输方式：推送（WebSocket）还是轮询（HTTP） ----------
//
// 两条路送的是**同一份快照**（后端 publish_ui 一个来源），对界面完全等价，
// 区别只在「谁先知道」和「耗不耗电」：
//   ws    长连接，改动一到就到（毫秒级），空闲时几乎不发包
//   http  每 2s 拉一次，穿代理/NAT 最稳，但最坏差 2s，手机端一直有细水长流
// 默认 auto：先试推送，连不上就落到轮询，并在后台继续试着把推送接回来。
const TRANSPORT_KEY = "anyboot.transport";
const TRANSPORT_AUTO = "auto";
const TRANSPORT_PUSH = "ws";
const TRANSPORT_POLL = "http";
/// 轮询间隔（毫秒）
const POLL_INTERVAL = 2000;
/// 推送重连退避的上限
const PUSH_RETRY_MAX = 30000;

/// 选定的传输方式（用户在横幅里选、或手机 App 用 ?transport= 指定）
/// —— 第一帧横幅就要显示它，所以在这儿就读出来（URL 的 ?transport= 也在这时候落库）
let transportMode = readTransportPreference();
/// 眼下**实际**在用的通道："ws" / "http" / "none"（横幅据此显示）
let activeTransport = "none";
/// 推送连接的 **中** 概览
let pushSocket = null;
let pushRetryTimer = null;
let pushRetryDelay = 1000;
/// 手动刷新 / 下拉刷新正在进行（防连点）
let refreshing = false;

async function probeRemoteApi() {
  try {
    const response = await fetch("/health", { cache: "no-store" });
    const json = await response.json().catch(() => ({}));
    remoteApiAvailable = Boolean(json && json.service === "anyboot");
  } catch {
    remoteApiAvailable = false;
  }
  return remoteApiAvailable;
}

/// 读传输方式偏好：URL 上的 `?transport=`（手机 App 传进来）> localStorage > 自动。
///
/// 为什么 App 要专门传这个参数：网页端的 localStorage 是按**来源站点**存的，
/// 而一台电脑有多个网卡时地址会换（换地址 = 换来源），偏好就跟着丢了。
/// 由 App 每次加载时带上，换地址也不会丢。
function readTransportPreference() {
  const known = (value) =>
    value === TRANSPORT_AUTO || value === TRANSPORT_PUSH || value === TRANSPORT_POLL;
  try {
    const fromUrl = (new URLSearchParams(window.location.search).get("transport") || "").toLowerCase();
    if (known(fromUrl)) {
      // App 里选的就是用户的意思，顺手也记进本页 localStorage
      localStorage.setItem(TRANSPORT_KEY, fromUrl);
      return fromUrl;
    }
    const stored = (localStorage.getItem(TRANSPORT_KEY) || "").toLowerCase();
    if (stored === TRANSPORT_PUSH || stored === TRANSPORT_POLL) {
      return stored;
    }
  } catch {
    // localStorage 不可用（隐私模式等）：按自动来
  }
  return TRANSPORT_AUTO;
}

/// 把一份快照交给页面：推送与轮询**共用同一个出口**，两条路不会分叉。
function deliverSnapshot(payload) {
  if (uiStateHandler) {
    uiStateHandler({ payload });
  }
}

/// 网页端的「快照轮询」：每 2s 拉一次 /api/ui-state。只起一个 —— 探测重试接回来
/// 时会再调一次，重复起会把请求翻倍。
function startUiStatePolling() {
  if (uiStatePollTimer !== null || !remoteApiAvailable) {
    return;
  }
  uiStatePollTimer = setInterval(pollUiStateOnce, POLL_INTERVAL);
}

function stopUiStatePolling() {
  if (uiStatePollTimer !== null) {
    clearInterval(uiStatePollTimer);
    uiStatePollTimer = null;
  }
}

async function pollUiStateOnce() {
  if (document.hidden) {
    return;
  }
  // 手上没有 token 就别轮询了：每次都只会 401，既是噪声，也会把
  // 「请重新输入」的提示反复糊到用户脸上（token 输入框被打断的帮凶之一）。
  // 定时器照常在跑，token 一填好（或从 401 恢复）下一拍就自动续上。
  if (!remoteToken) {
    return;
  }
  try {
    const response = await fetch("/api/ui-state", {
      headers: { "X-Auth-Token": remoteToken },
      cache: "no-store",
    });
    if (response.ok) {
      deliverSnapshot(await response.json());
    } else if (response.status === 401) {
      requestRemoteToken();
    }
  } catch {
    // 断线时静默，下一轮再试
  }
}

/// 手动刷新（横幅的「刷新」/ 手机上下拉）：立刻抓一份快照，不等下一拍。
///
/// 推送通道正常时这个按钮基本用不上，但它是**兜底**：推送连着、界面却看着不对
/// （比如某一拍被丢帧、或用户在另一台设备上刚改完），下拉一下立刻对齐。
async function refreshNow({ silent = false } = {}) {
  if (!IS_BROWSER || !remoteApiAvailable || !remoteToken) {
    if (!silent) {
      showStatusBar("还没连上本机服务，无法刷新", { error: true });
    }
    return;
  }
  if (refreshing) {
    return;
  }
  refreshing = true;
  setPullIndicator("正在刷新…", 12, true);
  try {
    const response = await fetch("/api/ui-state", {
      headers: { "X-Auth-Token": remoteToken },
      cache: "no-store",
    });
    if (response.status === 401) {
      requestRemoteToken();
      return;
    }
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    // force：手动刷新时后端可能没有新 revision，也要重画一遍
    // —— 用户要的是「我看得见的反馈」，不是「省一次渲染」
    applySnapshot(await response.json(), { force: true });
    if (!silent) {
      showStatusBar("已刷新");
    }
  } catch (error) {
    if (!silent) {
      showStatusBar(`刷新失败：${error}`, { error: true });
    }
  } finally {
    refreshing = false;
    setPullIndicator("", 0, false);
  }
}

/// 建推送连接。auto 模式下失败会退到轮询并继续重试；ws 模式下只重试。
function connectPush() {
  if (!IS_BROWSER || !remoteApiAvailable || pushSocket) {
    return;
  }
  // 没有 token 就别连了：后端只会回 401，浏览器控制台一片红
  if (!remoteToken) {
    setActiveTransport("none");
    return;
  }
  const scheme = window.location.protocol === "https:" ? "wss:" : "ws:";
  const url = `${scheme}//${window.location.host}/api/ws?token=${encodeURIComponent(remoteToken)}`;
  let socket;
  try {
    socket = new WebSocket(url);
  } catch {
    onPushUnavailable();
    return;
  }
  pushSocket = socket;

  socket.addEventListener("open", () => {
    pushRetryDelay = 1000;
    // 推送通了就把轮询撤掉：两条一起跑等于做两份功，还会互相覆盖
    stopUiStatePolling();
    setActiveTransport("ws");
  });
  socket.addEventListener("message", (event) => {
    try {
      deliverSnapshot(JSON.parse(event.data));
    } catch {
      // 不是 JSON（心跳之类）忽略
    }
  });
  // error 之后 close 一定会跟着来，收尾只写在 close 里，免得做两遍
  socket.addEventListener("close", () => {
    if (pushSocket === socket) {
      pushSocket = null;
    }
    onPushUnavailable();
  });
}

/// 推送断了（连不上 / 被断开）。
function onPushUnavailable() {
  if (!remoteApiAvailable || !remoteToken) {
    stopPush();
    setActiveTransport("none");
    return;
  }
  if (transportMode === TRANSPORT_AUTO) {
    // 退到轮询：页面照常可用，只是慢一拍；同时后台继续试着把推送接回来
    startUiStatePolling();
    setActiveTransport("http");
  } else {
    // 用户明确选了推送：不偷偷改回轮询（那正是他不想要的），只提示 + 重连。
    // 但至少抓一份当前快照，别让页面停在旧数据上。
    setActiveTransport("none");
    showStatusBar("推送通道已断开，正在重连…", { error: true });
    refreshNow({ silent: true });
  }
  schedulePushRetry();
}

/// 退避重连：1s → 2s → 4s … 封顶 30s。连上后 open 里会把间隔复位。
function schedulePushRetry() {
  if (pushRetryTimer !== null || !remoteApiAvailable || !remoteToken) {
    return;
  }
  pushRetryTimer = setTimeout(() => {
    pushRetryTimer = null;
    connectPush();
  }, pushRetryDelay);
  pushRetryDelay = Math.min(pushRetryDelay * 2, PUSH_RETRY_MAX);
}

function stopPush() {
  if (pushRetryTimer !== null) {
    clearTimeout(pushRetryTimer);
    pushRetryTimer = null;
  }
  if (pushSocket) {
    const socket = pushSocket;
    pushSocket = null;
    try {
      socket.close();
    } catch {
      // 关不掉就算了（这条连接本来就已经断了）
    }
  }
  pushRetryDelay = 1000;
}

/// 按当前的 transportMode 重起通道（切换模式 / token 变化 / 探测重连成功都走这里）。
function restartTransport() {
  if (!IS_BROWSER) {
    return;
  }
  stopPush();
  stopUiStatePolling();
  if (!remoteApiAvailable || !remoteToken) {
    setActiveTransport("none");
    return;
  }
  if (transportMode === TRANSPORT_POLL) {
    startUiStatePolling();
    setActiveTransport("http");
    return;
  }
  // 自动与推送都是先试推送
  setActiveTransport("none");
  connectPush();
}

function setActiveTransport(value) {
  if (activeTransport === value) {
    return;
  }
  activeTransport = value;
  renderBrowserBanner();
}

/// 横幅上那个传输方式下拉：用户手动选，立刻生效并长期记住。
function setTransportMode(mode) {
  transportMode = mode;
  try {
    localStorage.setItem(TRANSPORT_KEY, mode);
  } catch {
    // 存不了就本次会话有效
  }
  restartTransport();
}

/// 把界面接上远程模式（探通那一刻调一次）：横幅、token 引导、通道、首份快照。
async function attachRemoteApi() {
  renderBrowserBanner();
  // 首次访问（没有存过 token）：直接弹模态框要 token，别等第一次 401
  if (!remoteToken) {
    showTokenModal();
  }
  restartTransport();
  try {
    applySnapshot(await invoke("get_ui_state"), { force: true });
  } catch (error) {
    showStatusBar(String(error), { error: true });
  }
}

/// 探测失败就每 2s 重试一次，直到连上为止。
///
/// 为什么必须重试：客户端（手机 / 浏览器）刚打开的那一刻，电脑端服务可能正在
/// 重启、或者 WiFi 刚连上还没通 —— 只探一次会把「暂时连不上」**永久**定格成
/// 「纯预览模式」：横幅写着预览，正文永远是空的（没有任何数据源），用户只能
/// 手动刷新页面。探通的那一拍把界面接回远程模式，省掉这一次刷新。
function scheduleRemoteApiReprobe() {
  if (probeRetryTimer !== null) {
    return;
  }
  probeRetryTimer = setInterval(async () => {
    if (!(await probeRemoteApi())) {
      return;
    }
    clearInterval(probeRetryTimer);
    probeRetryTimer = null;
    await attachRemoteApi();
  }, 2000);
}

const invoke = IS_BROWSER
  ? async (command, args = {}) => {
      if (!remoteApiAvailable) {
        throw new Error(
          `浏览器预览模式：命令「${command}」不可用，请在 AnyBoot 应用内操作`,
        );
      }
      const response = await fetch("/api/ui-command", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "X-Auth-Token": remoteToken,
        },
        body: JSON.stringify({ cmd: command, args }),
      });
      const json = await response.json().catch(() => ({}));
      if (response.status === 401) {
        requestRemoteToken();
        throw new Error("需要访问令牌：请在顶部输入 AnyBoot 远程控制 token");
      }
      if (!response.ok || json.ok === false) {
        throw new Error(json.message || json.error || `HTTP ${response.status}`);
      }
      return json.result;
    }
  : tauri.core.invoke;

const listen = IS_BROWSER
  ? async (event, handler) => {
      // 浏览器里没有托盘，也就没有 focus-remote 事件会来。**这里什么都不做**：
      // 曾经写成「网页上等价于切到设置栏目」，可这个 shim 是在**注册时**就执行的
      // （见文件末尾 listen("focus-remote", …)），于是每次打开网页都被顶到基本设置，
      // 地址栏 hash 也跟着变成 #/settings —— 「刷新正好停在基本设置」的真凶就是它。
      if (event !== "ui-state-changed") {
        return () => {};
      }
      // 处理函数先记下：这一刻可能还没探通（远程模式没连上），等接回来再起通道
      uiStateHandler = handler;
      restartTransport();
      return () => {
        stopPush();
        stopUiStatePolling();
      };
    }
  : tauri.event.listen;
// 拖动不透明度滑条时的**预览广播**：change 才写后端（免得拖一次写十几次），
// 窗口自己靠这个事件即时重绘（拖动过程中不写后端）。网页上只有本页，无需广播。
const emitEvent = IS_BROWSER ? () => {} : tauri.event.emit;

const statusElement = document.querySelector("#status");
const systemsElement = document.querySelector("#systems");
const directoryElement = document.querySelector("#vars-dir");
const autostartElement = document.querySelector("#autostart");
const closeActionElement = document.querySelector("#close-action");
const unmountTemporaryElement = document.querySelector("#unmount-temporary");
const autostartStateElement = document.querySelector("#autostart-state");
const loginItemsElement = document.querySelector("#login-items");
// 电源动作：重启（默认）/ 关机 / 睡眠 —— 由调用方传参决定执行哪个
const powerButtons = [
  [document.querySelector("#power-reboot"), "reboot"],
  [document.querySelector("#power-shutdown"), "shutdown"],
  [document.querySelector("#power-sleep"), "sleep"],
];
// 程序自身的两个动作（顶部标题栏、电源三键右侧）：重启程序 / 退出程序
const restartAppElement = document.querySelector("#restart-app");
const quitAppElement = document.querySelector("#quit-app");
const startInTrayElement = document.querySelector("#start-in-tray");
const mainAlwaysOnTopElement = document.querySelector("#main-always-on-top");
const mainAutoHideElement = document.querySelector("#main-auto-hide");
const mainAutoHideStateElement = document.querySelector("#main-auto-hide-state");
const mainAutoHideEdgeDistanceElement = document.querySelector("#main-auto-hide-edge-distance");
const mainDockStripHoverMsElement = document.querySelector("#main-dock-strip-hover-ms");
const mainDockStripEdgeGapElement = document.querySelector("#main-dock-strip-edge-gap");
const mainDockStripThicknessElement = document.querySelector("#main-dock-strip-thickness");
const mainDockStripThicknessStateElement = document.querySelector("#main-dock-strip-thickness-state");
const mainDockStripLengthElement = document.querySelector("#main-dock-strip-length");
const mainDockStripLengthStateElement = document.querySelector("#main-dock-strip-length-state");
const mainDockStripOpacityElement = document.querySelector("#main-dock-strip-opacity");
const mainDockStripOpacityStateElement = document.querySelector("#main-dock-strip-opacity-state");
const mainDockStripColorElement = document.querySelector("#main-dock-strip-color");
// 贴边热区：厚度（触发判定用）+ 调试可视化（底色/透明度/边框）
const mainHotzoneThicknessElement = document.querySelector("#main-hotzone-thickness");
const mainHotzoneThicknessStateElement = document.querySelector("#main-hotzone-thickness-state");
const mainHotzoneColorElement = document.querySelector("#main-hotzone-color");
const mainHotzoneOpacityElement = document.querySelector("#main-hotzone-opacity");
const mainHotzoneOpacityStateElement = document.querySelector("#main-hotzone-opacity-state");
const mainHotzoneBorderElement = document.querySelector("#main-hotzone-border");
const mainHotzoneBorderColorElement = document.querySelector("#main-hotzone-border-color");
const mainWindowPositionElement = document.querySelector("#main-window-position");
const trayMenuStyleElement = document.querySelector("#tray-menu-style");
const mainFixedMonitorElement = document.querySelector("#main-fixed-monitor");
const resetMainWindowPositionElement = document.querySelector("#reset-main-window-position");
// 窗口大小：宽/高滑条 + 锁定开关（滑条 ⇄ 窗口尺寸双向实时）
const windowSizeWidthElement = document.querySelector("#window-size-width");
const windowSizeWidthStateElement = document.querySelector("#window-size-width-state");
const windowSizeHeightElement = document.querySelector("#window-size-height");
const windowSizeHeightStateElement = document.querySelector("#window-size-height-state");
const windowSizeLockedElement = document.querySelector("#window-size-locked");
const hideFromDockTaskbarElement = document.querySelector("#hide-from-dock-taskbar");
const refreshElement = document.querySelector("#refresh");
const useDirectoryElement = document.querySelector("#use-directory");
// 下次启动大字（问题 7）
const nextBootElement = document.querySelector("#next-boot");
const nextBootNameElement = document.querySelector("#next-boot-name");
// 外观（问题 8）
const themeElement = document.querySelector("#theme");
const accentColorElement = document.querySelector("#accent-color");
const opacityElement = document.querySelector("#opacity");
const opacityStateElement = document.querySelector("#opacity-state");

// 远程控制区
const remoteSectionElement = document.querySelector("#remote");
const httpEnabledElement = document.querySelector("#http-enabled");
const httpStateElement = document.querySelector("#http-state");
const httpBindElement = document.querySelector("#http-bind");
const httpPortElement = document.querySelector("#http-port");
const httpApplyElement = document.querySelector("#http-apply");
const tokenHintElement = document.querySelector("#token-hint");
const tokenInputElement = document.querySelector("#token-input");
const tokenSaveElement = document.querySelector("#token-save");
const tokenRevealElement = document.querySelector("#token-reveal");
const tokenCopyElement = document.querySelector("#token-copy");
const tokenRegenerateElement = document.querySelector("#token-regenerate");
// 「打开 Web 页面」按钮：只有桌面主窗口才有意义（网页端自己就是 Web 页面）
const openWebElement = document.querySelector("#open-web");
const notifyEnabledElement = document.querySelector("#notify-enabled");
const httpChannelNotifyElement = document.querySelector("#http-channel-notify");
const httpChannelDelayElement = document.querySelector("#http-channel-delay");
const cliChannelNotifyElement = document.querySelector("#cli-channel-notify");
const cliChannelDelayElement = document.querySelector("#cli-channel-delay");
const remoteAllowGuiAuthElement = document.querySelector("#remote-allow-gui-auth");
const pendingCardElement = document.querySelector("#pending-card");
const pendingTitleElement = document.querySelector("#pending-title");
const pendingDetailElement = document.querySelector("#pending-detail");
const pendingRunElement = document.querySelector("#pending-run");
const pendingCancelElement = document.querySelector("#pending-cancel");
const httpHintElement = document.querySelector("#http-hint");
const httpHintCopyElement = document.querySelector("#http-hint-copy");
const httpRequestsElement = document.querySelector("#http-requests");
// 提示音相关
const playSoundElement = document.querySelector("#play-sound");
const soundFilePathElement = document.querySelector("#sound-file-path");
const soundApplyElement = document.querySelector("#sound-apply");
// 打开目录按钮
const openConfigDirElement = document.querySelector("#open-config-dir");
const openVarsDirElement = document.querySelector("#open-vars-dir");
// 端口占用提示 + 两个按钮
const portOccupantElement = document.querySelector("#port-occupant");
const portOccupantMessageElement = document.querySelector("#port-occupant-message");
const portOccupantHintElement = document.querySelector("#port-occupant-hint");
const retryHttpElement = document.querySelector("#retry-http");
const killOccupantElement = document.querySelector("#kill-occupant");

// GRUB 细粒度切换区（三系统下方那一块）
const grubMetaElement = document.querySelector("#grub-meta");
const grubDirElement = document.querySelector("#grub-dir");
const grubDirDetectElement = document.querySelector("#grub-dir-detect");
const grubDirSaveElement = document.querySelector("#grub-dir-save");
const grubDirPickElement = document.querySelector("#grub-dir-pick");
const grubDirOpenElement = document.querySelector("#grub-dir-open");
const grubMountElement = document.querySelector("#grub-mount");
const grubMountLabelElement = document.querySelector("#grub-mount-label");
const grubPartElement = document.querySelector("#grub-part");
const grubPartuuidElement = document.querySelector("#grub-partuuid");
const grubPartuuidSaveElement = document.querySelector("#grub-partuuid-save");
const grubPartuuidDetectElement = document.querySelector("#grub-partuuid-detect");
const grubSyncElement = document.querySelector("#grub-sync");
const grubHintElement = document.querySelector("#grub-hint");
const grubErrorElement = document.querySelector("#grub-error");
// 空状态引导卡：还没识别出任何 GRUB 信息时替代列表区的一片空白
const grubEmptyElement = document.querySelector("#grub-empty");
const grubEmptyPickElement = document.querySelector("#grub-empty-pick");
const grubListElement = document.querySelector("#grub-list");
// GRUB 区块本体（左栏主流程里）与右栏容器：展示模式切换时把节点搬进/搬出右栏
const grubSectionElement = document.querySelector("#grub");
const grubPaneBodyElement = document.querySelector("#grub-pane-body");
// 展示方式下拉框（内嵌 / 加宽 / 列表独立 / GRUB 独立）
const grubViewModeElement = document.querySelector("#grub-view-mode");
const grubListFormatElement = document.querySelector("#grub-list-format");
// 切换启动项的回执行（跟着配置块走）：中间态与失败原因
const grubStatusElement = document.querySelector("#grub-status");
const grubInstallActionsElement = document.querySelector("#grub-install-actions");
const grubInstallElement = document.querySelector("#grub-install");
const grubUninstallElement = document.querySelector("#grub-uninstall");
const grubScriptElement = document.querySelector("#grub-script");
const grubScriptSaveElement = document.querySelector("#grub-script-save");
const grubScriptResetElement = document.querySelector("#grub-script-reset");
const grubLogBoxElement = document.querySelector("#grub-log-box");
const grubLogSummaryElement = document.querySelector("#grub-log-summary");
const grubLogElement = document.querySelector("#grub-log");

// 固定在窗口最下方的提示条（所有操作回执都落这里）
const statusBarElement = document.querySelector("#status-bar");
const statusBarIconElement = document.querySelector("#status-bar-icon");
const statusBarTextElement = document.querySelector("#status-bar-text");
const statusBarCloseElement = document.querySelector("#status-bar-close");

const autostartStateLabels = new Map([
  ["enabled", "已加入系统登录项"],
  ["requires-approval", "已注册，待系统在登录项中批准"],
  ["disabled", "未加入系统登录项"],
  ["unavailable", "登录项状态不可用"],
]);

const switchButtonLabel = new Map([
  ["linux", "下次启动 Linux"],
  ["windows", "下次启动 Windows"],
  ["macos", "下次启动 macOS"],
]);

const powerLabels = new Map([
  ["reboot", "重启"],
  ["shutdown", "关机"],
  ["sleep", "睡眠"],
  // 旧后端用的名字，留着不会出错
  ["restart", "重启"],
]);

// 快照版本号：只认更新的快照，乱序/过期的一律丢弃（阶段 1 修 D4）。
let lastRevision = -1;
// 最近一次快照：命令失败时用它把界面拉回系统真实状态。
let currentState = null;
// 是否成功渲染过任何一份快照 —— 用来判断「页面还是空的」这件事有没有发生过
// （见 applySnapshot：结构不对时只在对空页面说一次，不刷屏）。
let hasRenderedSnapshot = false;

// ---- 事件丢失兜底轮询（仅 Tauri 主窗） ----
// ui-state-changed 是主路径，但 macOS WKWebView 上实测偶发丢失（主窗卡在
// 「正在扫描 rEFInd 变量目录…」不再更新，HTTP/WS 通道都正常）——事件一发即失，
// 页面永远停在最后一帧。轮询按 revision 去重，事件正常时多拿的快照会被丢弃，
// 只在事件丢失时起作用；页面隐藏时暂停（隐藏的窗没人看，也免得空转）。
const TAURI_POLL_INTERVAL = 3000;
let tauriPollTimer = null;

function startTauriStatePolling() {
  if (IS_BROWSER || tauriPollTimer !== null) {
    return;
  }
  tauriPollTimer = setInterval(async () => {
    // 注意：不做 document.hidden 跳过 —— macOS 上主窗启动即隐藏（startInTray），
    // WKWebView 的 JS 在隐藏期被冻结/迟到，若连首次初始化都被吞掉，只有靠
    // 这里的轮询在窗口显示前后把首份快照拉回来。按 revision 去重，事件正常时
    // 同拍快照直接丢弃，隐藏期多拉的几拍也无害。
    try {
      applySnapshot(await tauri.core.invoke("get_ui_state"));
    } catch {
      // 后端正忙/重启中：静默，下一拍再试
    }
  }, TAURI_POLL_INTERVAL);
}

function isBusy(busy) {
  if (!busy) {
    return false;
  }
  return Boolean(
    busy.power || busy.switching || busy.scanning || busy.unmounting,
  );
}

function busyLabel(busy) {
  if (!busy) {
    return "";
  }
  if (busy.power) {
    return `${powerLabels.get(busy.power) || busy.power}处理中…`;
  }
  if (busy.switching) {
    return "正在切换下次启动系统…";
  }
  if (busy.unmounting) {
    return "正在卸载临时挂载…";
  }
  if (busy.scanning) {
    return "正在扫描…";
  }
  return "";
}

function actionMessage(state) {
  if (state.status.error) {
    return state.status.error;
  }
  if (state.lastAction && state.lastAction.message) {
    return state.lastAction.message;
  }
  return "";
}

// ---------- 窗口最下方的提示条（主内容区之外） ----------
//
// 以前所有操作回执都写在正文最末尾的 #message —— 用户要滚到页面底部才看得见，
// 于是「勾选了某个系统」这类操作看起来像没反应。现在统一走这条独立底栏：
// 它是 body 的直接子元素、和 main 平级，占自己那一格（不浮在内容上、不遮正文）。
//
// 谁最后说话谁显示：命令的即时回执（call() 里的错误、GRUB 区操作的结果）优先，
// 其次是快照里的 status.error / lastAction。用户点「×」只是收起当前这条，
// 之后有新回执会再弹出来。
let statusBarDismissed = "";
let statusBarMessage = "";

function showStatusBar(text, { error = false } = {}) {
  const message = (text || "").trim();
  if (!message) {
    hideStatusBar();
    return;
  }
  statusBarMessage = message;
  statusBarDismissed = "";
  statusBarElement.classList.remove("hidden");
  statusBarElement.classList.toggle("error", Boolean(error));
  statusBarElement.classList.toggle("ok", !error);
  statusBarIconElement.textContent = error ? "✕" : "✓";
  statusBarTextElement.textContent = message;
}

function hideStatusBar() {
  statusBarMessage = "";
  statusBarElement.classList.add("hidden");
  statusBarElement.classList.remove("error", "ok");
}

// 快照里带来的回执（status.error / lastAction）：只在用户没手动收起这条时覆盖
function syncStatusBarFromState(state) {
  const text = actionMessage(state);
  if (!text) {
    return;
  }
  if (text === statusBarDismissed || text === statusBarMessage) {
    return;
  }
  showStatusBar(text, { error: Boolean(state.status.error) });
}

statusBarCloseElement.addEventListener("click", () => {
  statusBarDismissed = statusBarMessage;
  hideStatusBar();
});

function renderSystems(status, busy, grub) {
  systemsElement.replaceChildren();

  // 顶部那行大字要说明「下次启动的系统是哪个」，先扫一遍选中项
  let activeName = null;
  let activeSystem = null;

  for (const item of status.variables) {
    const card = document.createElement("article");
    card.className = [
      "system-card",
      item.active ? "active" : "",
      item.exists ? "" : "unavailable",
    ].filter(Boolean).join(" ");

    // 勾占**独立的一列**（固定 26px）：有没有勾都不会挤动名称/按钮，三行天然对齐
    const check = document.createElement("span");
    check.className = "system-check";
    check.textContent = item.active ? "✓" : "";
    check.setAttribute("aria-hidden", "true");
    if (item.active) {
      activeName = item.displayName;
      activeSystem = item.system;
    }

    const name = document.createElement("div");
    name.className = "system-name";
    const title = document.createElement("strong");
    title.textContent = item.displayName;
    const source = document.createElement("small");
    source.textContent = item.exists
      ? item.sourceName
      : `${item.sourceName}（不存在）`;
    name.append(title, source);

    // Linux 项旁的问号提示（用户定案 2026-10-13）：rEFInd 的 Linux 项背后
    // 是 GRUB 在引导，要能读取 grub 列表 / 操作下次 Linux 启动项，前提是
    // 装了 09_anyboot 并 update-grub；同时 EFI 第一启动项的设置规则也要讲清。
    if (item.system === "linux") {
      const help = document.createElement("span");
      help.className = "system-help";
      help.textContent = "?";
      help.tabIndex = 0;
      help.setAttribute("role", "note");
      help.setAttribute(
        "aria-label",
        "Linux 启动项说明：需安装 09_anyboot 并执行 update-grub",
      );
      help.title =
        "Linux 由 GRUB 引导：在 Linux 上安装 09_anyboot 脚本（/etc/grub.d/09_anyboot）" +
        "并执行 update-grub 重建 grub.cfg 之后，AnyBoot 才能读取 GRUB 系统列表、" +
        "并通过 ESP 上的 grubenv 操作「下次启动哪个 Linux 启动项」。" +
        "EFI 第一启动项：用 rEFInd 就把 rEFInd 的 EFI 设为第一启动项；" +
        "不用 rEFInd 就把 GRUB 的 EFI 设为第一启动项。";
      name.append(help);
    }

    const button = document.createElement("button");
    button.type = "button";
    button.textContent = item.active
      ? "已选择"
      : switchButtonLabel.get(item.system);
    button.disabled = !item.exists || busy || Boolean(status.error);
    button.addEventListener("click", async () => {
      const result = await call("switch_system", { system: item.system });
      // 网页 / 手机端：选择系统会被后端转成「远程请求」—— 本机弹授权窗、
      // 显示来源客户端，本机用户确认后才真的切换。这里如实说一声，
      // 别让用户以为点了没反应（本机主窗口点按钮仍是即时生效的）。
      if (IS_BROWSER && result && result.pending) {
        showStatusBar(
          `已发送到本机：${result.message || "等待本机确认后切换"}`,
        );
      }
    });

    card.append(check, name, button);
    systemsElement.append(card);
  }

  // 下次启动 = 两层引导的级联总结（用户定案 2026-10-13）：rEFInd 选了 Linux
  // 时，开机真正进的是 GRUB 环境块里选中的那个启动项 —— 两层一并显示
  // 「Linux › 启动项名」；rEFInd 选的不是 Linux（Windows/macOS 直接引导），
  // 或者 GRUB 侧还没选过，就只显示 rEFInd 这一层。
  let nextBootText = activeName || "未知（没有选中的系统）";
  if (activeSystem === "linux" && grub) {
    const selector = grub.nextEntry || grub.savedEntry || "";
    const entry = (grub.entries || []).find(
      (candidate) => candidate.selector === selector,
    );
    if (entry) {
      // 条目文字与 GRUB 列表用同一条规则（grub-list-view.js 的 rowLabel）：
      // 极简/简易=去前缀内核名；中等=完整内核名；原始/复杂=完整菜单名 ——
      // 上面大字和下面列表说的是同一个东西，别一处一个叫法（用户定案 2026-10-13）。
      const format = grub.listFormat || "minimal";
      const leafLabel =
        format === "minimal" || format === "simple"
          ? grubStripKernel(grubKernelOf(entry))
          : format === "medium"
            ? grubKernelOf(entry)
            : entry.name;
      // submenu 子条目带上父级组名 ——「Linux › 组名 › 条目」
      if (entry.submenu) {
        const groups = new Map(
          (grub.groups || []).map((group) => [group.id, group.name]),
        );
        nextBootText += ` › ${groups.get(entry.submenu) || entry.submenu}`;
      }
      nextBootText += ` › ${leafLabel}`;
    }
  }
  nextBootElement.classList.toggle("unknown", !activeName);
  nextBootNameElement.textContent = nextBootText;
  // 顶部状态行要用它去重（见 render）
  return nextBootText;
}

/// ESP 分区那一行：FAT32 判定 · 分区UUID · fs-uuid · 设备 · 挂载状态 · 出处。
///
/// 探测失败时后端只给 `note`（没有 fstype），这里就老老实实只显示那句话，
/// 绝不自作主张说「不是 FAT32」。
function renderEsp(grub) {
  grubPartElement.replaceChildren();
  const esp = grub.esp;
  if (!esp) {
    return;
  }
  if (!esp.fstype) {
    grubPartElement.textContent = esp.note || "";
    return;
  }

  const nodes = [];
  const add = (node) => {
    if (nodes.length) {
      nodes.push(document.createTextNode(" · "));
    }
    nodes.push(node);
  };

  const filesystem = document.createElement(esp.fat ? "b" : "i");
  filesystem.textContent = esp.fat32
    ? `${esp.fstype}（FAT32）`
    : esp.fat
      ? `${esp.fstype}（不是 FAT32）`
      : `${esp.fstype}（不是 FAT）`;
  add(filesystem);

  // 分区 UUID 是识别键（三端同值），摆最前；fs-uuid 有就顺带展示
  if (esp.partuuid) {
    const label = document.createElement("b");
    label.textContent = `分区UUID ${esp.partuuid}`;
    add(label);
  }
  if (esp.uuid) {
    add(document.createTextNode(`fs-uuid ${esp.uuid}`));
  }
  if (!esp.partuuid && !esp.uuid) {
    const missing = document.createElement("i");
    missing.textContent = "没取出分区 UUID";
    add(missing);
  }
  if (esp.device) {
    add(document.createTextNode(esp.device));
  }
  // 挂载点摆出来（用户「填了分区 UUID 还是找不到」多数就是这一行空着/没挂上）
  if (esp.mountPoint) {
    add(
      document.createTextNode(
        esp.preexisting ? `已挂载于 ${esp.mountPoint}` : `挂载于 ${esp.mountPoint}`,
      ),
    );
  } else {
    const nowhere = document.createElement("i");
    nowhere.textContent = "当前没有挂载点";
    add(nowhere);
  }
  add(
    document.createTextNode(
      esp.mounted ? (esp.ownedMount ? "（本程序挂的）" : "") : "（未挂载）",
    ),
  );
  if (esp.note) {
    add(document.createTextNode(esp.note));
  }
  grubPartElement.append(...nodes);
}

/// 展示模式的 DOM 搬位：把 <main> 里 #grub 的子节点按模式搬进/搬出右栏。
/// 四种模式（与设置里的 grubListView 一一对应）：
/// · inline 内嵌 / wide 加宽 —— 全部子节点按出厂顺序回 #grub（左栏主流程里）；
/// · list 列表独立 —— 配置留在 #grub，列表搬进右栏；
/// · pane GRUB 独立 —— 子节点搬进右栏（#grub 变空壳，CSS 藏掉），
///   **但头部（圆标/标题/状态行）钉在卡片上方**：用户定案 2026-10-13，
///   头部不参与滚动 —— sticky 在 grid 子项上会被约束在自己的网格区里、
///   滚过头就跟着走，所以直接搬出滚动容器（.pane-body）放到它前面。
/// 只在模式真正变化时才搬（renderGrub 里比对缓存），别每帧搬 DOM。
/// 「出厂顺序」在脚本加载完、任何搬移发生之前捕获一次，回搬时靠它复原。
const grubCanonicalOrder = [...grubSectionElement.children];
let grubLayoutMode = null;
function applyGrubLayout(mode) {
  if (grubLayoutMode === mode) {
    return;
  }
  grubLayoutMode = mode;
  document.body.dataset.grubMode = mode;
  for (const el of grubCanonicalOrder) {
    if (mode === "pane" && el.classList.contains("grub-head")) {
      // 头部常驻：插到滚动容器前面，滚轮永远碰不到它
      grubPaneBodyElement.parentNode.insertBefore(el, grubPaneBodyElement);
      continue;
    }
    const toPane = mode === "pane" || (mode === "list" && el === grubListElement);
    (toPane ? grubPaneBodyElement : grubSectionElement).appendChild(el);
  }
}

/// GRUB 区块：列的是 grub.cfg 里的启动项，条数可能几十个，
/// 所以字号比左栏「可启动系统」小一号（用户明确要求）。
///
/// 三处输入框（ESP 目录 / 分区 UUID / 脚本正文）只在**没被聚焦**时才回写，
/// 否则用户打字打到一半会被快照盖掉。
function renderGrub(grub, script, busy, listViewMode) {
  if (!grub) {
    return;
  }

  // 展示模式：快照里的值归一后落地（搬 DOM + 亮/藏右栏 + 调整列表滚动方式）。
  // 下拉框的值只在和快照不一致时才写，避免用户正展开选项时被状态事件弹回去。
  const mode =
    listViewMode === "wide" || listViewMode === "list" || listViewMode === "pane"
      ? listViewMode
      : "inline";
  applyGrubLayout(mode);
  if (grubViewModeElement.value !== mode) {
    grubViewModeElement.value = mode;
  }
  // 列表格式同理：只在不一致时写，避免展开选项时被快照弹回（快照缺字段回落极简）
  const listFormat = grub.listFormat || "minimal";
  if (grubListFormatElement.value !== listFormat) {
    grubListFormatElement.value = listFormat;
  }

  if (document.activeElement !== grubPartuuidElement) {
    grubPartuuidElement.value = grub.partuuid || "";
  }
  if (document.activeElement !== grubDirElement) {
    grubDirElement.value = grub.dir || "";
  }
  if (document.activeElement !== grubScriptElement) {
    grubScriptElement.value = script || "";
  }

  renderEsp(grub);

  // 挂载勾选：状态以快照为准（挂载失败时后端会把勾退回去，界面跟着退）。
  // Windows 上挂载与否由盘符决定、不是我们能控制的，勾选框就只**反映**真实可访问性，
  // 别摆一个「没勾 = 没挂载」的假象。
  const mounter = Boolean(grub.mountSupported);
  const espMounted = Boolean(grub.esp && grub.esp.mounted);
  grubMountElement.checked = mounter ? Boolean(grub.keepMounted) : espMounted;
  const canMount = mounter && Boolean(grub.partuuid);
  grubMountElement.disabled = busy || !canMount;
  grubMountElement.title = !mounter
    ? "Windows 不能按分区 UUID 挂载：ESP 有没有盘符由「磁盘管理」决定"
    : grub.partuuid
      ? ""
      : "先填好分区 UUID（或用上面的「识别」自动取）";
  grubMountLabelElement.textContent = mounter
    ? "挂载 ESP（勾上后可直接读写）"
    : "ESP 可访问（Windows 由盘符决定，无需挂载）";

  const entries = grub.entries || [];
  const parts = [];
  if (grub.efiDir) {
    parts.push(`EFI 目录 ${grub.efiDir}`);
  }
  parts.push(`${entries.length} 个启动项`);
  if (grub.syncedAt) {
    parts.push(`同步于 ${formatLocalTime(grub.syncedAt)}`);
  }
  grubMetaElement.textContent = parts.join(" · ");

  grubHintElement.textContent = grub.hint || "";
  grubErrorElement.textContent = grub.error || "";
  grubErrorElement.classList.toggle("hidden", !grub.error);

  // 空状态：没有任何识别信息（没填分区 UUID、也没识别过目录）才亮。
  // efiDir 是后端解析结果（识别过就有值），dir 只是用户填的原文，不算数。
  const unidentified = !grub.partuuid && !grub.efiDir;
  grubEmptyElement.hidden = !unidentified;

  // 树列表：唯一的渲染处（列表节点在哪由 applyGrubLayout 决定，这里只管往里画；
  // 节点引用不变，搬完位置渲染照常生效）
  renderGrubEntries({
    container: grubListElement,
    grub,
    busy,
    format: grub.listFormat || "minimal",
    onSwitch: switchGrub,
  });

  // 安装 / 卸载只在 Linux 出现：后端在非 Linux 上把 installed 返回为 null
  const linuxOnly = typeof grub.installed === "boolean";
  grubInstallActionsElement.classList.toggle("hidden", !linuxOnly);
  if (linuxOnly) {
    grubInstallElement.textContent = grub.installed
      ? "重装 09_anyboot"
      : "安装 09_anyboot";
    grubUninstallElement.classList.toggle("hidden", !grub.installed);
  }

  renderScriptLog(grub.scriptLog);
}

/// 安装 / 卸载 09_anyboot 的执行过程：跑了哪几条命令、退出码、stderr 原文。
///
/// 「用户不知道你做了什么操作」是原话 —— 所以这里把命令与输出逐行摊开，
/// 默认展开、成功/失败各配一个颜色。
function renderScriptLog(log) {
  const lines = (log && log.lines) || [];
  if (!lines.length) {
    grubLogBoxElement.classList.add("hidden");
    grubLogElement.textContent = "";
    return;
  }
  grubLogBoxElement.classList.remove("hidden");
  const title = (log && log.title) || "执行过程";
  const verdict = log.ok === true ? "成功" : log.ok === false ? "失败" : "";
  grubLogSummaryElement.textContent = verdict
    ? `${title}：执行过程（${verdict}）`
    : `${title}：执行过程`;
  grubLogBoxElement.classList.toggle("ok", log.ok === true);
  grubLogBoxElement.classList.toggle("fail", log.ok === false);
  grubLogElement.textContent = lines.join("\n");
}

/// 主题取值 → 实际生效的亮/暗（system 时问一下系统偏好）
function resolvedTheme(theme) {
  if (theme === "system") {
    return window.matchMedia("(prefers-color-scheme: light)").matches
      ? "light"
      : "dark";
  }
  return theme === "light" ? "light" : "dark";
}

// ---- 调色板：主题强调色派生 ----------------------------------------
// amount > 0 往白混（提亮），< 0 往黑混（压暗）。暗色主题悬停提亮、
// 亮色主题悬停压暗，与内置配色（#6366f1 靛蓝，与远程管理同色）同方向。
function shadeHex(hex, amount = 0) {
  const match = /^#?([0-9a-f]{6})$/i.exec(String(hex || "").trim());
  if (!match) return null;
  const value = Number.parseInt(match[1], 16);
  const channels = [(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff].map(
    (c) => Math.round(amount >= 0 ? c + (255 - c) * amount : c * (1 + amount)),
  );
  // 任何通道算出 NaN（比如调用方漏传 amount）都返回 null，别把
  // "#NaNNaNNaN" 这种非法值喂给 CSS 变量 —— 那会让整个派生色失效。
  if (channels.some((c) => !Number.isFinite(c))) return null;
  return `#${channels.map((c) => c.toString(16).padStart(2, "0")).join("")}`;
}

/// 把调色板选的强调色落到 CSS 变量上（按钮/链接/悬停色一起派生）。
/// 暗色主题：悬停提亮 8%、链接提亮 18%；亮色主题反过来压暗。
const DEFAULT_ACCENT_DARK = "#6366f1";
const DEFAULT_ACCENT_LIGHT = "#4f46e5";
function defaultAccent(theme) {
  return resolvedTheme(theme) === "dark" ? DEFAULT_ACCENT_DARK : DEFAULT_ACCENT_LIGHT;
}

function applyAccent(accent, theme) {
  const base = shadeHex(accent) || defaultAccent(theme);
  const dark = resolvedTheme(theme) === "dark";
  const hover = shadeHex(base, dark ? 0.08 : -0.12) || base;
  const link = shadeHex(base, dark ? 0.18 : -0.18) || base;
  const linkHover = shadeHex(base, dark ? 0.3 : -0.3) || base;
  const root = document.documentElement.style;
  root.setProperty("--accent", base);
  root.setProperty("--accent-hover", hover);
  root.setProperty("--accent-link", link);
  root.setProperty("--accent-link-hover", linkHover);
}

// 当前强调色：快照渲染与调色板输入都会更新它，主题切换时要用它重算明暗档
let currentAccent = DEFAULT_ACCENT_DARK;

/// 把主题与不透明度落到 documentElement 上（CSS 用 var(--ui-alpha) 读）
function applyAppearance(theme, percent) {
  const resolved = resolvedTheme(theme);
  document.documentElement.dataset.theme = resolved;
  const value = Number(percent);
  const clamped = Math.min(100, Math.max(0, Number.isFinite(value) ? value : 100));
  const alpha = clamped / 100;
  document.documentElement.style.setProperty("--ui-alpha", String(alpha));
  applyAccent(currentAccent, theme);
  opacityStateElement.textContent = `${clamped}%（拖动即时预览）`;
  // 镜像到 localStorage：只供**下次页面重载的首帧**使用（见 index.html 的防闪脚本）。
  // 后端 settings 仍是唯一真相源，这里只是让重载那一拍不必先按默认暗色画一遍。
  try {
    localStorage.setItem("anyboot.theme-resolved", resolved);
    localStorage.setItem("anyboot.ui-alpha", String(alpha));
    // 强调色派生值一并缓存：首帧脚本没法算色阶，直接读成品
    const style = document.documentElement.style;
    for (const name of [
      "--accent",
      "--accent-hover",
      "--accent-link",
      "--accent-link-hover",
    ]) {
      const value = style.getPropertyValue(name);
      if (value) {
        localStorage.setItem(`anyboot.${name.slice(2)}`, value.trim());
      }
    }
  } catch (error) {
    void error;
  }
}

function renderSettings(view) {
  autostartElement.checked = Boolean(view.autostart);
  startInTrayElement.checked = Boolean(view.startInTray);
  mainAutoHideElement.checked = Boolean(view.mainAutoHide);
  mainAlwaysOnTopElement.checked = view.mainAlwaysOnTop !== false;
  if (document.activeElement !== mainAutoHideEdgeDistanceElement) {
    mainAutoHideEdgeDistanceElement.value = Number.isFinite(view.mainAutoHideEdgeDistance)
      ? view.mainAutoHideEdgeDistance
      : 24;
  }
  if (document.activeElement !== mainDockStripHoverMsElement) {
    mainDockStripHoverMsElement.value = Number.isFinite(view.mainDockStripHoverMs)
      ? view.mainDockStripHoverMs
      : 0;
  }
  if (document.activeElement !== mainDockStripEdgeGapElement) {
    mainDockStripEdgeGapElement.value = Number.isFinite(view.mainDockStripEdgeGap)
      ? view.mainDockStripEdgeGap
      : 0;
  }
  if (document.activeElement !== mainDockStripThicknessElement) {
    const thickness = Number.isFinite(view.mainDockStripThickness)
      ? view.mainDockStripThickness
      : 6;
    mainDockStripThicknessElement.value = thickness;
    if (mainDockStripThicknessStateElement) {
      mainDockStripThicknessStateElement.textContent = `${thickness}px（拖动即时预览）`;
    }
  }
  if (document.activeElement !== mainDockStripLengthElement) {
    const length = Number.isFinite(view.mainDockStripLength) ? view.mainDockStripLength : 100;
    mainDockStripLengthElement.value = length;
    if (mainDockStripLengthStateElement) {
      mainDockStripLengthStateElement.textContent = `${length}%（主窗那条边长的百分比）`;
    }
  }
  if (document.activeElement !== mainDockStripOpacityElement) {
    const stripOpacity = Number.isFinite(view.mainDockStripOpacity)
      ? view.mainDockStripOpacity
      : 100;
    mainDockStripOpacityElement.value = stripOpacity;
    if (mainDockStripOpacityStateElement) {
      mainDockStripOpacityStateElement.textContent = `${stripOpacity}%（拖动即时预览）`;
    }
  }
  if (document.activeElement !== mainDockStripColorElement) {
    mainDockStripColorElement.value = shadeHex(view.mainDockStripColor) || "#ffffff";
  }
  if (document.activeElement !== mainHotzoneThicknessElement) {
    const hotzone = Number.isFinite(view.mainHotzoneThickness) ? view.mainHotzoneThickness : 40;
    mainHotzoneThicknessElement.value = hotzone;
    if (mainHotzoneThicknessStateElement) {
      mainHotzoneThicknessStateElement.textContent = `${hotzone}px（鼠标移入屏幕边往内这一段就呼出主窗）`;
    }
  }
  if (document.activeElement !== mainHotzoneColorElement) {
    mainHotzoneColorElement.value = shadeHex(view.mainHotzoneColor) || "#ff0000";
  }
  if (document.activeElement !== mainHotzoneOpacityElement) {
    const hotzoneOpacity = Number.isFinite(view.mainHotzoneOpacity) ? view.mainHotzoneOpacity : 0;
    mainHotzoneOpacityElement.value = hotzoneOpacity;
    if (mainHotzoneOpacityStateElement) {
      mainHotzoneOpacityStateElement.textContent = `${hotzoneOpacity}%（0 = 全透明看不见）`;
    }
  }
  mainHotzoneBorderElement.checked = Boolean(view.mainHotzoneBorder);
  if (document.activeElement !== mainHotzoneBorderColorElement) {
    mainHotzoneBorderColorElement.value = shadeHex(view.mainHotzoneBorderColor) || "#ff0000";
  }
  if (document.activeElement !== mainWindowPositionElement) {
    const mode = view.mainWindowPosition;
    mainWindowPositionElement.value =
      mode === "center" || mode === "last" ? mode : "free";
  }
  // 托盘菜单方式：classic（各自方式，默认）/ unified（全局统一）
  if (document.activeElement !== trayMenuStyleElement) {
    trayMenuStyleElement.value = view.trayMenuStyle === "unified" ? "unified" : "classic";
  }
  if (document.activeElement !== mainFixedMonitorElement) {
    syncFixedMonitorOptions(view.mainMonitorOptions, view.mainFixedMonitor);
  }
  syncWindowSizeControls(view);
  hideFromDockTaskbarElement.checked = Boolean(view.hideFromDockTaskbar);
  closeActionElement.value = view.closeAction === "quit" ? "quit" : "tray";
  autostartStateElement.textContent =
    autostartStateLabels.get(view.autostartState) || "";
  loginItemsElement.classList.toggle(
    "hidden",
    view.autostartState !== "requires-approval",
  );

  // 外观：主题 + 不透明度（快照是唯一真相，别让控件停在没生效的位置）
  const theme =
    view.theme === "light" || view.theme === "system" ? view.theme : "dark";
  const percent = Number.isInteger(view.opacityPercent)
    ? view.opacityPercent
    : 100;
  if (document.activeElement !== opacityElement) {
    opacityElement.value = String(percent);
  }
  themeElement.value = theme;
  currentAccent = shadeHex(view.accentColor) || defaultAccent(theme);
  if (document.activeElement !== accentColorElement) {
    accentColorElement.value = currentAccent;
  }
  applyAppearance(theme, Number(opacityElement.value));
}

// 「远程怎么调」里的示例带着**明文 token**。快照里刻意只放打码版（后端契约：
// 明文只能由 reveal_http_token 单独取），所以这里靠那些**由明文派生出来的字段**
// （打码 token / 地址 / 端口 / 监听状态）判断该重新取一次示例了 ——
// 保存或重新生成 token 之后，面板里的示例立刻跟着换，不会还留着一份过期命令。
let httpHintKey = null;

async function refreshHttpHint() {
  try {
    httpHintElement.textContent = await invoke("http_access_hint");
  } catch (error) {
    // 取不到就保留上一次的内容，别把面板清空
    void error;
  }
}

function refreshHttpHintIfChanged(http) {
  const key = [http.tokenHint, http.bind, http.port, http.listening, http.addr || ""].join("|");
  if (key === httpHintKey) {
    return;
  }
  httpHintKey = key;
  refreshHttpHint();
}

function renderRemote(http, settings) {
  httpEnabledElement.checked = Boolean(http.enabled);
  httpBindElement.value = http.bind || "";
  httpPortElement.value = String(http.port === undefined ? "" : http.port);
  // 监听状态由后端给（托盘菜单读的是同一句话），前端不再自己拼
  httpStateElement.textContent = http.stateText || "";
  tokenHintElement.textContent = `当前令牌：${http.tokenHint}`;
  refreshHttpHintIfChanged(http);

  notifyEnabledElement.checked = Boolean(settings.notifyEnabled);
  httpChannelNotifyElement.checked = Boolean(settings.channels.http.notify);
  cliChannelNotifyElement.checked = Boolean(settings.channels.cli.notify);
  remoteAllowGuiAuthElement.checked = Boolean(settings.remoteAllowGuiAuth);
  // 正在输入的框不要被快照覆盖
  if (document.activeElement !== httpChannelDelayElement) {
    httpChannelDelayElement.value = String(settings.channels.http.delaySeconds);
  }
  if (document.activeElement !== cliChannelDelayElement) {
    cliChannelDelayElement.value = String(settings.channels.cli.delaySeconds);
  }

  // 端口被占用：告诉用户是谁占的、能不能一键结束
  const occupant = http.portOccupant;
  if (occupant) {
    portOccupantElement.classList.remove("hidden");
    portOccupantMessageElement.textContent = occupant.message;
    portOccupantHintElement.textContent = occupant.killHint
      ? `等价命令：${occupant.killHint}`
      : "";
    // 占端口的是本程序自己时后端会拒绝「结束占用进程」，按钮直接藏掉 ——
    // 摆一个点了必然报错的按钮，只会让人以为程序坏了。
    killOccupantElement.classList.toggle("hidden", Boolean(occupant.isSelf));
    killOccupantElement.textContent = occupant.pid
      ? `结束 PID ${occupant.pid}`
      : "结束占用进程";
    killOccupantElement.disabled = false;
  } else {
    portOccupantElement.classList.add("hidden");
  }

  // 渲染提示音设置
  if (playSoundElement) {
    playSoundElement.checked = Boolean(settings.playSound);
  }
  if (soundFilePathElement) {
    soundFilePathElement.value = settings.soundFilePath || "";
  }

  httpRequestsElement.replaceChildren();
  for (const entry of http.requests) {
    const item = document.createElement("li");
    const clock = formatLocalTime(entry.time);
    item.textContent = `${clock} ${entry.source} [${entry.channel}] ${entry.summary} — ${entry.status}${
      entry.message ? `：${entry.message}` : ""
    }`;
    httpRequestsElement.append(item);
  }
  // 最新请求在最后：滚动条压到底，一眼看到最近发生的事
  httpRequestsElement.scrollTop = httpRequestsElement.scrollHeight;
}

function renderPending(notify) {
  if (!notify) {
    pendingCardElement.classList.add("hidden");
    return;
  }
  pendingCardElement.classList.remove("hidden");
  pendingTitleElement.textContent =
    notify.kind === "pending"
      ? `远程请求待执行：${notify.secondsLeft} 秒后自动执行`
      : notify.title;
  pendingDetailElement.textContent = `${notify.message}｜来源 ${notify.source}｜渠道 ${notify.channel}｜发起 ${formatLocalTime(notify.created)}`;
  pendingRunElement.classList.toggle("hidden", !notify.canCancel);
  pendingCancelElement.classList.toggle("hidden", !notify.canCancel);
  // 「立即执行」只有电脑本机能点：远程端能点 = 自己给自己盖章，授权窗就成了摆设
  // （后端也会拒，见 ui_command_is_local_only）。这里灰掉而不是藏起来 —— 藏起来
  // 像是「没这功能」，灰着 + 悬停提示才能让用户明白要回电脑前点。
  // 「取消」远程保留：撤回自己刚发的那条请求是安全方向的操作。
  pendingRunElement.disabled = IS_BROWSER;
  pendingRunElement.title = IS_BROWSER
    ? "需要在电脑本机上点（远程只能取消这条请求）"
    : "";
}

// 设置项联动：总开关关掉时，它的专属子项一律置灰（改了也不生效的东西不该看起来能改）。
// 只读勾选框现状——勾选框本身由快照统一写入，所以这里不用参数；
// render() 每拍收尾跑一次兜住快照驱动，四个总开关的 change 监听里也各调一次让灰化跟手。
function applySettingLinkage() {
  for (const element of [
    httpChannelNotifyElement,
    httpChannelDelayElement,
    cliChannelNotifyElement,
    cliChannelDelayElement,
  ]) {
    element.disabled = !notifyEnabledElement.checked;
  }
  for (const element of [
    httpBindElement,
    httpPortElement,
    httpApplyElement,
    tokenInputElement,
    tokenSaveElement,
    tokenRevealElement,
    tokenCopyElement,
    tokenRegenerateElement,
    retryHttpElement,
    killOccupantElement,
  ]) {
    element.disabled = !httpEnabledElement.checked;
  }
  // 「打开 Web 页面」只在 HTTP 服务开着时可用（关着就没有页面可开）
  if (openWebElement) {
    openWebElement.disabled = !httpEnabledElement.checked;
  }
  for (const element of [soundFilePathElement, soundApplyElement]) {
    element.disabled = !playSoundElement.checked;
  }
  // 贴边自动隐藏与「屏幕中间」互斥：居中要求窗口钉在屏幕正中，贴边要求窗口
  // 缩到屏幕边上，两个功能会互相拉扯（后端同样拒绝在居中模式下贴边）。
  // 居中选中时整个开关灰掉，别让用户勾了却看不到任何效果。
  const centerLocked = mainWindowPositionElement.value === "center";
  mainAutoHideElement.disabled = centerLocked;
  if (mainAutoHideStateElement) {
    mainAutoHideStateElement.classList.toggle("hidden", !centerLocked);
    mainAutoHideStateElement.textContent = centerLocked
      ? "与「屏幕中间」互斥：居中期间贴边不可用，切回其他落位方式后自动恢复"
      : "";
  }
  for (const element of [
    mainDockStripHoverMsElement,
    mainDockStripEdgeGapElement,
    mainDockStripThicknessElement,
    mainDockStripLengthElement,
    mainDockStripOpacityElement,
    mainDockStripColorElement,
  ]) {
    element.disabled = centerLocked || !mainAutoHideElement.checked;
  }
}

function render(state) {
  const busy = isBusy(state.busy);
  // 渲染心跳：把「这次渲染画了什么」回传后端 trace。沙箱环境拿不到截图，
  // 「主界面数据为空」这类问题只能靠它判断是渲染层没跑还是画了被盖。
  // 只在主窗启用；上报失败静默（不能让诊断破坏渲染）。
  if (!IS_BROWSER) {
    try {
      void tauri.core.invoke("sol_log_client_error", {
        kind: "render",
        detail: `rev=${state.revision} vars=${(state.status.variables || []).length} busy=${JSON.stringify(busy)}`,
      });
    } catch {
      void busy;
    }
  }

  // 操作回执统一走底部固定提示条（不再写在正文末尾，用户看不看得见全凭滚轮）
  syncStatusBarFromState(state);
  directoryElement.value = state.status.varsDir || "";
  unmountTemporaryElement.classList.toggle(
    "hidden",
    !state.status.temporaryMount,
  );

  renderSystems(state.status, busy, state.grub);
  // 顶部那行状态文案：扫描成功后 status.message 就是「下次启动：X」，
  // 而下面的大字已经把下次启动说全了（级联时还带 GRUB 启动项，信息更多）
  // —— 用户定案 2026-10-13：凡是「下次启动：」开头的都算重复，不再显示。
  const statusText = (state.status.message || "").trim();
  const duplicatesNextBoot = !statusText || statusText.startsWith("下次启动：");
  const label = busyLabel(state.busy);
  statusElement.textContent = label || (duplicatesNextBoot ? "" : statusText);
  statusElement.classList.toggle("hidden", !label && duplicatesNextBoot);

  renderGrub(
    state.grub,
    state.grubScript,
    busy,
    state.settings && state.settings.grubListView,
  );
  renderSettings(state.settings);
  renderRemote(state.http, state.settings);
  renderPending(state.notify);

  // 动作进行中两边一起禁用（托盘那边由后端按同一份 busy 置灰）。
  for (const element of [
    refreshElement,
    ...powerButtons.map(([button]) => button),
    restartAppElement,
    quitAppElement,
    unmountTemporaryElement,
    useDirectoryElement,
    autostartElement,
    startInTrayElement,
    hideFromDockTaskbarElement,
    closeActionElement,
    loginItemsElement,
    grubPartuuidSaveElement,
    grubPartuuidDetectElement,
    grubSyncElement,
    grubDirDetectElement,
    grubDirSaveElement,
    grubDirPickElement,
    grubEmptyPickElement,
    grubDirOpenElement,
    grubInstallElement,
    grubUninstallElement,
    grubScriptSaveElement,
    grubScriptResetElement,
  ]) {
    element.disabled = busy;
  }

  // 快照驱动的设置项联动收尾（renderRemote 里 killOccupant 的 disabled 会被这里按 http 开关纠正）
  applySettingLinkage();
}

function applySnapshot(state, { force = false } = {}) {
  if (!state || typeof state.revision !== "number") {
    // 结构不对（后端刚起来 / 版本不匹配）：别静默什么都不做 —— 客户端会停在
    // 「一片空白」且不给任何线索。只在从未成功渲染过时说一句，别刷屏。
    if (!hasRenderedSnapshot) {
      showStatusBar("已连上本机服务，但状态快照不可用，请刷新页面重试", {
        error: true,
      });
    }
    return;
  }
  hasRenderedSnapshot = true;
  if (!force && state.revision <= lastRevision) {
    return;
  }
  lastRevision = Math.max(lastRevision, state.revision);
  currentState = state;
  render(state);
  applyDockStrip(state);
  markReady();
}

/// 首帧渲染完成 → 解除 body 的隐藏（见 styles.css 的 html[data-ready]）。
/// 只在第一次调用时生效；之后不再动，避免每拍都触发样式写。
let readyMarked = false;
function markReady() {
  if (readyMarked) {
    return;
  }
  readyMarked = true;
  document.documentElement.dataset.ready = "1";
}

/// 兜底：万一快照迟迟不来（后端异常/预览模式），1.2 秒后也把页面显示出来，
/// 免得用户对着透明窗口发愣。此时至少能看到框架和状态条的错误提示。
setTimeout(markReady, 1200);

// 后端统一记 UTC ISO 串（…T…Z）。Webview 的时区 = 系统时区，用 toLocaleString
// 转成本地时间显示，与 Windows 任务栏 / macOS 菜单栏 / Linux 桌面时钟一致。
// 解析失败时原样返回，别把界面搞出 "Invalid Date"。
function formatLocalTime(iso) {
  if (!iso || typeof iso !== "string") return iso || "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString([], { hour12: false });
}

// 贴边隐藏时，白条是独立渲染的白色条（#dock-strip）。前端负责：① 按
// dockEdge 打方向类；② 写三个 CSS 变量：长度（窗口边百分比）、厚度（px）、
// 不透明度 —— 都与窗口几何无关，设置拖动即时改。
function applyDockStrip(state) {
  const body = document.body;
  body.classList.remove("dock-top", "dock-bottom", "dock-left", "dock-right");
  const settings = state.settings || {};
  const clampInt = (value, min, max, fallback) => {
    const parsed = Math.round(Number(value));
    return Number.isFinite(parsed) ? Math.min(Math.max(parsed, min), max) : fallback;
  };
  const thickness = clampInt(settings.mainDockStripThickness, 2, 40, 6);
  const length = clampInt(settings.mainDockStripLength, 10, 100, 100);
  const opacity = clampInt(settings.mainDockStripOpacity, 10, 100, 100);
  document.documentElement.style.setProperty("--dock-strip-thickness", `${thickness}px`);
  document.documentElement.style.setProperty("--dock-strip-length", `${length}%`);
  document.documentElement.style.setProperty("--dock-strip-opacity", String(opacity / 100));
  const color = typeof settings.mainDockStripColor === "string" ? settings.mainDockStripColor : "";
  document.documentElement.style.setProperty(
    "--dock-strip-color",
    shadeHex(color) || "#ffffff",
  );
  // 热区可视化：判定矩形从**屏幕边**往内 hotzone 厚，但可视化层画在条形窗口
  // 里 —— 窗口离屏幕边还隔一条 gap 缝，所以窗口内可见厚度 = hotzone - gap。
  // 窗口缝厚由后端按 max(槽位 40, 热区) 重贴配合，这里不用 clamp 上限：
  // 渲染的矩形必须与后端判定的热区矩形是同一个。
  const hotzone = clampInt(settings.mainHotzoneThickness, 1, 100, 40);
  const gap = clampInt(settings.mainDockStripEdgeGap, 0, 100, 0);
  document.documentElement.style.setProperty(
    "--hotzone-thickness",
    `${Math.max(hotzone - gap, 0)}px`,
  );
  document.documentElement.style.setProperty(
    "--hotzone-color",
    shadeHex(settings.mainHotzoneColor) || "#ff0000",
  );
  document.documentElement.style.setProperty(
    "--hotzone-opacity",
    String(clampInt(settings.mainHotzoneOpacity, 0, 100, 0) / 100),
  );
  document.documentElement.style.setProperty(
    "--hotzone-border-color",
    shadeHex(settings.mainHotzoneBorderColor) || "#ff0000",
  );
  document.documentElement.style.setProperty(
    "--hotzone-border-width",
    settings.mainHotzoneBorder ? "1px" : "0px",
  );
  if (state.docked && state.dockEdge) {
    body.classList.add("dock-" + state.dockEdge);
    // 渲染取证：白条的实际矩形 + 视口/缩放报给后端落日志（多屏排障——
    // 「白条画在哪」必须与后端的窗口物理矩形对拍，肉眼描述定位不了根因）
    requestAnimationFrame(() => {
      const strip = document.getElementById("dock-strip");
      if (!strip) return;
      const rect = strip.getBoundingClientRect();
      invoke("report_dock_strip_rect", {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
        viewportWidth: window.innerWidth,
        viewportHeight: window.innerHeight,
        dpr: window.devicePixelRatio,
      }).catch(() => {});
    });
  }
}

// 命令跑完主动拉一次快照：事件推送偶尔会落在命令返回之后，
// 主动拉一次能保证「状态文案 / 监听地址」这些刚变的东西立刻显示出来。
async function refreshSnapshotQuietly() {
  try {
    applySnapshot(await invoke("get_ui_state"), { force: true });
  } catch (error) {
    // 拉不到就等事件推送，别把界面清空
    void error;
  }
}

// 所有命令调用都走这里：失败时除了显示原因，还要把界面拉回系统真实状态
// （勾选框不能停在「没生效」的位置 —— 阶段 1 修 D5 的前端一半）。
async function call(command, args) {
  hideStatusBar();
  try {
    // 返回值给调用方用：远程模式下有些命令是「已受理、待本机确认」而不是已执行
    // （例如网页端选择系统，见 renderSystems），调用方要据 pending 给个说法。
    return await invoke(command, args);
  } catch (error) {
    showStatusBar(String(error), { error: true });
    try {
      applySnapshot(await invoke("get_ui_state"), { force: true });
    } catch (inner) {
      // 主错误已经显示，这里不再覆盖
      void inner;
    }
    return undefined;
  }
}

refreshElement.addEventListener("click", () => call("refresh"));

// ---------- 界面栏目页签（地址栏可寻址） ----------
// 主窗口内容分三个栏目：系统引导（rEFInd + GRUB 同页，主功能）、
// 基本设置（应用设置 + 远程控制）、SOL 远程唤醒（sol 接收器管理）。
// 切换纯前端，但**栏目写进地址栏 hash**：
//   #/boot  → 系统引导
//   #/settings → 基本设置
//   #/sol   → SOL 远程唤醒
// 这样刷新页面、收藏、把链接发给别人，都落回同一个栏目里。
const TAB_ROUTES = { boot: "#/boot", settings: "#/settings", sol: "#/sol" };
const tabButtons = Array.from(document.querySelectorAll("#tabs .tab"));
const tabPanels = {
  boot: document.querySelector("#tab-boot"),
  settings: document.querySelector("#tab-settings"),
  sol: document.querySelector("#tab-sol"),
};

// 从地址栏 hash 读栏目名；不是已知栏目一律返回空串（脏 hash 不生效）。
function tabFromLocation() {
  const raw = String(window.location.hash || "")
    .replace(/^#\/?/, "")
    .trim()
    .toLowerCase();
  return tabPanels[raw] ? raw : "";
}

// 把栏目写回地址栏。用 replaceState 顶掉当前历史项而不是新压一条：
// 只有两个栏目，来回点不该把浏览器历史塞满；刷新/分享链接照样生效。
function syncTabToLocation(name) {
  const route = TAB_ROUTES[name];
  if (!route || window.location.hash === route) {
    return;
  }
  try {
    window.history.replaceState(null, "", route);
  } catch (error) {
    void error;
    window.location.hash = route;
  }
}

function activateTab(name, options = {}) {
  if (!tabPanels[name]) {
    return;
  }
  for (const button of tabButtons) {
    button.classList.toggle("active", button.dataset.tab === name);
  }
  for (const [key, panel] of Object.entries(tabPanels)) {
    panel.classList.toggle("active", key === name);
  }
  // GRUB 独立右栏（展示方式 ③④）只属于引导栏目：切到设置页时收起，
  // 切回时清掉内联样式，显隐交还原有的展示模式机制接管。
  const grubPane = document.querySelector(".pane-grub");
  if (grubPane) {
    grubPane.style.display = name === "boot" ? "" : "none";
  }
  // SOL 面板按需拉取：只在**进**这个栏目时读一次 sol 状态（独立服务，
  // 不进 UiState 快照），离开不动作，回来再刷。
  if (name === "sol") {
    solPkgTargetElement.textContent = SOL_PKG_TARGET;
    renderSolDir();
    refreshSolPanel();
    refreshSolHttp();
    refreshSolPackage();
    refreshSolProjectInfo();
    loadSolLogs();
  }
  // 不再记忆栏目：启动一律回「系统引导」（用户定案 2026-10-19）。
  if (options.syncLocation !== false) {
    syncTabToLocation(name);
  }
}

for (const button of tabButtons) {
  button.addEventListener("click", () => activateTab(button.dataset.tab));
}
// 地址栏 hash 被外部改动（手敲地址、浏览器前进/后退）也要跟着切栏目。
window.addEventListener("hashchange", () => {
  const name = tabFromLocation();
  if (name) {
    activateTab(name, { syncLocation: false });
  }
});

// 初始栏目：**一律进「系统引导」**（用户定案 2026-10-19）。
//
// 以前的优先级是「地址栏 hash > 桌面端 localStorage 记忆」，导致桌面主窗每次
// 启动都停在上次那个栏目（上次退出在 SOL，重启还进 SOL）。主窗内容的主功能
// 就是系统引导，启动就该回到它；「记住上次栏目」那条规则不再生效。
//
// 保留 hash 的唯一用途：**同一次运行中刷新页面**（dev 热重载 / 手动刷新）
// 不跳栏目 —— 由下面的 boot 判断实现，而不是靠 initialState。
const TAB_STORAGE_KEY_LEGACY = "anyboot.active-tab";
try {
  // 清掉历史遗留的记忆，免得老用户机器上的旧值继续被别处读到
  localStorage.removeItem(TAB_STORAGE_KEY_LEGACY);
} catch (error) {
  void error;
}

function initialTab() {
  // 只在本页面**已经处于某个栏目路由**且是刷新场景时才沿用；
  // 冷启动（hash 为空或根路径）一律系统引导。
  const fromLocation = tabFromLocation();
  if (fromLocation && window.location.hash) {
    return fromLocation;
  }
  return "boot";
}
// 初始栏目**不能在这里立即激活**：activateTab 进 SOL 栏目会立刻调
// refreshSolPanel()/refreshSolHttp()，而它们用到的 solStateTextElement 等
// const 声明在下面几十行之后 —— TDZ 下直接 ReferenceError，面板永久停在
// 「正在读取…」（偶现：只有上次停在 SOL 栏目时触发）。改到脚本末尾激活。
let pendingInitialTab = initialTab();

// ---------- SOL 远程唤醒标签页（sol 接收器管理） ----------
// 面板状态不进 UiState 快照：sol 是独立服务，按需拉取（进页时 / 刷新按钮 / 动作后）。
// 配置编辑器的「脏」状态只留在前端 —— 保存走 sol_config_write，后端备份后原子写。
const solStateTextElement = document.querySelector("#sol-state-text");
const solRefreshElement = document.querySelector("#sol-refresh");
const solRestartServiceElement = document.querySelector("#sol-restart-service");
const solStatusPrivilegedElement = document.querySelector("#sol-status-privileged");
const solOneClickTerminalElement = document.querySelector("#sol-one-click-terminal");
const solInstallFromPkgTerminalElement = document.querySelector("#sol-install-from-pkg-terminal");
const solInstallHintElement = document.querySelector("#sol-install-hint");
const solConfigPathElement = document.querySelector("#sol-config-path");
const solConfigTextElement = document.querySelector("#sol-config-text");
const solConfigSaveElement = document.querySelector("#sol-config-save");
const solConfigReloadElement = document.querySelector("#sol-config-reload");
const solConfigRefreshElement = document.querySelector("#sol-config-refresh");
// 安装包：下载 → 解压 → 安装，目录可自定义
const solPkgStateElement = document.querySelector("#sol-pkg-state");
const solProjectRepoElement = document.querySelector("#sol-project-repo");
const solProjectReleasesElement = document.querySelector("#sol-project-releases");
const solProjectVersionElement = document.querySelector("#sol-project-version");
const solPkgTargetElement = document.querySelector("#sol-pkg-target");
const solDirInputElement = document.querySelector("#sol-dir-input");
const solDirStateElement = document.querySelector("#sol-dir-state");
const solOpenDirInputElement = document.querySelector("#sol-open-dir-input");
const solPickDirElement = document.querySelector("#sol-pick-dir");
const solDownloadElement = document.querySelector("#sol-download");
const solPickArchiveElement = document.querySelector("#sol-pick-archive");
const solExtractElement = document.querySelector("#sol-extract");
const solInstallFromPkgElement = document.querySelector("#sol-install-from-pkg");
const solPkgHintElement = document.querySelector("#sol-pkg-hint");
const solOneClickElement = document.querySelector("#sol-one-click");
const solProgressElement = document.querySelector("#sol-progress");
const solProgressFillElement = document.querySelector("#sol-progress-fill");
const solProgressTextElement = document.querySelector("#sol-progress-text");
const solLogElement = document.querySelector("#sol-log");
const solLogCountElement = document.querySelector("#sol-log-count");
const solLogClearElement = document.querySelector("#sol-log-clear");
// HTTP 控制面三选项（与 sol.yaml 双向联动）
const solHttpEnabledElement = document.querySelector("#sol-http-enabled");
const solHttpEnabledStateElement = document.querySelector("#sol-http-enabled-state");
const solHttpListenElement = document.querySelector("#sol-http-listen");
const solHttpListenStateElement = document.querySelector("#sol-http-listen-state");
const solHttpListenSaveElement = document.querySelector("#sol-http-listen-save");
const solHttpTokenElement = document.querySelector("#sol-http-token");
const solHttpTokenStateElement = document.querySelector("#sol-http-token-state");
const solHttpTokenCurrentElement = document.querySelector("#sol-http-token-current");
const solHttpTokenCopyElement = document.querySelector("#sol-http-token-copy");
const solHttpTokenGenerateElement = document.querySelector("#sol-http-token-generate");
const solHttpTokenSaveElement = document.querySelector("#sol-http-token-save");

// 编辑器正在展示的配置文件路径（保存时回传后端）
let solConfigEditingPath = "";
// 编辑器是否已被用户改脏（改脏后切走再切回来不覆盖用户未保存的修改）
let solConfigDirty = false;
// 安装包工作目录（下载落盘 + 解压 + 安装源）：localStorage 记住，跨启动沿用
const SOL_DIR_KEY = "anyboot.sol-dir";
let solDir = (() => {
  try {
    return localStorage.getItem(SOL_DIR_KEY) || "";
  } catch (error) {
    void error;
    return "";
  }
})();
// 最近下载/解压出的包路径（解压按钮只在有包时亮）
let solPkgArchivePath = "";
// 日志行数上限（超过就丢最老的，防长会话内存涨）
const SOL_LOG_MAX_LINES = 200;
let solLogLines = 0;

// ---------- 日志区 ----------
// 两路来源：① 后端 sol-log 事件（实时）；② sol_logs 命令（进页面回填历史）。
function solLogAppend(message, time) {
  if (solLogLines === 0) {
    solLogElement.textContent = "";
  }
  const line = document.createElement("span");
  line.className = "sol-log-line";
  const stamp = document.createElement("span");
  stamp.className = "sol-log-time";
  stamp.textContent = (time || solNowTime()) + " ";
  line.append(stamp, document.createTextNode(message));
  solLogElement.append(line);
  solLogLines += 1;
  // 超限从前面摘
  while (solLogLines > SOL_LOG_MAX_LINES) {
    if (solLogElement.firstChild) {
      solLogElement.removeChild(solLogElement.firstChild);
    }
    solLogLines -= 1;
  }
  solLogElement.scrollTop = solLogElement.scrollHeight;
  solLogCountElement.textContent = `${solLogLines} 条`;
}

// 路径的最后一段（文件名）。后端回的是原生路径：Windows 用 \，Linux/macOS 用 /，
// 以前只按 "/" 切，Windows 上「已下载：」后面会跟着整条绝对路径。
function solBaseName(path) {
  const text = String(path || "");
  const index = Math.max(text.lastIndexOf("/"), text.lastIndexOf("\\"));
  return index >= 0 ? text.slice(index + 1) : text;
}

function solNowTime() {
  const now = new Date();
  const pad = (value) => String(value).padStart(2, "0");
  return `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
}

solLogClearElement.addEventListener("click", () => {
  solLogElement.textContent = "（还没有日志）";
  solLogLines = 0;
  solLogCountElement.textContent = "0 条";
});

// ---------- 下载进度条 ----------
// 后端每 500ms 推一条 sol-download-progress { bytes, total, done, ok }。
function solProgressShow(visible) {
  solProgressElement.hidden = !visible;
}

function solProgressPaint(bytes, total) {
  if (total && total > 0) {
    const percent = Math.min(100, Math.round((bytes / total) * 100));
    solProgressFillElement.style.width = `${percent}%`;
    const mb = (bytes / 1048576).toFixed(1);
    const mbTotal = (total / 1048576).toFixed(1);
    solProgressTextElement.textContent = `${percent}%（${mb}/${mbTotal} MB）`;
  } else {
    // 总大小未知：只显示已落盘字节，进度条走"流动"样式
    solProgressFillElement.style.width = "100%";
    solProgressFillElement.style.background =
      "repeating-linear-gradient(90deg, var(--btn, #6366f1) 0 12px, var(--btn-hover, #7070f2) 12px 24px)";
    solProgressTextElement.textContent = `${(bytes / 1048576).toFixed(1)} MB`;
    return;
  }
  solProgressFillElement.style.background = "var(--btn, var(--accent, #6366f1))";
}

function solProgressDone(ok) {
  if (ok) {
    solProgressFillElement.style.width = "100%";
    solProgressFillElement.style.background = "var(--ok, #22c55e)";
    solProgressTextElement.textContent = "完成";
  }
  setTimeout(() => solProgressShow(false), ok ? 1500 : 200);
}

// ---------- 事件监听（sol-log / sol-download-progress） ----------
if (!IS_BROWSER) {
  listen("sol-log", (event) => {
    const { message, time } = event.payload || {};
    solLogAppend(String(message || ""), time);
  });
  listen("sol-download-progress", (event) => {
    const { bytes, total, done, ok } = event.payload || {};
    if (done) {
      solProgressDone(Boolean(ok));
      return;
    }
    solProgressShow(true);
    solProgressPaint(Number(bytes) || 0, Number(total) || 0);
  });
}

// 进页面回填历史日志（trace.log 里本会话的 [sol] 段，倒序取 50 条）
async function loadSolLogs() {
  if (IS_BROWSER) {
    return;
  }
  try {
    const logs = await invoke("sol_logs");
    if (Array.isArray(logs) && logs.length) {
      solLogElement.textContent = "";
      solLogLines = 0;
      for (const entry of logs) {
        solLogAppend(String(entry));
      }
    }
  } catch (error) {
    void error; // 历史日志取不到不碍事，实时日志照收
  }
}

// 用一次状态拉取渲染整个面板顶部（状态行）。配置编辑器只有在
// 「没有未保存修改」时才跟随刷新 —— 用户正在改的草稿不被后台数据冲掉。
/// 回填「安装包」一栏：本地已有包 → 显示已下载 + 进度条 100%。
/// 进页面时调一次（之前只在点了下载/解压那一刻才写文案，刷新就丢）。
/// 回填 sol 项目信息（仓库 / Releases / 当前跟随版本），并接上打开链接。
async function refreshSolProjectInfo() {
  if (IS_BROWSER) {
    // 网页端不打开本机浏览器：链接直接指向 GitHub
    solProjectRepoElement.href = "https://github.com/lidiaoo/sol";
    solProjectReleasesElement.href = "https://github.com/lidiaoo/sol/releases";
    return;
  }
  try {
    const info = await invoke("sol_project_info");
    solProjectVersionElement.textContent = `v${info.version}（${info.asset}）`;
    solProjectVersionElement.title = info.asset;
    for (const [element, page] of [
      [solProjectRepoElement, "repo"],
      [solProjectReleasesElement, "releases"],
    ]) {
      element.addEventListener("click", async (event) => {
        event.preventDefault();
        try {
          await invoke("sol_project_open", { page });
        } catch (error) {
          showStatusBar(String(error), { error: true });
        }
      });
    }
  } catch (error) {
    void error;
  }
}

async function refreshSolPackage() {
  if (IS_BROWSER) {
    return;
  }
  try {
    const local = await invoke("sol_local_package", { dir: solDir || null });
    if (local.archive) {
      const name = solBaseName(local.archive) || local.archive;
      const mb = (local.size / 1024 / 1024).toFixed(1);
      solPkgStateElement.textContent = local.verified
        ? `已下载：${name}（${mb} MB，md5 校验通过）`
        : `已有安装包：${name}（${mb} MB）`;
      solPkgStateElement.title = local.archive;
      // 包已经在盘上就直接可解压：不再要求先点一次「下载」才能解压。
      // （以前这里只回填文案，解压按钮用的路径仍为空 → 盘上有包也点不动。）
      solPkgArchivePath = local.archive;
      // 已有包 = 下载已完成：进度条直接画满（100%）
      solProgressShow(true);
      solProgressPaint(local.size, local.size);
    } else if (local.extracted) {
      solPkgStateElement.textContent = `已解压就绪：${local.dir}`;
      solPkgStateElement.title = local.dir;
    } else {
      solPkgStateElement.textContent = "未下载";
      solPkgStateElement.removeAttribute("title");
    }
    // 下载按钮文案跟着状态走：下载过 → 「重新下载」；没下载过 → 「下载」。
    // 只改文案不改 id，事件监听绑的是 sol-download，不受影响。
    solDownloadElement.textContent = local.archive ? "重新下载" : "下载";
    // 「安装（静默）/（调终端）」任何情况都亮着（用户定案）：未解压时点它后端会给出
    // 明确报错提示，比灰着让用户猜强。这里不再按 extracted 禁用它们。
    solExtractElement.disabled = !local.archive;
    if (local.extracted) {
      solPkgHintElement.dataset.root = local.dir;
    }
  } catch (error) {
    void error;
  }
}

async function refreshSolPanel() {
  if (IS_BROWSER) {
    // 远程 / 网页端不透出（与「重启程序」同一策略：面板命令只在本机窗口放行）
    solStateTextElement.textContent = "远程模式不可用（仅本机主窗口）";
    solInstallHintElement.textContent = "sol 管理仅在本机主窗口开放，网页远程端只读不可操作。";
    for (const element of [
      solRefreshElement,
      solRestartServiceElement,
      solOneClickTerminalElement,
      solConfigSaveElement,
      solConfigReloadElement,
      solStatusPrivilegedElement,
      solConfigTextElement,
      solPickDirElement,
      solDownloadElement,
      solExtractElement,
      solInstallFromPkgElement,
      solOneClickElement,
      solPickArchiveElement,
      solLogClearElement,
      solDirInputElement,
      solOpenDirInputElement,
      solHttpEnabledElement,
      solHttpListenElement,
      solHttpListenSaveElement,
      solHttpTokenElement,
      solHttpTokenGenerateElement,
      solHttpTokenSaveElement,
    ]) {
      element.disabled = true;
    }
    return;
  }
  for (const element of [solRefreshElement, solRestartServiceElement]) {
    element.disabled = true;
  }
  try {
    const status = await invoke("sol_status");
    renderSolStatus(status);
    // 配置编辑器：只在「没在编辑」或路径变了时加载
    if (!solConfigDirty || !status.configPath || status.configPath !== solConfigEditingPath) {
      await loadSolConfig(status.configPath);
    }
  } catch (error) {
    solStateTextElement.textContent = `读取状态失败：${error}`;
    solStateTextElement.classList.add("setting-state-error");
  } finally {
    for (const element of [solRefreshElement, solRestartServiceElement]) {
      element.disabled = false;
    }
  }
}

// 状态被权限挡住时：用户显式点击 → 弹一次授权 → 读回真实状态
solStatusPrivilegedElement.addEventListener("click", async () => {
  solStatusPrivilegedElement.disabled = true;
  try {
    const status = await invoke("sol_status_privileged");
    renderSolStatus(status);
    showStatusBar("已通过授权读取服务状态");
  } catch (error) {
    showStatusBar(String(error), { error: true });
  } finally {
    solStatusPrivilegedElement.disabled = false;
  }
});

function renderSolStatus(status) {
  // 运行状态一行话：版本 + 服务 + 进程
  const parts = [];
  if (status.installed) {
    parts.push(status.version || "已安装");
  } else {
    parts.push("未安装");
  }
  if (status.serviceLoaded) {
    parts.push(status.serviceActive ? "服务运行中" : "服务已停止");
  } else if (status.installed) {
    parts.push("服务未注册");
  }
  if (!status.serviceActive && status.processRunning) {
    parts.push("进程在跑（非服务方式）");
  }
  if (status.statusNeedsPrivilege) {
    parts.push("状态需授权查看");
  }
  solStateTextElement.textContent = parts.join(" · ");
  solStateTextElement.classList.toggle("setting-state-error", !status.installed || !status.serviceActive);
  // 只有「权限挡住」时才需要用户点一下授权（不是自动弹）
  solStatusPrivilegedElement.classList.toggle("hidden", !status.statusNeedsPrivilege);
  if (status.serviceDetail) {
    solStateTextElement.title = status.serviceDetail;
  } else {
    solStateTextElement.removeAttribute("title");
  }

  // 安装 / 卸载的可用性与提示
  solInstallHintElement.textContent = status.installed
    ? `已安装在 ${status.binPath || "未知路径"}；重新安装会覆盖二进制与服务注册（安装目录里的 sol.yaml 原文件有备份）。`
    : "先「一键安装」（下载 → 解压到安装目录 → 官方 install.sh）；也可先「下载安装包」「选择安装包…」再解压、安装。安装目录看上面那一行。";
}

async function loadSolConfig(path, { force = false } = {}) {
  solConfigPathElement.textContent = "读取中…";
  try {
    const result = await invoke("sol_config_read", { path: path || null });
    solConfigEditingPath = result.path;
    solConfigPathElement.textContent = result.path;
    solConfigTextElement.value = result.content;
    solConfigDirty = false;
    solConfigTextElement.disabled = false;
    solConfigSaveElement.disabled = false;
    solConfigReloadElement.disabled = false;
  } catch (error) {
    solConfigPathElement.textContent = "未找到配置文件";
    solConfigTextElement.value = "";
    solConfigTextElement.placeholder = String(error);
    solConfigTextElement.disabled = true;
    solConfigSaveElement.disabled = true;
    solConfigReloadElement.disabled = true;
  }
}

// 用户改了编辑器 → 标脏；此后后台刷新不再覆盖草稿，直到保存或放弃修改
solConfigTextElement.addEventListener("input", () => {
  solConfigDirty = true;
});

solRefreshElement.addEventListener("click", () => refreshSolPanel());

solRestartServiceElement.addEventListener("click", async () => {
  solRestartServiceElement.disabled = true;
  try {
    showStatusBar("正在重启 sol 服务…");
    const message = await call("sol_service_ctl", { action: "restart" });
    showStatusBar(message || "sol 服务已重启");
  } finally {
    solRestartServiceElement.disabled = false;
  }
  refreshSolPanel();
});

// 当前安装目录输入框的值（null = 留空走后端默认 ~/sol）
function solDirArg() {
  const value = solDirInputElement.value.trim();
  return value ? value : null;
}

// 安装/解压等动作完成后把面板整个刷一遍：refreshSolPanel 内部会在
// 「没在编辑」时重读配置文件，安装刚生成的 sol.yaml 立刻显示出来。
// force = true 时无视「正在编辑」保护（安装会重写盘上内容，旧草稿没意义）。
function refreshSolAfterAction({ forceConfig = false } = {}) {
  if (forceConfig) {
    solConfigDirty = false;
  }
  refreshSolPanel();
}

solOneClickTerminalElement.addEventListener("click", async () => {
  solOneClickTerminalElement.disabled = true;
  try {
    showStatusBar("① 下载 → ② 解压 → ③ 呼出终端跑官方安装脚本…");
    const message = await call("sol_one_click_install_terminal", { dir: solDirArg() });
    showStatusBar(message || "已呼出终端");
  } finally {
    solOneClickTerminalElement.disabled = false;
  }
  refreshSolPanel();
});

solConfigSaveElement.addEventListener("click", async () => {
  if (!solConfigEditingPath) {
    return;
  }
  solConfigSaveElement.disabled = true;
  try {
    showStatusBar("正在保存 sol 配置…");
    const message = await call("sol_config_write", {
      path: solConfigEditingPath,
      content: solConfigTextElement.value,
    });
    showStatusBar(message || "已保存");
    // 保存成功后强制重载编辑器：盘上内容已是权威版本，textarea 必须跟盘同步，
    // 否则下次保存会把这份旧快照再写回去（覆盖掉选项写入的任何改动）。
    solConfigDirty = false;
    loadSolConfig(solConfigEditingPath, { force: true });
    // 编辑器保存也可能改了 http 段（enabled/listen/token）——选项一并刷新
    refreshSolHttp();
  } catch (error) {
    // 保存失败：保持编辑器可编辑，让用户改完再存
    solConfigSaveElement.disabled = false;
    return;
  }
  solConfigSaveElement.disabled = false;
});

solConfigReloadElement.addEventListener("click", () => {
  solConfigDirty = false;
  loadSolConfig(solConfigEditingPath || null);
});

// 刷新配置：从盘上重读（无视正在编辑的草稿 —— 用户显式点了刷新，就是要以盘为准）
solConfigRefreshElement.addEventListener("click", async () => {
  solConfigRefreshElement.disabled = true;
  try {
    solConfigDirty = false;
    await loadSolConfig(null); // 传 null 走后端最新探测（安装换了目录也能跟上）
    refreshSolHttp();
  } finally {
    solConfigRefreshElement.disabled = false;
  }
});

// ---------- HTTP 控制面三选项（与 sol.yaml 双向联动） ----------
// 读：sol_http_get 解析 YAML → 回填开关/输入框/状态字；
// 写：任一选项改动 → sol_http_set 单键写回（自动备份 + pkexec 提权），
// sol watch 到文件变化自动热重载；其它选项不受影响。
// 非内联 token 模式（token_env / token_file）输入框照常可填，应用后配置切回
// 内联 token —— 状态字里说明这一点。
// 任何写入成功后都调 syncEditorToDisk()：配置编辑器里的旧快照必须作废重读，
// 否则编辑器「保存配置」会把旧内容整篇写回，覆盖掉刚写入的选项（踩过的坑：
// 取消勾选 → 编辑器保存旧快照 → enabled 复活为 true）。
function syncEditorToDisk() {
  if (!IS_BROWSER && solConfigEditingPath) {
    solConfigDirty = false;
    loadSolConfig(solConfigEditingPath, { force: true });
  }
}

async function refreshSolHttp() {
  if (IS_BROWSER) {
    solHttpEnabledStateElement.textContent = "远程模式不可用（仅本机主窗口）";
    for (const element of [solHttpEnabledElement, solHttpListenElement, solHttpListenSaveElement, solHttpTokenElement, solHttpTokenGenerateElement, solHttpTokenSaveElement, solHttpTokenCurrentElement, solHttpTokenCopyElement]) {
      element.disabled = true;
    }
    return;
  }
  try {
    const http = await invoke("sol_http_get");
    solHttpEnabledElement.checked = Boolean(http.enabled);
    solHttpEnabledStateElement.textContent = http.enabled ? "已开启（sol 控制面在跑）" : "已关闭";
    if (document.activeElement !== solHttpListenElement) {
      solHttpListenElement.value = http.listen || "";
    }
    // 当前 token 展示 + 复制（token_env / token_file 模式下原文在别处，提示之）
    if (http.authKind === "token") {
      solHttpTokenCurrentElement.value = http.token || "";
      solHttpTokenCopyElement.disabled = !http.token;
    } else {
      solHttpTokenCurrentElement.value = "";
      solHttpTokenCurrentElement.placeholder = `（${http.authKind} 模式，原文不在此文件）`;
      solHttpTokenCopyElement.disabled = true;
    }
    solHttpTokenStateElement.textContent = http.authKind === "token"
      ? `内联 bearer token，${(http.token || "").length} 位`
      : `当前 ${http.authKind} 模式（未内联存储）`;
    for (const element of [solHttpEnabledElement, solHttpListenElement, solHttpListenSaveElement, solHttpTokenElement, solHttpTokenGenerateElement, solHttpTokenSaveElement, solHttpTokenCopyElement]) {
      element.disabled = false;
    }
  } catch (error) {
    solHttpEnabledStateElement.textContent = `读取失败：${error}`;
  }
}

// 开关：勾选/取消立即写配置
solHttpEnabledElement.addEventListener("change", async () => {
  const flag = solHttpEnabledElement.checked;
  solHttpEnabledElement.disabled = true;
  try {
    showStatusBar(flag ? "正在开启 HTTP 服务监听…" : "正在关闭 HTTP 服务监听…");
    const message = await call("sol_http_set", { path: null, option: "enabled", value: String(flag) });
    showStatusBar(message || "已写入");
    solLogAppend(`HTTP 服务监听 → ${flag ? "开启" : "关闭"}`);
    syncEditorToDisk();
  } catch (error) {
    // call() 已展示错误；开关回弹由 refreshSolHttp 纠正
  } finally {
    solHttpEnabledElement.disabled = false;
  }
  refreshSolHttp();
});

// 监听地址：应用按钮写配置
solHttpListenSaveElement.addEventListener("click", async () => {
  const value = solHttpListenElement.value.trim();
  if (!value) {
    showStatusBar("监听地址不能为空（host:port）", { error: true });
    return;
  }
  solHttpListenSaveElement.disabled = true;
  try {
    showStatusBar("正在写入监听地址…");
    const message = await call("sol_http_set", { path: null, option: "listen", value });
    showStatusBar(message || "已写入");
    solLogAppend(`监听地址 → ${value}`);
    syncEditorToDisk();
  } catch (error) {
    // 错误已在底部提示条
  } finally {
    solHttpListenSaveElement.disabled = false;
  }
  refreshSolHttp();
});
// 输入框回车 = 应用
solHttpListenElement.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    solHttpListenSaveElement.click();
  }
});

// Token：生成 → **立即写入配置** → 面板回显同步（用户要求：生成即落盘）。
// 新值同时出现在「当前 token」展示框里，可点复制。
solHttpTokenGenerateElement.addEventListener("click", async () => {
  solHttpTokenGenerateElement.disabled = true;
  try {
    showStatusBar("正在生成并写入新 token…");
    const token = await invoke("sol_token_generate");
    const message = await call("sol_http_set", { path: null, option: "token", value: token });
    showStatusBar(message || "新 token 已写入配置");
    solLogAppend(`Token 已重新生成并写入（${token.length} 位）`);
    solHttpTokenElement.value = ""; // 新 token 已在「当前 token」框里，输入框清空
    syncEditorToDisk();
  } catch (error) {
    // 错误已在底部提示条
  } finally {
    solHttpTokenGenerateElement.disabled = false;
  }
  refreshSolHttp();
});

// 复制当前 token（优先剪贴板 API，降级 execCommand）
solHttpTokenCopyElement.addEventListener("click", async () => {
  const token = solHttpTokenCurrentElement.value;
  if (!token) {
    showStatusBar("没有可复制的 token（当前为非内联模式）", { error: true });
    return;
  }
  try {
    await navigator.clipboard.writeText(token);
    showStatusBar("token 已复制到剪贴板");
  } catch (error) {
    // 非安全上下文 / 权限拒绝：降级选中文本让用户 Ctrl+C
    solHttpTokenCurrentElement.focus();
    solHttpTokenCurrentElement.select();
    try {
      document.execCommand("copy");
      showStatusBar("token 已复制（降级方式）");
    } catch (inner) {
      showStatusBar("复制失败：已选中全文，请按 Ctrl+C", { error: true });
    }
  }
});

solHttpTokenSaveElement.addEventListener("click", async () => {
  const value = solHttpTokenElement.value.trim();
  if (!value) {
    showStatusBar("token 为空：留空 = 不改动，要换就先生成或粘贴一个", { error: true });
    return;
  }
  solHttpTokenSaveElement.disabled = true;
  try {
    showStatusBar("正在写入 token…");
    const message = await call("sol_http_set", { path: null, option: "token", value });
    showStatusBar(message || "已写入");
    solLogAppend(`Token 已更新（${value.length} 位）`);
    solHttpTokenElement.value = "";
    syncEditorToDisk();
  } catch (error) {
    // 错误已在底部提示条
  } finally {
    solHttpTokenSaveElement.disabled = false;
  }
  refreshSolHttp();
});
// token 输入框回车 = 应用
solHttpTokenElement.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    solHttpTokenSaveElement.click();
  }
});

// ---------- 安装包：下载 → 解压 → 一键安装 ----------
// 目录记忆在本页 localStorage（改了立刻落盘，跨启动沿用）。
// 双向回显：输入框（手填）↔ 选择框（文件管理器）写同一个 solDir，
// 副文字实时提示「~ 展开后的真实路径」，空 = 用后端默认（~/sol）。
function renderSolDir() {
  solDirInputElement.value = solDir;
  // 默认值从后端拿（首次渲染时懒加载，写进 placeholder）
  if (!renderSolDir._defaultLoaded && !IS_BROWSER) {
    renderSolDir._defaultLoaded = true;
    invoke("sol_default_dir")
      .then((dir) => {
        if (dir) {
          solDirInputElement.placeholder = dir;
          solDirStateElement.textContent = solDir ? `当前生效：${solDir}` : `留空 = 默认 ${dir}`;
          solDirStateElement.title = dir;
        }
      })
      .catch(() => {});
  }
  solDirStateElement.textContent = solDir
    ? `当前生效：${solDir}`
    : `留空 = 默认 ${solDirInputElement.placeholder || "~/sol"}`;
  if (solDir) {
    try {
      localStorage.setItem(SOL_DIR_KEY, solDir);
    } catch (error) {
      void error;
    }
  } else {
    try {
      localStorage.removeItem(SOL_DIR_KEY);
    } catch (error) {
      void error;
    }
  }
}

// 手动输入：双向回显的另一向（输入 → 状态字），trim 后为空视作恢复默认
solDirInputElement.addEventListener("input", () => {
  solDir = solDirInputElement.value.trim();
  renderSolDir();
});
// 失焦时若填了 ~ 开头，提示会由后端原样收到后自行展开（后端 PathBuf 不展开 ~，
// 所以在前端先把 ~ 解掉，保证提示与实际生效路径一致）
solDirInputElement.addEventListener("change", () => {
  let value = solDirInputElement.value.trim();
  if (value === "~" || value.startsWith("~/")) {
    if (IS_BROWSER) {
      showStatusBar("远程模式下 ~ 无法展开，请填完整路径", { error: true });
      return;
    }
    invoke("sol_default_dir")
      .then((dir) => {
        // dir 本身就是 <home>/sol：取它的父目录当 HOME
        const home = dir.replace(/[/\\]sol[/\\]?$/, "");
        if (home && home !== dir) {
          value = value === "~" ? home : home + value.slice(1);
          solDirInputElement.value = value;
          solDir = value;
          renderSolDir();
        }
      })
      .catch(() => {});
    return;
  }
  solDir = value;
  renderSolDir();
});

// 「打开」：把输入框里填的目录（空 = 后端默认）在文件管理器里打开
solOpenDirInputElement.addEventListener("click", async () => {
  try {
    const opened = await call("sol_open_dir", { dir: solDir || null });
    if (typeof opened === "string" && opened) {
      showStatusBar(`已打开 ${opened}`);
    }
  } catch (error) {
    // call() 已把错误写底部提示条
  }
});

// 平台目录名只做提示文案，真实文件名以后端返回的落盘路径为准
const SOL_PKG_TARGET = (() => {
  const platform = navigator.platform || "";
  if (/Win/i.test(platform)) return "windows-amd64";
  if (/Mac/i.test(platform))
    return /arm/i.test(navigator.userAgent) ? "darwin-arm64" : "darwin-amd64";
  if (/arm|aarch/i.test(navigator.userAgent)) return "linux-arm64";
  return "linux-amd64";
})();

solPickDirElement.addEventListener("click", async () => {
  try {
    const picked = await invoke("sol_pick_dir", { current: solDir || null });
    if (picked) {
      solDir = picked;
      solDirInputElement.value = picked; // 双向回显：选择框 → 输入框
      renderSolDir();
      showStatusBar(`安装目录：${picked}`);
    }
  } catch (error) {
    // 取消 / 关闭选择框：静默（不是错误）
    if (!String(error).includes("被关闭")) {
      showStatusBar(String(error), { error: true });
    }
  }
});

solDownloadElement.addEventListener("click", async () => {
  solDownloadElement.disabled = true;
  solOneClickElement.disabled = true;
  solPkgStateElement.textContent = "正在下载…";
  solProgressShow(true);
  try {
    const saved = await call("sol_install_download", { dir: solDir || null });
    if (typeof saved === "string" && saved) {
      solPkgArchivePath = saved;
      solPkgStateElement.textContent = `已下载：${solBaseName(saved) || saved}`;
      solPkgStateElement.title = saved;
      solExtractElement.disabled = false;
      solProgressDone(true);
      showStatusBar(`已下载到 ${saved}`);
    }
  } catch (error) {
    solProgressDone(false);
  } finally {
    solDownloadElement.disabled = false;
    solOneClickElement.disabled = false;
  }
});

// 手动选安装包：文件管理器挑一个 .tar.gz / .zip，选中后解压按钮直接亮
solPickArchiveElement.addEventListener("click", async () => {
  try {
    const picked = await invoke("sol_pick_archive");
    if (picked) {
      solPkgArchivePath = picked;
      solPkgStateElement.textContent = `已选择：${solBaseName(picked) || picked}`;
      solPkgStateElement.title = picked;
      solExtractElement.disabled = false;
      solLogAppend(`手动选择安装包：${picked}`);
      showStatusBar(`已选择 ${picked}`);
    }
  } catch (error) {
    if (!String(error).includes("被关闭")) {
      showStatusBar(String(error), { error: true });
    }
  }
});

// 一键下载安装：下载 → 解压 → 安装全流程，后端顺序执行并推日志
solOneClickElement.addEventListener("click", async () => {
  for (const element of [solOneClickElement, solOneClickTerminalElement, solDownloadElement, solExtractElement, solInstallFromPkgElement, solInstallFromPkgTerminalElement]) {
    element.disabled = true;
  }
  solProgressShow(true);
  try {
    showStatusBar("一键安装（静默）：① 下载 → ② 解压 → ③ 官方脚本无人值守…");
    const message = await call("sol_one_click_install", { dir: solDir || null });
    showStatusBar(message || "一键安装完成");
    solProgressDone(true);
    // 一键成功后状态全刷新；安装按钮不再置灰（任何情况都亮着）
    solPkgArchivePath = "";
  } catch (error) {
    solProgressDone(false);
  } finally {
    for (const element of [solOneClickElement, solOneClickTerminalElement, solDownloadElement, solExtractElement, solInstallFromPkgElement, solInstallFromPkgTerminalElement]) {
      element.disabled = false;
    }
  }
  refreshSolAfterAction({ forceConfig: true });
  refreshSolPackage();
});

// 一键（调终端）：同一套下载/解压，第三步呼出终端让用户确认
solOneClickTerminalElement.addEventListener("click", async () => {
  for (const element of [solOneClickElement, solOneClickTerminalElement, solDownloadElement, solExtractElement, solInstallFromPkgElement, solInstallFromPkgTerminalElement]) {
    element.disabled = true;
  }
  solProgressShow(true);
  try {
    showStatusBar("一键安装（调终端）：① 下载 → ② 解压 → ③ 呼出终端…");
    const message = await call("sol_one_click_install_terminal", { dir: solDir || null });
    showStatusBar(message || "已在终端打开安装脚本");
    solProgressDone(true);
    solPkgArchivePath = "";
  } catch (error) {
    solProgressDone(false);
  } finally {
    for (const element of [solOneClickElement, solOneClickTerminalElement, solDownloadElement, solExtractElement, solInstallFromPkgElement, solInstallFromPkgTerminalElement]) {
      element.disabled = false;
    }
  }
  refreshSolAfterAction({ forceConfig: true });
});

solExtractElement.addEventListener("click", async () => {
  if (!solPkgArchivePath) {
    return;
  }
  // 解压前会清空安装目录（保留 sol.yaml），这一步不可撤销，先让用户确认。
  const target = solDir || "默认目录（~/sol）";
  if (
    !window.confirm(
      `解压前会先清空安装目录：\n\n${target}\n\n` +
        "· 目录里除 sol.yaml 之外的内容都会被删除\n" +
        "· 这两个配置文件会保留，不会被覆盖\n" +
        "· 包内容会直接铺到安装目录，不保留 sol-v* 子目录\n\n" +
        "确认清空并解压？",
    )
  ) {
    return;
  }
  solExtractElement.disabled = true;
  solPkgStateElement.textContent = "正在解压…";
  try {
    const root = await call("sol_extract", { archive: solPkgArchivePath, dir: solDir || null });
    if (typeof root === "string" && root) {
      solPkgStateElement.textContent = `已解压：${root}`;
      solPkgStateElement.title = root;
      solPkgHintElement.dataset.root = root;
      solInstallHintElement.textContent = `解压完成：${root}。点「安装（静默）」或「安装（调终端）」继续。`;
      showStatusBar(`已解压到 ${root}`);
      // 解压成功后**重新查一次**包状态：状态文案最终由 refreshSolPackage 说了算。
      // 安装按钮不再在这里反复启用/禁用 —— 任何情况都亮着（用户定案）。
      solExtractElement.disabled = false;
      await refreshSolPackage();
    }
  } finally {
    solExtractElement.disabled = false;
  }
});

// 已解压的包直接安装（安装源 = 解压根目录 = 安装目录本身）
async function installFromPkg(viaTerminal) {
  const source = solPkgHintElement.dataset.root || solDir || null;
  // 运行期间防重复点击（这是唯一的禁用场景），跑完立即恢复
  solInstallFromPkgElement.disabled = true;
  solInstallFromPkgTerminalElement.disabled = true;
  try {
    const message = viaTerminal
      ? await call("sol_install_terminal", { installDir: source })
      : await call("sol_install_silent", { installDir: source });
    showStatusBar(message || "已安装");
  } finally {
    solInstallFromPkgElement.disabled = false;
    solInstallFromPkgTerminalElement.disabled = false;
  }
  refreshSolAfterAction({ forceConfig: true });
}
solInstallFromPkgElement.addEventListener("click", () => installFromPkg(false));
solInstallFromPkgTerminalElement.addEventListener("click", () => installFromPkg(true));

// 浏览器端顶部横幅：预览模式说明 / 远程已连接 / token 输入（三态）。
// 仅浏览器模式创建 —— Tauri 主窗口不需要这条横幅。
const browserBanner = IS_BROWSER
  ? (() => {
      const element = document.createElement("div");
      element.className = "browser-banner";
      document.body.prepend(element);
      return element;
    })()
  : null;

/// 实际在用的通道给人看的说法（横幅上那一句）
function transportLabel() {
  if (activeTransport === "ws") {
    return "推送（WebSocket）";
  }
  if (activeTransport === "http") {
    return "轮询（HTTP 每 2s）";
  }
  return transportMode === TRANSPORT_POLL ? "轮询未启动" : "正在连接推送…";
}

/// 横幅上那个「传输方式」下拉：选项就是三档，当前档位显示实际在用的通道。
function buildTransportSelect() {
  const select = document.createElement("select");
  select.title = "改用推送（WebSocket）或轮询（HTTP）取快照";
  for (const [value, label] of [
    [TRANSPORT_AUTO, `自动（现在：${transportLabel()}）`],
    [TRANSPORT_PUSH, "推送（WebSocket）"],
    [TRANSPORT_POLL, "轮询（HTTP）"],
  ]) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    select.append(option);
  }
  select.value = transportMode;
  select.addEventListener("change", () => setTransportMode(select.value));
  return select;
}

function renderBrowserBanner() {
  if (!IS_BROWSER) {
    return;
  }
  browserBanner.replaceChildren();
  if (!remoteApiAvailable) {
    browserBanner.textContent =
      "浏览器预览模式：仅展示界面布局，操作请在 AnyBoot 应用内进行";
    return;
  }
  if (remoteToken) {
    browserBanner.textContent =
      "远程控制已连接：这里的设置改动实时作用于本机 AnyBoot";
    // 手动刷新：推送正常时用不上，但它是兜底（某一拍丢帧 / 另一台设备刚改过）
    const refresh = document.createElement("button");
    refresh.type = "button";
    refresh.className = "banner-link";
    refresh.textContent = "刷新";
    refresh.addEventListener("click", () => refreshNow());
    // 传输方式：推送优先还是轮询
    browserBanner.append("　", refresh, "　", buildTransportSelect());
    // 给个退路：换 token / 断开重连
    const change = document.createElement("button");
    change.type = "button";
    change.className = "banner-link";
    change.textContent = "更换 token";
    change.addEventListener("click", () => showTokenModal());
    browserBanner.append("　", change);
    return;
  }
  browserBanner.textContent = "远程控制未连接：请输入访问令牌";
  // 输入框可能被用户「暂不连接」关掉了，横幅里常驻一个入口
  const enter = document.createElement("button");
  enter.type = "button";
  enter.className = "banner-link";
  enter.textContent = "输入 token";
  enter.addEventListener("click", () => showTokenModal());
  browserBanner.append("　", enter);
}

// ---------- 下拉刷新（手机 / 触屏） ----------

/// 顶部那个小药丸（下拉时跟着手指走，或显示「正在刷新…」）
let pullIndicator = null;

function pullIndicatorElement() {
  if (!pullIndicator) {
    pullIndicator = document.createElement("div");
    pullIndicator.className = "pull-refresh";
    pullIndicator.hidden = true;
    document.body.append(pullIndicator);
  }
  return pullIndicator;
}

function setPullIndicator(text, offset = 0, visible = true) {
  if (!IS_BROWSER) {
    return;
  }
  const element = pullIndicatorElement();
  element.textContent = text;
  element.style.transform = `translate(-50%, ${offset}px)`;
  element.hidden = !visible;
}

/// 装下拉刷新。只在**真的处在顶部**时才接管手势 —— 否则往下拖只是正常往回滚。
///
/// 另外刻意用非 passive 的 touchmove（要 preventDefault）：不拦的话浏览器会把这串
/// 手势当成它自己的「整页回弹刷新」，两套刷新叠在一起，用户看到的是页面自己重载了。
function installPullToRefresh() {
  if (!IS_BROWSER) {
    return;
  }
  const THRESHOLD = 64; // 触发阈值
  const MAX = 96; // 指示器最多往下走多少
  let startY = null;
  let distance = 0;

  /// 手指落点这一路是否都在顶部（页面本身 + 沿途每一个滚动容器）
  const atTop = (node) => {
    const scroller = document.scrollingElement || document.documentElement;
    if ((scroller && scroller.scrollTop > 0) || window.scrollY > 0) {
      return false;
    }
    for (let element = node; element && element !== document.body; element = element.parentElement) {
      if (element.scrollTop > 0) {
        return false;
      }
    }
    return true;
  };

  const reset = () => {
    startY = null;
    distance = 0;
    // 正在刷新时保留「正在刷新…」（由 refreshNow 收尾）
    if (!refreshing) {
      setPullIndicator("", 0, false);
    }
  };

  document.addEventListener(
    "touchstart",
    (event) => {
      if (event.touches.length !== 1 || refreshing) {
        startY = null;
        return;
      }
      const target = event.target;
      // 弹窗上不接手势（token 框、服务端列表…），不然在里面滚动会误触发
      if (target instanceof Element && target.closest(".token-modal, .token-modal-overlay")) {
        startY = null;
        return;
      }
      if (!atTop(target)) {
        startY = null;
        return;
      }
      startY = event.touches[0].clientY;
      distance = 0;
    },
    { passive: true },
  );

  document.addEventListener(
    "touchmove",
    (event) => {
      if (startY === null) {
        return;
      }
      const delta = event.touches[0].clientY - startY;
      if (delta <= 0) {
        reset();
        return;
      }
      // 阻尼：手指走 2px 指示器走 1px，越拉越沉
      distance = Math.min(delta / 2, MAX);
      if (event.cancelable) {
        event.preventDefault();
      }
      setPullIndicator(distance >= THRESHOLD ? "松开刷新" : "下拉刷新", distance);
    },
    { passive: false },
  );

  document.addEventListener(
    "touchend",
    () => {
      if (startY === null) {
        return;
      }
      const trigger = distance >= THRESHOLD;
      reset();
      if (trigger) {
        refreshNow();
      }
    },
    { passive: true },
  );
  document.addEventListener("touchcancel", reset, { passive: true });
}

// ---------- token 模态框（首次访问 / 401 时弹出） ----------

let tokenModal = null;
// 用户点过「暂不连接，先看看界面」后置位：不用再每 2s 把框弹回来打扰他
let tokenModalDismissed = false;

function buildTokenModal() {
  const overlay = document.createElement("div");
  overlay.className = "token-modal-overlay";
  overlay.hidden = true;

  const box = document.createElement("form");
  box.className = "token-modal";

  const title = document.createElement("h2");
  title.textContent = "AnyBoot 远程控制";

  const desc = document.createElement("p");
  desc.textContent =
    "首次访问需要输入访问令牌（token）。token 在 AnyBoot 应用 → 基本设置 → 远程控制 中查看；也可以直接用带 ?token=xxx 的链接访问。";

  const input = document.createElement("input");
  input.type = "password";
  input.placeholder = "访问令牌 token";
  input.autocomplete = "off";
  input.name = "token";
  // 手机软键盘默认首字母大写 / 联想纠错，会把 token 悄悄改掉，这里一律关掉
  input.setAttribute("autocapitalize", "off");
  input.setAttribute("autocorrect", "off");
  input.spellcheck = false;

  const reveal = document.createElement("label");
  reveal.className = "token-reveal";
  const revealBox = document.createElement("input");
  revealBox.type = "checkbox";
  reveal.append(revealBox, document.createTextNode("显示 token（确认没被输入法改掉）"));
  revealBox.addEventListener("change", () => {
    input.type = revealBox.checked ? "text" : "password";
  });

  const error = document.createElement("p");
  error.className = "token-error";
  error.hidden = true;

  const actions = document.createElement("div");
  actions.className = "token-actions";
  const connect = document.createElement("button");
  connect.type = "submit";
  connect.textContent = "连接";
  const skip = document.createElement("button");
  skip.type = "button";
  skip.className = "secondary";
  skip.textContent = "暂不连接，先看看界面";
  actions.append(connect, skip);

  box.append(title, desc, input, reveal, error, actions);
  overlay.append(box);
  document.body.append(overlay);

  const fail = (message) => {
    error.textContent = message;
    error.hidden = false;
  };

  box.addEventListener("submit", async (event) => {
    event.preventDefault();
    const value = input.value.trim();
    if (!value) {
      fail("token 不能为空");
      return;
    }
    connect.disabled = true;
    try {
      // 先验再存：错误的 token 不落 cookie，免得下次打开又是一轮 401
      const response = await fetch("/api/ui-state", {
        headers: { "X-Auth-Token": value },
        cache: "no-store",
      });
      if (response.status === 401) {
        fail("token 不正确或已失效，请重新输入");
        return;
      }
      if (!response.ok) {
        fail(`连接失败（HTTP ${response.status}），请稍后再试`);
        return;
      }
      remoteToken = value;
      try {
        writeCookie(REMOTE_TOKEN_COOKIE, remoteToken);
      } catch {
        // 存不了就本次会话先用着
      }
      overlay.hidden = true;
      renderBrowserBanner();
      // 新 token 到手：按当前传输方式把通道接起来（启动时没 token，那会儿什么都没起）
      restartTransport();
      // 初始快照立刻补上，不等下一拍
      applySnapshot(await response.json(), { force: true });
    } catch (connectError) {
      fail(`连接失败：${connectError}`);
    } finally {
      connect.disabled = false;
    }
  });

  skip.addEventListener("click", () => {
    overlay.hidden = true;
    // 用户明确选了「先看看界面」：之后不再自动弹回来，横幅里留了入口
    tokenModalDismissed = true;
  });

  return { overlay, input, error };
}

/// 弹出 token 输入框（首次访问 / 收到 401 时）。**已经在输入状态时只更新文案**：
/// 轮询每 2s 一次的 401 会重复调到这里，若照旧写 `input.value = remoteToken`
/// （那时已被清成空串）就会把用户正在打字的 token 抹掉 —— 表现为「打一半就没了、
/// 像页面被刷新」。输入框的内容只归用户本人管，这里除首次打开外不许再碰。
function showTokenModal(message = "") {
  if (!IS_BROWSER) {
    return;
  }
  if (!tokenModal) {
    tokenModal = buildTokenModal();
  }
  tokenModal.error.hidden = !message;
  tokenModal.error.textContent = message;
  if (!tokenModal.overlay.hidden) {
    // 已经开着（用户正在输入）：不动输入框、不抢焦点
    return;
  }
  tokenModalDismissed = false;
  tokenModal.overlay.hidden = false;
  tokenModal.input.value = remoteToken || "";
  tokenModal.input.focus();
}

/// 命令返回 401 时清掉旧 token（cookie 一并失效）、重新弹输入框
function requestRemoteToken() {
  if (!IS_BROWSER) {
    return;
  }
  remoteToken = "";
  try {
    document.cookie = `${REMOTE_TOKEN_COOKIE}=; path=/; max-age=0; SameSite=Strict`;
  } catch {
    void 0;
  }
  // 通道也要停：推送还挂着旧 token，轮询再跑下去只会继续 401 刷屏。
  // 用户把新 token 填好后 restartTransport() 会把通道接回来。
  stopPush();
  stopUiStatePolling();
  setActiveTransport("none");
  renderBrowserBanner();
  // 输入框已经开着：只把提示换成「token 不对」，绝不重开/清空
  if (tokenModal && !tokenModal.overlay.hidden) {
    tokenModal.error.hidden = false;
    tokenModal.error.textContent = "token 不正确或已失效，请重新输入";
    return;
  }
  // 用户选了「暂不连接」：不再自动弹回来（横幅里有「输入 token」入口）
  if (tokenModalDismissed) {
    return;
  }
  showTokenModal(
    remoteApiAvailable ? "token 不正确或已失效，请重新输入" : "",
  );
}

/// 切换 GRUB 启动项。与其它操作不同，这里不走 call()：切换的中间态
/// 「正在写入 grubenv…」以及失败原因要显示在**右栏**（用户点的是右栏的条目，
/// 视线不该被迫跳到窗口最底部去找回执）。最终结果仍由后端 record_action +
/// publish_ui 带回来，落在窗口底部那条提示条上（失败原因两处都会显示）。
async function switchGrub(selector) {
  if (!selector) {
    return;
  }
  grubStatusElement.textContent = "正在写入 grubenv…";
  grubStatusElement.classList.remove("fail");
  try {
    await invoke("switch_grub", { selector });
    // 后端写完 grubenv 会 publish 新快照（选中高亮跟着变），中间态到这里清掉
    grubStatusElement.textContent = "";
  } catch (error) {
    grubStatusElement.textContent = String(error);
    grubStatusElement.classList.add("fail");
  }
}

useDirectoryElement.addEventListener("click", () => {
  const path = directoryElement.value.trim();
  call("set_vars_dir", { path: path || null });
});

unmountTemporaryElement.addEventListener("click", () =>
  call("unmount_temporary"),
);

for (const [button, action] of powerButtons) {
  button.addEventListener("click", () => call("power_action", { action }));
}

// ---------- 程序自身的两个动作（顶部标题栏） ----------
// 「重启程序」：后端先停 HTTP 监听（放开端口）再拉起新实例，本进程随即退出 ——
// 所以这条 invoke 正常永远不返回，走不到下面的 catch。
restartAppElement.addEventListener("click", async () => {
  showStatusBar("正在重启程序…");
  try {
    await invoke("restart_app");
  } catch (error) {
    showStatusBar(String(error), { error: true });
  }
});

// 「退出程序」：与托盘菜单里的「退出程序」同一条路（app.exit(0)）。
quitAppElement.addEventListener("click", async () => {
  showStatusBar("正在退出程序…");
  try {
    await invoke("quit_app");
  } catch (error) {
    showStatusBar(String(error), { error: true });
  }
});

// 网页端（远程控制页 / 手机客户端）：这两个动作操作的是**电脑上**的应用，
// 后端也明确不放行（见 ui_command 的本机限定名单）—— 藏起来，别留一个点了
// 必然报错的死按钮。
if (IS_BROWSER) {
  restartAppElement.classList.add("hidden");
  quitAppElement.classList.add("hidden");
}

// ---------- 远程管理（SSH/SFTP 独立窗）入口 ----------
// 只在本机窗口有意义：开的是电脑上的 webview 窗口，网页端点了没有落点。
const openHubElement = document.querySelector("#open-hub");
if (openHubElement) {
  if (IS_BROWSER) {
    openHubElement.classList.add("hidden");
  } else {
    openHubElement.addEventListener("click", async () => {
      try {
        await invoke("hub_open_window");
      } catch (error) {
        showStatusBar(String(error), { error: true });
      }
    });
  }
}

// ---------- GRUB 细粒度切换区 ----------

/// 这一区的操作回执（识别 / 挂载 / 打开目录）：统一落到窗口最下方那条独立提示条
/// —— 提示框属于窗口，不属于正文区块，所以不再往这一区内部写一份。
function grubMessage(text, { error = false } = {}) {
  const message = (text || "").trim();
  if (message) {
    showStatusBar(message, { error });
  }
}

// 展示方式切换（内嵌 / 加宽 / 列表独立 / GRUB 独立）：后端负责保存设置并
// 调整窗口宽度（460 ⇄ 932），前端只管把选择报上去；布局本身由下一帧快照
// 的 renderGrub → applyGrubLayout 落地。
grubViewModeElement.addEventListener("change", () => {
  call("set_grub_list_view", { mode: grubViewModeElement.value });
});

// 列表格式（原始/极简/简易/中等/复杂）：纯展示偏好，后端落盘后广播回来
grubListFormatElement.addEventListener("change", () => {
  call("set_grub_list_format", { format: grubListFormatElement.value });
});

// 识别 ESP 目录：后端判断是不是 FAT32，并把分区 UUID（识别键）填好
grubDirDetectElement.addEventListener("click", async () => {
  const path = grubDirElement.value.trim();
  if (!path) {
    grubMessage("先填 ESP 分区里的任意路径，例如 H:\\ 或 /boot/efi", {
      error: true,
    });
    return;
  }
  grubMessage("");
  grubDirDetectElement.disabled = true;
  try {
    grubMessage(await invoke("detect_esp", { path }));
  } catch (error) {
    grubMessage(String(error), { error: true });
  } finally {
    grubDirDetectElement.disabled = false;
  }
  await refreshSnapshotQuietly();
});

// 呼出系统文件夹选择框选 ESP 目录，选中后自动走识别：
// 识别成功会同时填好分区 UUID，「选文件夹」和「填分区 UUID」两条入口自动同步
//（分区 UUID 是权威键，目录只是入口）。空状态卡上的按钮走同一条路。
async function pickGrubDirAndDetect() {
  grubMessage("");
  try {
    const path = await invoke("pick_grub_dir");
    if (!path) {
      grubMessage("已取消选择");
      return;
    }
    grubDirElement.value = path;
    grubMessage(await invoke("detect_esp", { path }));
  } catch (error) {
    grubMessage(String(error), { error: true });
  }
  await refreshSnapshotQuietly();
}
grubDirPickElement.addEventListener("click", pickGrubDirAndDetect);
grubEmptyPickElement.addEventListener("click", pickGrubDirAndDetect);

// 三端都用系统文件管理器打开解析出来的 ESP 目录
grubDirOpenElement.addEventListener("click", async () => {
  grubMessage("");
  try {
    grubMessage(`已用文件管理器打开：${await invoke("open_grub_dir")}`);
  } catch (error) {
    grubMessage(String(error), { error: true });
  }
});

// 勾选 = 按分区 UUID 挂载并保持；取消 = 卸掉**本程序自己挂的**那份
grubMountElement.addEventListener("change", async () => {
  const wanted = grubMountElement.checked;
  grubMessage("");
  grubMountElement.disabled = true;
  try {
    await invoke("set_grub_mount", { enabled: wanted });
    grubMessage(wanted ? "ESP 已挂载，可以直接读写" : "已卸载本程序挂上的 ESP");
  } catch (error) {
    grubMessage(String(error), { error: true });
  } finally {
    grubMountElement.disabled = false;
  }
  await refreshSnapshotQuietly();
});
// 目录「保存」：只落盘目录原文，不做校验、不取分区 UUID —— 清空后保存即还原成空目录
grubDirSaveElement.addEventListener("click", () =>
  call("set_grub_dir", { dir: grubDirElement.value.trim() }),
);

grubPartuuidSaveElement.addEventListener("click", () =>
  call("set_grub_partuuid", { partuuid: grubPartuuidElement.value.trim() }),
);

// UUID「识别」：没挂载先按需挂载，分区上有 grub.cfg 就解析，
// 挂载点回填进「ESP 目录」并保存（与目录识别双向同步）
grubPartuuidDetectElement.addEventListener("click", () =>
  call("detect_grub_by_partuuid"),
);

// 「同步」：把当前系统 /boot/grub（或 grub2）的 grub.cfg 覆盖搬进 ESP；
// 硬性条件 = 分区已挂载到 /boot 或 /boot/efi，否则后端直接拒绝、什么都不动
grubSyncElement.addEventListener("click", () => call("sync_grub"));

grubInstallElement.addEventListener("click", () => call("install_grub_script"));

grubUninstallElement.addEventListener("click", () =>
  call("uninstall_grub_script"),
);

grubScriptSaveElement.addEventListener("click", () =>
  call("set_grub_script", { script: grubScriptElement.value }),
);

// 清空 = 恢复内置模板，后端把空串当成「用模板」处理
grubScriptResetElement.addEventListener("click", () =>
  call("set_grub_script", { script: "" }),
);

// ---------- 远程控制区 ----------

/// 远程控制区的回执：统一落到窗口最下方那条独立提示条上
/// （提示框一律在主内容区之外，不再往区块内部塞）。
function remoteMessage(text, { error = false } = {}) {
  const message = (text || "").trim();
  if (message) {
    showStatusBar(message, { error });
  }
}

async function copyText(text, okMessage) {
  try {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(text);
    } else {
      const area = document.createElement("textarea");
      area.value = text;
      document.body.append(area);
      area.select();
      document.execCommand("copy");
      area.remove();
    }
    remoteMessage(okMessage);
  } catch (error) {
    remoteMessage(`复制失败：${error}`, { error: true });
  }
}

function applyDelayFrom(channel, element) {
  const seconds = Number.parseInt(element.value.trim(), 10);
  if (!Number.isInteger(seconds) || seconds < 0 || seconds > 600) {
    remoteMessage("延迟必须是 0-600 之间的整数", { error: true });
    return;
  }
  call("set_channel_delay", { channel, seconds });
}

httpEnabledElement.addEventListener("change", () => {
  applySettingLinkage();
  call("set_http_enabled", { enabled: httpEnabledElement.checked });
});

httpApplyElement.addEventListener("click", () => {
  const bind = httpBindElement.value.trim();
  const port = Number.parseInt(httpPortElement.value.trim(), 10);
  if (!bind) {
    remoteMessage("监听地址不能为空（0.0.0.0 或 127.0.0.1）", { error: true });
    return;
  }
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    remoteMessage("端口必须是 1-65535 的整数", { error: true });
    return;
  }
  call("set_http_bind", { bind }).then(() => call("set_http_port", { port }));
});

notifyEnabledElement.addEventListener("change", () => {
  applySettingLinkage();
  call("set_notify_enabled", { enabled: notifyEnabledElement.checked });
});

httpChannelNotifyElement.addEventListener("change", () =>
  call("set_channel_notify", {
    channel: "http",
    notify: httpChannelNotifyElement.checked,
  }),
);

cliChannelNotifyElement.addEventListener("change", () =>
  call("set_channel_notify", {
    channel: "cli",
    notify: cliChannelNotifyElement.checked,
  }),
);

httpChannelDelayElement.addEventListener("change", () =>
  applyDelayFrom("http", httpChannelDelayElement),
);

// 提示音开关与路径：勾选只改本地待提交状态（点「应用」才写后端），灰化跟勾选即时走
playSoundElement.addEventListener("change", applySettingLinkage);

soundApplyElement.addEventListener("click", () => {
  const enabled = playSoundElement.checked;
  const path = soundFilePathElement.value.trim();
  call("set_play_sound", { enabled });
  call("set_sound_file_path", { path: path || "" });
});

// 重试监听（端口刚被释放、或用户手动清掉了占用者）
retryHttpElement.addEventListener("click", async () => {
  remoteMessage("");
  try {
    remoteMessage(`重试成功：${await invoke("retry_http")}`);
  } catch (error) {
    remoteMessage(`重试失败：${error}`, { error: true });
  }
  try {
    applySnapshot(await invoke("get_ui_state"), { force: true });
  } catch (error) {
    void error;
  }
});

// 结束占用进程：显式动作，按钮上已写明 PID
killOccupantElement.addEventListener("click", async () => {
  remoteMessage("");
  try {
    remoteMessage(await invoke("kill_port_occupant"));
  } catch (error) {
    remoteMessage(`结束失败：${error}`, { error: true });
  }
  try {
    applySnapshot(await invoke("get_ui_state"), { force: true });
  } catch (error) {
    void error;
  }
});

// 打开配置目录
openConfigDirElement.addEventListener("click", async () => {
  try {
    const dirPath = await invoke("open_config_dir");
    remoteMessage(`已用文件管理器打开：${dirPath}`);
  } catch (error) {
    remoteMessage(`打开配置目录失败：${error}`, { error: true });
  }
});

// 打开变量目录
openVarsDirElement.addEventListener("click", async () => {
  try {
    const dirPath = await invoke("open_vars_dir");
    remoteMessage(`已用文件管理器打开：${dirPath}`);
  } catch (error) {
    remoteMessage(`打开变量目录失败：${error}`, { error: true });
  }
});

cliChannelDelayElement.addEventListener("change", () =>
  applyDelayFrom("cli", cliChannelDelayElement),
);

remoteAllowGuiAuthElement.addEventListener("change", () =>
  call("set_remote_allow_gui_auth", {
    enabled: remoteAllowGuiAuthElement.checked,
  }),
);

tokenRevealElement.addEventListener("click", async () => {
  try {
    const token = await invoke("reveal_http_token");
    tokenHintElement.textContent = `当前令牌：${token}`;
    remoteMessage("已显示明文令牌（注意旁人视线）");
  } catch (error) {
    remoteMessage(String(error), { error: true });
  }
});

tokenCopyElement.addEventListener("click", async () => {
  try {
    await copyText(await invoke("reveal_http_token"), "令牌已复制到剪贴板");
  } catch (error) {
    remoteMessage(String(error), { error: true });
  }
});

tokenRegenerateElement.addEventListener("click", async () => {
  try {
    const token = await invoke("regenerate_http_token");
    tokenHintElement.textContent = `新令牌：${token}`;
    // 不写「已重启」这种漂亮话：重启成没成、绑在哪个地址上由后端如实报
    remoteMessage("已重新生成令牌，旧 token 立即失效");
  } catch (error) {
    remoteMessage(String(error), { error: true });
  }
  await refreshSnapshotQuietly();
});

// 「打开 Web 页面」：让后端用系统默认浏览器打开带 token 的 Web 地址，
// 省得用户自己去翻 IP 和令牌。网页端自己就是 Web 页面，这一行直接藏掉。
if (openWebElement) {
  if (IS_BROWSER) {
    openWebElement.closest(".setting")?.classList.add("hidden");
  } else {
    openWebElement.addEventListener("click", async () => {
      openWebElement.disabled = true;
      try {
        const url = await invoke("open_web_page");
        remoteMessage(`已在浏览器中打开：${url}`);
      } catch (error) {
        remoteMessage(String(error), { error: true });
      } finally {
        applySettingLinkage();
      }
    });
  }
}

// 自定义 token：保存后后端会**关掉旧监听、按新 token 重新绑定端口**，旧 token 立即失效。
tokenSaveElement.addEventListener("click", async () => {
  const token = tokenInputElement.value.trim();
  tokenSaveElement.disabled = true;
  try {
    await invoke("set_http_token", { token });
    tokenInputElement.value = "";
    // 同样不写死「已重启」：真实结果由后端写进状态文案
    remoteMessage(
      token
        ? "令牌已更新，正在按新 token 重启监听"
        : "令牌已清空：token 为空时内置 HTTP 服务不会启动",
    );
  } catch (error) {
    remoteMessage(`令牌保存失败：${error}`, { error: true });
  } finally {
    tokenSaveElement.disabled = false;
  }
  await refreshSnapshotQuietly();
});

httpHintCopyElement.addEventListener("click", async () => {
  try {
    const text = await invoke("http_access_hint");
    httpHintElement.textContent = text;
    await copyText(text, "命令已复制到剪贴板");
  } catch (error) {
    remoteMessage(String(error), { error: true });
  }
});

pendingRunElement.addEventListener("click", () => call("confirm_pending"));
pendingCancelElement.addEventListener("click", () => call("cancel_pending"));

autostartElement.addEventListener("change", () =>
  call("set_autostart", { enabled: autostartElement.checked }),
);

startInTrayElement.addEventListener("change", () =>
  call("set_start_in_tray", { enabled: startInTrayElement.checked }),
);

// 窗口始终置顶：勾选立即钉到最上层，取消立即放回普通层级
mainAlwaysOnTopElement.addEventListener("change", () =>
  call("set_main_always_on_top", { enabled: mainAlwaysOnTopElement.checked }),
);

// 贴边自动隐藏：开后鼠标一离窗就缩成边条，悬停边条展开；关掉立即展开
mainAutoHideElement.addEventListener("change", () => {
  applySettingLinkage();
  call("set_main_auto_hide", { enabled: mainAutoHideElement.checked });
});

// 贴边触发距离：主窗口离屏幕边框多近时，鼠标离开才会隐藏
mainAutoHideEdgeDistanceElement.addEventListener("change", () => {
  const parsed = Number.parseInt(mainAutoHideEdgeDistanceElement.value, 10);
  const distance = Number.isFinite(parsed) ? Math.min(Math.max(parsed, 0), 200) : 24;
  mainAutoHideEdgeDistanceElement.value = distance;
  call("set_main_auto_hide_edge_distance", { distance });
});

// 悬浮呼出延时：压住白条这么久才展开主窗（毫秒，0 = 当拍立即）
mainDockStripHoverMsElement.addEventListener("change", () => {
  const parsed = Number.parseInt(mainDockStripHoverMsElement.value, 10);
  const value = Number.isFinite(parsed) ? Math.min(Math.max(parsed, 0), 2000) : 0;
  mainDockStripHoverMsElement.value = value;
  call("set_main_dock_strip_hover_ms", { value });
});

// 白条距屏幕边界：停靠落点沿法线方向离边这条缝（0 = 紧贴）
mainDockStripEdgeGapElement.addEventListener("change", () => {
  const parsed = Number.parseInt(mainDockStripEdgeGapElement.value, 10);
  const value = Number.isFinite(parsed) ? Math.min(Math.max(parsed, 0), 100) : 0;
  mainDockStripEdgeGapElement.value = value;
  call("set_main_dock_strip_edge_gap", { value });
});

// 贴边白条厚度：白条独立渲染的粗细，纯前端 CSS 实时（input），
// 松手提交后端存值（窗口几何与厚度无关，无需重贴）
mainDockStripThicknessElement.addEventListener("input", () => {
  const value = Number.parseInt(mainDockStripThicknessElement.value, 10);
  if (mainDockStripThicknessStateElement) {
    mainDockStripThicknessStateElement.textContent = `${value}px（拖动即时预览）`;
  }
  document.documentElement.style.setProperty("--dock-strip-thickness", `${value}px`);
});
mainDockStripThicknessElement.addEventListener("change", () => {
  const parsed = Number.parseInt(mainDockStripThicknessElement.value, 10);
  const value = Number.isFinite(parsed) ? Math.min(Math.max(parsed, 2), 40) : 6;
  mainDockStripThicknessElement.value = value;
  call("set_main_dock_strip_thickness", { value });
});

// 贴边白条长度：占主窗那条窗口边的百分比，纯前端 CSS 实时（input），
// 松手提交后端存值（不重贴窗口，窗口尺寸与长度无关）
mainDockStripLengthElement.addEventListener("input", () => {
  const value = Number.parseInt(mainDockStripLengthElement.value, 10);
  if (mainDockStripLengthStateElement) {
    mainDockStripLengthStateElement.textContent = `${value}%（窗口边百分比）`;
  }
  document.documentElement.style.setProperty("--dock-strip-length", `${value}%`);
});
mainDockStripLengthElement.addEventListener("change", () => {
  const parsed = Number.parseInt(mainDockStripLengthElement.value, 10);
  const value = Number.isFinite(parsed) ? Math.min(Math.max(parsed, 10), 100) : 100;
  mainDockStripLengthElement.value = value;
  call("set_main_dock_strip_length", { value });
});

// 贴边白条不透明度：滑条即时预览（input）+ 松手提交（change）
mainDockStripOpacityElement.addEventListener("input", () => {
  const value = Number.parseInt(mainDockStripOpacityElement.value, 10);
  if (mainDockStripOpacityStateElement) {
    mainDockStripOpacityStateElement.textContent = `${value}%（拖动即时预览）`;
  }
  document.documentElement.style.setProperty(
    "--dock-strip-opacity",
    String(value / 100),
  );
});
mainDockStripOpacityElement.addEventListener("change", () => {
  const parsed = Number.parseInt(mainDockStripOpacityElement.value, 10);
  const value = Number.isFinite(parsed) ? Math.min(Math.max(parsed, 10), 100) : 100;
  mainDockStripOpacityElement.value = value;
  call("set_main_dock_strip_opacity", { value });
});

// 贴边白条颜色：调色板即时预览（input），关闭选色器（change）才落盘
mainDockStripColorElement.addEventListener("input", () => {
  const color = shadeHex(mainDockStripColorElement.value);
  if (color) {
    document.documentElement.style.setProperty("--dock-strip-color", color);
  }
});
mainDockStripColorElement.addEventListener("change", () => {
  call("set_main_dock_strip_color", { color: mainDockStripColorElement.value });
});

// 热区触发厚度：input 即时预览（可视化层立刻跟着变），change 落盘并重贴
// 窗口（后端按 max(槽位, 热区) 重算缝厚，渲染与判定保持同一个矩形）
mainHotzoneThicknessElement.addEventListener("input", () => {
  const value = Number.parseInt(mainHotzoneThicknessElement.value, 10);
  if (mainHotzoneThicknessStateElement) {
    mainHotzoneThicknessStateElement.textContent = `${value}px（鼠标移入屏幕边往内这一段就呼出主窗）`;
  }
  const gap = Math.round(Number(mainDockStripEdgeGapElement.value)) || 0;
  document.documentElement.style.setProperty(
    "--hotzone-thickness",
    `${Math.max(value - gap, 0)}px`,
  );
});
mainHotzoneThicknessElement.addEventListener("change", () => {
  const parsed = Number.parseInt(mainHotzoneThicknessElement.value, 10);
  const value = Number.isFinite(parsed) ? Math.min(Math.max(parsed, 1), 100) : 40;
  mainHotzoneThicknessElement.value = value;
  call("set_main_hotzone_thickness", { value });
});

// 热区调试底色 + 透明度：即时预览，change 落盘
mainHotzoneColorElement.addEventListener("change", () => {
  call("set_main_hotzone_color", { color: mainHotzoneColorElement.value });
});
mainHotzoneOpacityElement.addEventListener("input", () => {
  const value = Number.parseInt(mainHotzoneOpacityElement.value, 10);
  if (mainHotzoneOpacityStateElement) {
    mainHotzoneOpacityStateElement.textContent = `${value}%（0 = 全透明看不见）`;
  }
  document.documentElement.style.setProperty("--hotzone-opacity", String(value / 100));
});
mainHotzoneOpacityElement.addEventListener("change", () => {
  const parsed = Number.parseInt(mainHotzoneOpacityElement.value, 10);
  const value = Number.isFinite(parsed) ? Math.min(Math.max(parsed, 0), 100) : 0;
  mainHotzoneOpacityElement.value = value;
  call("set_main_hotzone_opacity", { value });
});

// 热区边框开关 + 颜色
mainHotzoneBorderElement.addEventListener("change", () => {
  call("set_main_hotzone_border", { enabled: mainHotzoneBorderElement.checked });
});
mainHotzoneBorderColorElement.addEventListener("change", () => {
  call("set_main_hotzone_border_color", { color: mainHotzoneBorderColorElement.value });
});

// 主题强调色：调色板即时预览（主窗 + GRUB 同步），change 落盘
accentColorElement.addEventListener("input", () => {
  currentAccent = shadeHex(accentColorElement.value) || currentAccent;
  applyAccent(currentAccent, themeElement.value);
  emitEvent("accent-preview", currentAccent);
});
accentColorElement.addEventListener("change", () => {
  call("set_accent_color", { color: accentColorElement.value });
});

// 固定显示位置：free / 屏幕中间 / 上次位置
// （切到「屏幕中间」会立刻停用贴边自动隐藏整组 —— 两者互斥，见 applySettingLinkage）
mainWindowPositionElement.addEventListener("change", () => {
  applySettingLinkage();
  call("set_main_window_position", { mode: mainWindowPositionElement.value });
});

// 托盘菜单方式：各自方式（默认）/ 全局统一（positioner TrayCenter）
trayMenuStyleElement.addEventListener("change", () =>
  call("set_tray_menu_style", { mode: trayMenuStyleElement.value }),
);

// 固定显示器：auto + 每台显示器一项（与托盘「固定显示器」子菜单同源）
mainFixedMonitorElement.addEventListener("change", () =>
  call("set_main_fixed_monitor", { name: mainFixedMonitorElement.value }),
);

// 固定显示器选项随硬件热插拔变化：选项集没变就不重建（保住焦点/滚动），
// 选中值失效（那台显示器拔了）回退 auto，与后端回落逻辑一致。
function syncFixedMonitorOptions(options, current) {
  const names = Array.isArray(options)
    ? options.filter((name) => typeof name === "string" && name)
    : [];
  const signature = names.join("\u0000");
  if (mainFixedMonitorElement.dataset.options !== signature) {
    mainFixedMonitorElement.dataset.options = signature;
    while (mainFixedMonitorElement.options.length > 1) {
      mainFixedMonitorElement.remove(1);
    }
    for (const name of names) {
      const option = document.createElement("option");
      option.value = name;
      option.textContent = name;
      mainFixedMonitorElement.append(option);
    }
  }
  const fixed = typeof current === "string" && current ? current : "auto";
  mainFixedMonitorElement.value = names.includes(fixed) ? fixed : "auto";
}

// 重置位置：立即把主窗口放到屏幕正中央
resetMainWindowPositionElement.addEventListener("click", () =>
  call("reset_main_window_position", {}),
);

// 重置窗口大小：宽回标称、高回整屏 2/3（与托盘「重置窗口大小」同一条链路）
const resetMainWindowSizeElement = document.querySelector("#reset-main-window-size");
resetMainWindowSizeElement.addEventListener("click", () =>
  call("reset_window_size", {}),
);

// ============ 窗口大小：滑条 ⇄ 窗口尺寸，双向实时 ============
// 滑条拖动（input）→ 120ms 防抖后实时应用窗口尺寸；
// 用户拖窗口边框（onResized）→ 滑条实时跟动，停手 500ms 后把尺寸回写持久化。
// 锁定后：滑条禁用变灰、后端把窗口设为不可缩放/不可最大化。
const WINDOW_MIN = { width: 360, height: 560 };

function updateWindowSizeStateLabels(width, height) {
  if (windowSizeWidthStateElement) {
    windowSizeWidthStateElement.textContent = `${width}px（拖动实时调整窗口）`;
  }
  if (windowSizeHeightStateElement) {
    windowSizeHeightStateElement.textContent = `${height}px（拖动实时调整窗口）`;
  }
}

/// 快照 → 控件（锁定状态也在这里落地：禁用滑条 + 变灰）。
function syncWindowSizeControls(view) {
  // windowHeight == 0 = 后端「还没记过尺寸」（它会按**屏幕工作区一半**现算）。
  // 这一刻别拿旧常量 900 去糊滑条 —— 用窗口实际视口高度当近似值，
  // 免得滑条显示 900、窗口其实是 560（隐藏期 innerHeight 是 0，会落到兜底）。
  const width = Number(view.windowWidth) || WINDOW_MIN.width;
  const height = Number(view.windowHeight) || Math.round(window.innerHeight) || 900;
  const locked = Boolean(view.windowSizeLocked);
  if (document.activeElement !== windowSizeWidthElement) {
    windowSizeWidthElement.value = String(width);
  }
  if (document.activeElement !== windowSizeHeightElement) {
    windowSizeHeightElement.value = String(height);
  }
  windowSizeWidthElement.disabled = locked;
  windowSizeHeightElement.disabled = locked;
  windowSizeLockedElement.checked = locked;
  updateWindowSizeStateLabels(width, height);
}

let windowSizeApplyTimer = 0;
function applyWindowSizeFromSliders() {
  const width = Number.parseInt(windowSizeWidthElement.value, 10);
  const height = Number.parseInt(windowSizeHeightElement.value, 10);
  if (!Number.isFinite(width) || !Number.isFinite(height)) {
    return;
  }
  updateWindowSizeStateLabels(width, height);
  return call("set_window_size", { width, height });
}

for (const slider of [windowSizeWidthElement, windowSizeHeightElement]) {
  // 拖动状态跟踪（修用户实测的 bug：鼠标离开滑块后 hover 还在改值）。
  // 根因：拖动中窗口跟着滑块变宽变窄，滑块自己往指针底下挪，WebKit 的
  // range 输入可能不结束隐式指针捕获 —— 松手后 mousemove 继续产生幽灵
  // input 事件。对策：input 只认两个来源 —— 指针真按着（dragging）或
  // 键盘操作（:focus-visible），其余一律忽略。
  let dragging = false;
  slider.addEventListener("pointerdown", () => {
    dragging = true;
  });
  slider.addEventListener("pointerup", () => {
    dragging = false;
  });
  slider.addEventListener("pointercancel", () => {
    dragging = false;
  });
  // 兜底：松手落在滑块之外（拖出窗口/捕获异常）也不许卡在「拖动中」状态
  window.addEventListener(
    "pointerup",
    () => {
      dragging = false;
    },
    { capture: true },
  );
  slider.addEventListener("input", () => {
    if (!dragging && !slider.matches(":focus-visible")) {
      return; // 幽灵输入：不是拖动也不是键盘，忽略
    }
    const width = Number.parseInt(windowSizeWidthElement.value, 10);
    const height = Number.parseInt(windowSizeHeightElement.value, 10);
    updateWindowSizeStateLabels(width, height);
    clearTimeout(windowSizeApplyTimer);
    windowSizeApplyTimer = setTimeout(applyWindowSizeFromSliders, 120);
  });
  // 松手立刻提交一次（不用等防抖计时器走完）
  slider.addEventListener("change", () => {
    clearTimeout(windowSizeApplyTimer);
    applyWindowSizeFromSliders();
  });
}

windowSizeLockedElement.addEventListener("change", () =>
  call("set_window_size_locked", { locked: windowSizeLockedElement.checked }),
);

// 窗口 → 滑条（双向的另一半）：用户拖窗口边框时滑条跟动。
// 贴边成条时窗口会缩到几十像素宽 —— 那是程序自己的临时形态，不是用户调尺寸，
// 小于下限的尺寸一律不进滑条、不回写。
let windowResizePersistTimer = 0;
// 浏览器直开没有窗口对象：滑条双向同步只在应用内生效
if (!IS_BROWSER) {
window.__TAURI__.window
  .getCurrentWindow()
  .onResized(({ payload: physical }) => {
    if (windowSizeLockedElement.checked) {
      return;
    }
    if (physical.width < WINDOW_MIN.width || physical.height < WINDOW_MIN.height) {
      return;
    }
    window.__TAURI__
      .window
      .getCurrentWindow()
      .scaleFactor()
      .then((scale) => {
        const width = Math.round(physical.width / scale);
        const height = Math.round(physical.height / scale);
        if (document.activeElement !== windowSizeWidthElement) {
          windowSizeWidthElement.value = String(width);
        }
        if (document.activeElement !== windowSizeHeightElement) {
          windowSizeHeightElement.value = String(height);
        }
        updateWindowSizeStateLabels(width, height);
        // 停手 500ms 才回写：拖动过程每次 Resized 都写盘太频繁
        clearTimeout(windowResizePersistTimer);
        windowResizePersistTimer = setTimeout(() => {
          void call("set_window_size", { width, height });
        }, 500);
      });
  });
}

hideFromDockTaskbarElement.addEventListener("change", () =>
  call("set_hide_from_dock_taskbar", {
    enabled: hideFromDockTaskbarElement.checked,
  }),
);

closeActionElement.addEventListener("change", () =>
  call("set_close_action", { action: closeActionElement.value }),
);

// 主题：先立刻预览（不等往返），再落盘
themeElement.addEventListener("change", () => {
  applyAppearance(themeElement.value, Number(opacityElement.value));
  call("set_theme", { theme: themeElement.value });
});

// 不透明度：拖动即时预览，松手（change）才写设置，免得拖一次写十几次
opacityElement.addEventListener("input", () => {
  applyAppearance(themeElement.value, Number(opacityElement.value));
  // 自己监听同一个事件做即时重绘（拖动期间后端还没收到新值）
  emitEvent("ui-alpha-preview", Number(opacityElement.value));
});
opacityElement.addEventListener("change", () => {
  call("set_opacity", { percent: Number(opacityElement.value) });
});

// 选「跟随系统」时，系统明暗变了要跟着换（不用等下一次快照）
window
  .matchMedia("(prefers-color-scheme: light)")
  .addEventListener("change", () => {
    if (themeElement.value === "system") {
      applyAppearance("system", Number(opacityElement.value));
    }
  });

loginItemsElement.addEventListener("click", async () => {
  try {
    await invoke("open_login_items_settings");
    showStatusBar("已打开系统设置的登录项面板");
  } catch (error) {
    showStatusBar(String(error), { error: true });
  }
});

// ---- 唤醒钩子（主窗，后端 show 时 eval 调用） ----
// macOS 上主窗启动即隐藏（startInTray）时，WKWebView 的 JS 会被深度冻结：
// 启动早期的 get_ui_state/listen/setInterval 全部没跑或跑一半就停，show 出来
// 也不会自动恢复 —— 用户看到的就是「三个栏目都有框架但全没数据」（100% 复现）。
// 后端 show_main_window 时 eval 这个钩子：渲染过 = 只补拉一拍；没渲染过 =
// 整页重载（此刻窗口已可见，重载后的初始化会正常执行）。
window.__ANYBOOT_REVIVE__ = async () => {
  try {
    if (hasRenderedSnapshot) {
      // 已经渲染过：补一拍即可，绝不重载（重载 = 可见的闪烁/白屏）
      applySnapshot(await tauri.core.invoke("get_ui_state"));
      return "pulled";
    }
    // 还没渲染过 = 页面初始化可能被冻结吞掉了。
    // **不重载**：重载会让窗口白一下、再重新初始化，用户看到闪烁。
    // 改为就地重跑一次「首拉」：等 kickstart 完成后主动 invoke 拿快照并渲染。
    // （启动早期的初始化若只跑到一半，listen 可能没注册成功；但这里的主动拉
    //   不依赖 listen —— 直接 invoke，拿到就 render。）
    startTauriStatePolling();
    applySnapshot(await tauri.core.invoke("get_ui_state"), { force: true });
    return "bootstrapped";
  } catch (error) {
    // 连 invoke 都失败（页面根本没加载完）：只能重载兜底
    location.reload();
    return `error:${error}`;
  }
};

// ---- 页面级异常上报（主窗） ----
// WKWebView 的 console 我们看不到；「数据不加载」这类问题必须让 JS 侧异常
// 落到后端 trace.log 才能定位。只 Tauri 主窗启用；上报本身失败静默。
if (!IS_BROWSER) {
  const report = (kind, detail) => {
    try {
      void tauri.core.invoke("sol_log_client_error", { kind, detail });
    } catch {
      void kind;
    }
  };
  window.addEventListener("error", (event) =>
    report("error", `${event.message} @ ${event.filename}:${event.lineno}`),
  );
  window.addEventListener("unhandledrejection", (event) =>
    report("rejection", String(event.reason)),
  );
}

// 所有元素都已声明完毕 —— 现在才能安全地激活初始栏目
//（见 pendingInitialTab 处的注释：提前激活会撞 SOL 面板的 const TDZ）
activateTab(pendingInitialTab);

(async () => {
  if (IS_BROWSER) {
    await probeRemoteApi();
    renderBrowserBanner();
    // 首次访问（没有存过 token）：直接弹模态框要 token，别等第一次 401
    if (remoteApiAvailable && !remoteToken) {
      showTokenModal();
    }
    // 手指从页面顶部往下拖就刷新（触屏上比去点横幅里的按钮顺手得多）
    installPullToRefresh();
    // 探不通不等于没有后端：可能电脑端正在重启、或网络刚起来。别把这一刻
    // 定格成「纯预览模式」（那会让正文永远空着），挂上重试直到连上。
    if (!remoteApiAvailable) {
      scheduleRemoteApiReprobe();
    }
  }

  // 托盘里的改动会通过同一个事件把完整快照推过来，两边永远同源。
  await listen("ui-state-changed", (event) => applySnapshot(event.payload));
  // 兜底：事件偶发丢失时按 revision 去重轮询（只 Tauri 主窗；见 startTauriStatePolling）
  startTauriStatePolling();

  // 托盘里的「远程控制设置…」：先把主窗口切到「基本设置」栏目再滚到这一区。
  // 只滚不切的话，窗口停在「系统引导」时远程控制区整块是隐藏的，滚了也看不见。
  await listen("focus-remote", () => {
    activateTab("settings");
    remoteSectionElement.scrollIntoView({ behavior: "smooth", block: "start" });
  });

  try {
    applySnapshot(await invoke("get_ui_state"), { force: true });
  } catch (error) {
    // 无后端的纯预览模式不必刷错误条（横幅已经说明了）
    if (remoteApiAvailable) {
      showStatusBar(String(error), { error: true });
    }
  }

  // 「远程怎么调」默认就展开：第一份快照渲染时 renderRemote 已经把含 token 的
  // 示例填好了（见 refreshHttpHintIfChanged），这里不用再取一次。
  // 纯预览模式没有后端，跳过（call 会报错）。
  if (!IS_BROWSER || remoteApiAvailable) {
    await call("refresh");
  }
})();