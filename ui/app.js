const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

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
const hideFromDockTaskbarElement = document.querySelector("#hide-from-dock-taskbar");
const refreshElement = document.querySelector("#refresh");
const useDirectoryElement = document.querySelector("#use-directory");
// 下次启动大字（问题 7）
const nextBootElement = document.querySelector("#next-boot");
const nextBootNameElement = document.querySelector("#next-boot-name");
// 外观（问题 8）
const themeElement = document.querySelector("#theme");
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
const grubDirOpenElement = document.querySelector("#grub-dir-open");
const grubMountElement = document.querySelector("#grub-mount");
const grubMountLabelElement = document.querySelector("#grub-mount-label");
const grubPartElement = document.querySelector("#grub-part");
const grubUuidElement = document.querySelector("#grub-uuid");
const grubUuidSaveElement = document.querySelector("#grub-uuid-save");
const grubSyncElement = document.querySelector("#grub-sync");
const grubHintElement = document.querySelector("#grub-hint");
const grubErrorElement = document.querySelector("#grub-error");
const grubListElement = document.querySelector("#grub-list");
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

/// ESP 分区那一行：FAT32 判定 · fs-uuid · 设备 · 挂载状态 · 出处。
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

  if (esp.uuid) {
    add(document.createTextNode(`fs-uuid ${esp.uuid}`));
  } else {
    const missing = document.createElement("i");
    missing.textContent = "没取出 fs-uuid";
    add(missing);
  }
  if (esp.device) {
    add(document.createTextNode(esp.device));
  }
  // 挂载点摆出来（用户「填了 fs-uuid 还是找不到」多数就是这一行空着/没挂上）
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

/// GRUB 区块：列的是 grub.cfg 里的启动项，条数可能几十个，
/// 所以字号比上面三系统小一号（用户明确要求）。
///
/// 三处输入框（ESP 目录 / fs-uuid / 脚本正文）只在**没被聚焦**时才回写，
/// 否则用户打字打到一半会被快照盖掉。
function renderGrub(grub, script, busy) {
  if (!grub) {
    return;
  }

  if (document.activeElement !== grubUuidElement) {
    grubUuidElement.value = grub.uuid || "";
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
  const canMount = mounter && Boolean(grub.uuid);
  grubMountElement.disabled = busy || !canMount;
  grubMountElement.title = !mounter
    ? "Windows 不能按 fs-uuid 挂载：ESP 有没有盘符由「磁盘管理」决定"
    : grub.uuid
      ? ""
      : "先填好 fs-uuid（或用左边的「识别」自动取）";
  grubMountLabelElement.textContent = mounter
    ? "挂载 ESP（勾上后可直接读写）"
    : "ESP 可访问（Windows 由盘符决定，无需挂载）";

  const entries = grub.entries || [];
  const parts = [];
  if (grub.efiDir) {
    parts.push(`EFI 目录 ${grub.efiDir}`);
  }
  parts.push(`${entries.length} 个启动项`);  if (grub.syncedAt) {
    parts.push(`同步于 ${grub.syncedAt}`);
  }
  grubMetaElement.textContent = parts.join(" · ");

  grubHintElement.textContent = grub.hint || "";
  grubErrorElement.textContent = grub.error || "";
  grubErrorElement.classList.toggle("hidden", !grub.error);

  // 当前生效的条目：优先看一次性项，其次看永久默认（都来自 ESP 上的环境块）
  const activeSelector = grub.nextEntry || grub.savedEntry || "";

  grubListElement.replaceChildren();
  if (!entries.length) {
    const empty = document.createElement("small");
    empty.className = "grub-empty";
    empty.textContent = grub.uuid
      ? "还没有可显示的启动项 —— 先在 Linux 上装好 09_anyboot 并同步一次"
      : "填好上面的 ESP 目录（点「识别」）或 fs-uuid，就能列出 GRUB 启动项";
    grubListElement.append(empty);
  }

  // grub.cfg 里 submenu 是分组、不是可选条目（解析器按同一规则跳过它），
  // 所以这里**自己把组头补出来**：子条目归到它的父 submenu 下，并统计组内数量。
  // 后端还给了 submenu 的 (id → 标题) 映射，组头就能显示真名而不是一串 id。
  const groupTitles = new Map(
    (grub.groups || []).map((group) => [group.id, group.name]),
  );
  const childrenOf = new Map();
  for (const entry of entries) {
    if (entry.submenu) {
      if (!childrenOf.has(entry.submenu)) {
        childrenOf.set(entry.submenu, []);
      }
      childrenOf.get(entry.submenu).push(entry);
    }
  }

  const makeRow = (entry, { group, depth }) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "grub-entry";
    row.classList.add(group ? "is-group" : "is-child");
    if (depth >= 2) {
      row.classList.add(`depth-${depth}`);
    }
    if (entry.selector === activeSelector) {
      row.classList.add("active");
    }
    row.disabled = busy;
    row.title = entry.selector;

    const name = document.createElement("span");
    name.className = "grub-entry-name";
    // 缩进靠 CSS（.is-child / .depth-N），这里只用箭头补强「这是组里的项」
    name.textContent = group || depth === 0 ? entry.name : `↳ ${entry.name}`;

    const tag = document.createElement("small");
    if (group) {
      tag.className = "grub-entry-count";
      tag.textContent = `${(childrenOf.get(entry.selector) || []).length} 项`;
      // 组头不是 menuentry，写进 grubenv 也切不过去 —— 只作标题，不可点
      row.disabled = true;
    } else {
      // 同名条目已经在后端折叠过了：这里标出它背后有几个变体，
      // 免得用户以为「少列了」，也免得再出现两行一模一样的文字。
      const variants = Number(entry.variants) || 1;
      const parts = [];
      if (variants > 1) {
        parts.push(`${variants} 个变体`);
      }
      parts.push(
        entry.chainloader
          ? "转交引导"
          : (entry.kernels && entry.kernels[0]) || entry.class || "",
      );
      tag.className = "grub-entry-tag";
      tag.textContent = parts.filter(Boolean).join(" · ");
    }

    row.append(name, tag);
    if (!group) {
      row.addEventListener("click", () =>
        call("switch_grub", { selector: entry.selector }),
      );
    }
    return row;
  };

  // 顶层条目按原顺序铺开；走到第一条子项时，先把它的组头插进去，再补齐同组子项。
  const emittedGroups = new Set();
  for (const entry of entries) {
    if (!entry.submenu) {
      grubListElement.append(makeRow(entry, { group: false, depth: 0 }));
      continue;
    }
    if (emittedGroups.has(entry.submenu)) {
      continue;
    }
    emittedGroups.add(entry.submenu);
    grubListElement.append(
      makeRow(
        {
          selector: entry.submenu,
          name: groupTitles.get(entry.submenu) || entry.submenu,
        },
        { group: true, depth: 1 },
      ),
    );
    for (const child of childrenOf.get(entry.submenu) || []) {
      grubListElement.append(makeRow(child, { group: false, depth: 1 }));
    }
  }

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

/// 把主题与不透明度落到 documentElement 上（CSS 用 var(--ui-alpha) 读）
function applyAppearance(theme, percent) {
  document.documentElement.dataset.theme = resolvedTheme(theme);
  const value = Number(percent);
  const clamped = Math.min(100, Math.max(0, Number.isFinite(value) ? value : 100));
  const alpha = clamped / 100;
  document.documentElement.style.setProperty("--ui-alpha", String(alpha));
  opacityStateElement.textContent = `${clamped}%（拖动即时预览）`;
}

function renderSettings(view) {
  autostartElement.checked = Boolean(view.autostart);
  startInTrayElement.checked = Boolean(view.startInTray);
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
    const clock = (entry.time || "").slice(11, 19);
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
  pendingDetailElement.textContent = `${notify.message}｜来源 ${notify.source}｜渠道 ${notify.channel}｜发起 ${notify.created}`;
  pendingRunElement.classList.toggle("hidden", !notify.canCancel);
  pendingCancelElement.classList.toggle("hidden", !notify.canCancel);
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

  renderGrub(state.grub, state.grubScript, busy);
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
    grubUuidSaveElement,
    grubSyncElement,
    grubDirDetectElement,
    grubDirOpenElement,
    grubInstallElement,
    grubUninstallElement,
    grubScriptSaveElement,
    grubScriptResetElement,
  ]) {
    element.disabled = busy;
  }
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

