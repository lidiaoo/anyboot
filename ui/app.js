// 本页面有两个运行环境：
// 1. Tauri 主窗口：window.__TAURI__ 注入，命令/事件走 IPC（原路径，不变）；
// 2. 任意浏览器：由内置 HTTP 服务（默认 12300）托管 —— 命令走 POST /api/ui-command、
//    状态走轮询 GET /api/ui-state（token 鉴权），设置改动**真正作用于本机应用**。
//    只有当页面由无后端的静态服务托管（探测 /health 失败）时才降级为纯预览。
const IS_BROWSER = typeof window.__TAURI__ === "undefined";
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
      if (event !== "ui-state-changed" || !remoteApiAvailable) {
        return () => {};
      }
      // 无推送通道：轮询完整快照兜底（2s；页面隐藏时跳过）
      const timer = setInterval(async () => {
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
            handler({ payload: await response.json() });
          } else if (response.status === 401) {
            requestRemoteToken();
          }
        } catch {
          // 断线时静默，下一轮再试
        }
      }, 2000);
      return () => clearInterval(timer);
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
const startInTrayElement = document.querySelector("#start-in-tray");
const mainAlwaysOnTopElement = document.querySelector("#main-always-on-top");
const mainAutoHideElement = document.querySelector("#main-auto-hide");
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
// 当前选中启动项的顶部横幅
const grubActiveElement = document.querySelector("#grub-active");
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

function renderSystems(status, busy) {
  systemsElement.replaceChildren();

  // 顶部那行大字要说明「下次启动的系统是哪个」，先扫一遍选中项
  let activeName = null;

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

    const button = document.createElement("button");
    button.type = "button";
    button.textContent = item.active
      ? "已选择"
      : switchButtonLabel.get(item.system);
    button.disabled = !item.exists || busy || Boolean(status.error);
    button.addEventListener("click", () =>
      call("switch_system", { system: item.system }),
    );

    card.append(check, name, button);
    systemsElement.append(card);
  }

  nextBootElement.classList.toggle("unknown", !activeName);
  nextBootNameElement.textContent = activeName || "未知（没有选中的系统）";
  // 顶部状态行要用它去重（见 render）
  return activeName;
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
/// · list 列表独立 —— 配置留在 #grub，当前选中横幅 + 列表搬进右栏；
/// · pane GRUB 独立 —— 全部子节点按出厂顺序搬进右栏（#grub 变空壳，CSS 藏掉）。
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
    const toPane =
      mode === "pane" ||
      (mode === "list" && (el === grubActiveElement || el === grubListElement));
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

  // 当前选中的启动项（右上角横幅，与树列表共用同一个渲染器）
  renderGrubActive(grubActiveElement, grub);

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
// 亮色主题悬停压暗，与内置配色（#2f7fe0 → hover #3d8dee / #256fd0）同方向。
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
function applyAccent(accent, theme) {
  const base = shadeHex(accent) || "#2f7fe0";
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
let currentAccent = "#2f7fe0";

/// 把主题与不透明度落到 documentElement 上（CSS 用 var(--ui-alpha) 读）
function applyAppearance(theme, percent) {
  document.documentElement.dataset.theme = resolvedTheme(theme);
  const value = Number(percent);
  const clamped = Math.min(100, Math.max(0, Number.isFinite(value) ? value : 100));
  const alpha = clamped / 100;
  document.documentElement.style.setProperty("--ui-alpha", String(alpha));
  applyAccent(currentAccent, theme);
  opacityStateElement.textContent = `${clamped}%（拖动即时预览）`;
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
  currentAccent = shadeHex(view.accentColor) || "#2f7fe0";
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
  for (const element of [
    mainDockStripHoverMsElement,
    mainDockStripEdgeGapElement,
    mainDockStripThicknessElement,
    mainDockStripLengthElement,
    mainDockStripOpacityElement,
    mainDockStripColorElement,
  ]) {
    element.disabled = !mainAutoHideElement.checked;
  }
}

function render(state) {
  const busy = isBusy(state.busy);

  // 操作回执统一走底部固定提示条（不再写在正文末尾，用户看不看得见全凭滚轮）
  syncStatusBarFromState(state);
  directoryElement.value = state.status.varsDir || "";
  unmountTemporaryElement.classList.toggle(
    "hidden",
    !state.status.temporaryMount,
  );

  const activeName = renderSystems(state.status, busy);
  // 顶部那行状态文案只在**跟「下次启动：X」不一样**时才显示：
  // 扫描成功后 status.message 本身就是「下次启动：X」，紧挨着下面的大字再来
  // 一遍就是用户报的「文字重复」。规则与托盘菜单的 menu_status_line 一致。
  const statusText = (state.status.message || "").trim();
  const duplicatesNextBoot =
    !statusText || statusText === `下次启动：${activeName || "未知"}`;
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
    return;
  }
  if (!force && state.revision <= lastRevision) {
    return;
  }
  lastRevision = Math.max(lastRevision, state.revision);
  currentState = state;
  render(state);
  applyDockStrip(state);
}

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
    await invoke(command, args);
  } catch (error) {
    showStatusBar(String(error), { error: true });
    try {
      applySnapshot(await invoke("get_ui_state"), { force: true });
    } catch (inner) {
      // 主错误已经显示，这里不再覆盖
      void inner;
    }
  }
}

