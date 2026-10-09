const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const statusElement = document.querySelector("#status");
const systemsElement = document.querySelector("#systems");
const directoryElement = document.querySelector("#vars-dir");
const messageElement = document.querySelector("#message");
const autostartElement = document.querySelector("#autostart");
const closeActionElement = document.querySelector("#close-action");
const unmountTemporaryElement = document.querySelector("#unmount-temporary");
const settingsMessageElement = document.querySelector("#settings-message");
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

// 远程控制区
const remoteSectionElement = document.querySelector("#remote");
const httpEnabledElement = document.querySelector("#http-enabled");
const httpStateElement = document.querySelector("#http-state");
const httpBindElement = document.querySelector("#http-bind");
const httpPortElement = document.querySelector("#http-port");
const httpApplyElement = document.querySelector("#http-apply");
const tokenHintElement = document.querySelector("#token-hint");
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
const remoteMessageElement = document.querySelector("#remote-message");

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

function renderSystems(status, busy) {
  systemsElement.replaceChildren();

  for (const item of status.variables) {
    const card = document.createElement("article");
    card.className = [
      "system-card",
      item.active ? "active" : "",
      item.exists ? "" : "unavailable",
    ].filter(Boolean).join(" ");

    const name = document.createElement("div");
    name.className = "system-name";
    const title = document.createElement("strong");
    title.textContent = item.active ? `● ${item.displayName}` : item.displayName;
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

    card.append(name, button);
    systemsElement.append(card);
  }
}

function renderSettings(view) {
  autostartElement.checked = Boolean(view.autostart);
  startInTrayElement.checked = Boolean(view.startInTray);
  hideFromDockTaskbarElement.checked = Boolean(view.hideFromDockTaskbar);
  closeActionElement.value = view.closeAction === "quit" ? "quit" : "tray";
  autostartStateElement.textContent =
    autostartStateLabels.get(view.autostartState) || "";
  settingsMessageElement.textContent = view.autostartHint || "";
  loginItemsElement.classList.toggle(
    "hidden",
    view.autostartState !== "requires-approval",
  );
}

function renderRemote(http, settings) {
  httpEnabledElement.checked = Boolean(http.enabled);
  httpBindElement.value = http.bind || "";
  httpPortElement.value = String(http.port === undefined ? "" : http.port);
  httpStateElement.textContent = http.listening
    ? `正在监听 ${http.addr}`
    : http.lastError
      ? `未监听：${http.lastError}`
      : http.tokenSet
        ? "未监听（服务已关闭）"
        : "未监听（token 为空，服务不会启动）";
  tokenHintElement.textContent = `当前令牌：${http.tokenHint}`;

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

  httpRequestsElement.replaceChildren();
  for (const entry of http.requests) {
    const item = document.createElement("li");
    const clock = (entry.time || "").slice(11, 19);
    item.textContent = `${clock} ${entry.source} [${entry.channel}] ${entry.summary} — ${entry.status}${
      entry.message ? `：${entry.message}` : ""
    }`;
    httpRequestsElement.append(item);
  }
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

  statusElement.textContent = busyLabel(state.busy) || state.status.message;
  messageElement.textContent = actionMessage(state);
  directoryElement.value = state.status.varsDir || "";
  unmountTemporaryElement.classList.toggle(
    "hidden",
    !state.status.temporaryMount,
  );

  renderSystems(state.status, busy);
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

// 所有命令调用都走这里：失败时除了显示原因，还要把界面拉回系统真实状态
// （勾选框不能停在「没生效」的位置 —— 阶段 1 修 D5 的前端一半）。
async function call(command, args) {
  messageElement.textContent = "";
  try {
    await invoke(command, args);
  } catch (error) {
    messageElement.textContent = String(error);
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

// ---------- 远程控制区 ----------

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
    remoteMessageElement.textContent = okMessage;
  } catch (error) {
    remoteMessageElement.textContent = `复制失败：${error}`;
  }
}

function applyDelayFrom(channel, element) {
  const seconds = Number.parseInt(element.value.trim(), 10);
  if (!Number.isInteger(seconds) || seconds < 0 || seconds > 600) {
    remoteMessageElement.textContent = "延迟必须是 0-600 之间的整数";
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
    remoteMessageElement.textContent = "监听地址不能为空（0.0.0.0 或 127.0.0.1）";
    return;
  }
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    remoteMessageElement.textContent = "端口必须是 1-65535 的整数";
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
    remoteMessageElement.textContent = "已显示明文令牌（注意旁人视线）";
  } catch (error) {
    remoteMessageElement.textContent = String(error);
  }
});

tokenCopyElement.addEventListener("click", async () => {
  try {
    await copyText(await invoke("reveal_http_token"), "令牌已复制到剪贴板");
  } catch (error) {
    remoteMessageElement.textContent = String(error);
  }
});

tokenRegenerateElement.addEventListener("click", async () => {
  try {
    const token = await invoke("regenerate_http_token");
    tokenHintElement.textContent = `新令牌：${token}`;
    remoteMessageElement.textContent = "已重新生成令牌，旧 token 立即失效";
  } catch (error) {
    remoteMessageElement.textContent = String(error);
  }
});

httpHintCopyElement.addEventListener("click", async () => {
  try {
    const text = await invoke("http_access_hint");
    httpHintElement.textContent = text;
    await copyText(text, "命令已复制到剪贴板");
  } catch (error) {
    remoteMessageElement.textContent = String(error);
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

loginItemsElement.addEventListener("click", async () => {
  settingsMessageElement.textContent = "";
  try {
    await invoke("open_login_items_settings");
    settingsMessageElement.textContent = "已打开系统设置的登录项面板";
  } catch (error) {
    settingsMessageElement.textContent = String(error);
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
    messageElement.textContent = String(error);
  }

  await call("refresh");
})();