// 识别 ESP 目录：后端判断是不是 FAT32，并把从中取到的 fs-uuid 填好
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

// 三端都用系统文件管理器打开解析出来的 ESP 目录
grubDirOpenElement.addEventListener("click", async () => {
  grubMessage("");
  try {
    grubMessage(`已用文件管理器打开：${await invoke("open_grub_dir")}`);
  } catch (error) {
    grubMessage(String(error), { error: true });
  }
});

// 勾选 = 按 fs-uuid 挂载并保持；取消 = 卸掉**本程序自己挂的**那份
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
grubUuidSaveElement.addEventListener("click", () =>
  call("set_grub_uuid", { uuid: grubUuidElement.value.trim() }),
);

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

httpEnabledElement.addEventListener("change", () =>
  call("set_http_enabled", { enabled: httpEnabledElement.checked }),
);

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

notifyEnabledElement.addEventListener("change", () =>
  call("set_notify_enabled", { enabled: notifyEnabledElement.checked }),
);

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

// 提示音开关与路径
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
  // 托盘里的改动会通过同一个事件把完整快照推过来，两边永远同源。
  await listen("ui-state-changed", (event) => applySnapshot(event.payload));

  // 托盘里的「远程控制设置…」：把窗口打开并滚到这一区
  await listen("focus-remote", () =>
    remoteSectionElement.scrollIntoView({ behavior: "smooth", block: "start" }),
  );

  try {
    applySnapshot(await invoke("get_ui_state"), { force: true });
  } catch (error) {
    showStatusBar(String(error), { error: true });
  }

  // 「远程怎么调」默认就展开：第一份快照渲染时 renderRemote 已经把含 token 的
  // 示例填好了（见 refreshHttpHintIfChanged），这里不用再取一次。
  await call("refresh");
})();