refreshElement.addEventListener("click", () => call("refresh"));

// ---------- 界面栏目页签（地址栏可寻址） ----------
// 主窗口内容分两个栏目：系统引导（rEFInd + GRUB 同页，主功能）和
// 基本设置（应用设置 + 远程控制）。切换纯前端，但**栏目写进地址栏 hash**：
//   #/boot  → 系统引导
//   #/settings → 基本设置
// 这样刷新页面、收藏、把链接发给别人，都落回同一个栏目里。
const TAB_STORAGE_KEY = "anyboot.active-tab";
const TAB_ROUTES = { boot: "#/boot", settings: "#/settings" };
const tabButtons = Array.from(document.querySelectorAll("#tabs .tab"));
const tabPanels = {
  boot: document.querySelector("#tab-boot"),
  settings: document.querySelector("#tab-settings"),
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
  try {
    localStorage.setItem(TAB_STORAGE_KEY, name);
  } catch (error) {
    void error;
  }
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

// 初始栏目优先级：
// 1) 地址栏带了 #/xxx → 听地址栏的（刷新 / 分享链接都靠这条）；
// 2) 网页端（含手机 App 的 WebView）没有 hash → 固定进「系统引导」，
//    不继承上次栏目，免得每次打开都停在基本设置；
// 3) 桌面主窗口没有地址栏语义 → 沿用 localStorage 记忆。
function initialTab() {
  const fromLocation = tabFromLocation();
  if (fromLocation) {
    return fromLocation;
  }
  if (!IS_BROWSER) {
    try {
      const saved = localStorage.getItem(TAB_STORAGE_KEY);
      if (saved === "boot" || saved === "settings") {
        return saved;
      }
    } catch (error) {
      void error;
    }
  }
  return "boot";
}
activateTab(initialTab());

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
      // 初始快照立刻补上（轮询本来也在跑，这里不等下一拍）
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
mainWindowPositionElement.addEventListener("change", () =>
  call("set_main_window_position", { mode: mainWindowPositionElement.value }),
);

// 重置位置：立即把主窗口放到屏幕正中央
resetMainWindowPositionElement.addEventListener("click", () =>
  call("reset_main_window_position", {}),
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
  const width = Number(view.windowWidth) || 460;
  const height = Number(view.windowHeight) || 900;
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
  slider.addEventListener("input", () => {
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

(async () => {
  // 浏览器端：先探测后端（区分「远程控制」与「纯预览」），再决定横幅形态
  if (IS_BROWSER) {
    await probeRemoteApi();
    renderBrowserBanner();
    // 首次访问（没有存过 token）：直接弹模态框要 token，别等第一次 401
    if (remoteApiAvailable && !remoteToken) {
      showTokenModal();
    }
  }

  // 托盘里的改动会通过同一个事件把完整快照推过来，两边永远同源。
  await listen("ui-state-changed", (event) => applySnapshot(event.payload));

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