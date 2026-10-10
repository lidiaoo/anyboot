const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const titleElement = document.querySelector("#title");
const sourceElement = document.querySelector("#source");
const messageElement = document.querySelector("#message");
const barFillElement = document.querySelector("#bar-fill");
const countdownElement = document.querySelector("#countdown");
const actionsElement = document.querySelector("#actions");
const hintElement = document.querySelector("#hint");
const panelElement = document.querySelector(".panel");

// 与主窗口同一套契约：快照带 revision，只认更新的
let lastRevision = -1;

// 倒计时本地 tick：快照里的 secondsLeft 只有 emit 时刷新（每 ~250ms 一次调度 tick），
// 前端自己每秒更新一次显示，让数字和进度条是流动的，不用等下一次快照。
let tickTimer = null;
let tickContext = null;

function stopLocalTick() {
  if (tickTimer) {
    clearInterval(tickTimer);
    tickTimer = null;
    tickContext = null;
  }
}

function startLocalTick(notify) {
  stopLocalTick();
  const total = Math.max(1, notify.totalSeconds);
  let remaining = notify.secondsLeft;
  if (remaining <= 0) return;

  const tick = () => {
    remaining = Math.max(0, remaining - 1);
    const elapsed = Math.max(0, Math.min(total, total - remaining));
    countdownElement.textContent = `${remaining} 秒后自动执行`;
    barFillElement.style.width = `${Math.round((elapsed / total) * 100)}%`;
    if (remaining === 0) {
      // 到点了：等后端调度线程真正执行、publish_ui 把弹窗收掉；这里只停本地 tick
      stopLocalTick();
    }
  };
  // 每秒刷新一次；第一帧立刻画一遍（避免整秒边界等待）
  tickTimer = setInterval(tick, 1000);
  tick();

  tickContext = notify;
}

/// 提示音不在这里播：webview 里 new Audio(本地路径) 读不到文件、
/// 默认那段 base64 也没有采样数据，Linux 还依赖 gstreamer 插件。
/// 现在由后端在弹窗那一刻用系统播放器播（paplay/aplay/afplay/powershell）。

/// 量出「刚好兜住内容」的窗口高度（CSS 像素 = 逻辑像素）。
///
/// 弹窗窗口是隐藏状态被创建的，WebView 照样会跑 layout，所以这里量得到。
/// 万一量不到（拿到 0 / NaN），返回 null 让后端退回标称高度 ——
/// 宁可不够紧凑，也不能因为窗口太矮把最后一行文字裁掉。
function measureContentHeight() {
  const last = [...panelElement.children]
    .reverse()
    .find((element) => element.getBoundingClientRect().height > 0);
  if (!last) {
    return null;
  }
  const style = getComputedStyle(panelElement);
  const height =
    last.getBoundingClientRect().bottom -
    panelElement.getBoundingClientRect().top +
    (parseFloat(style.paddingBottom) || 0) +
    (parseFloat(style.borderBottomWidth) || 0);
  return Number.isFinite(height) && height > 0 ? Math.ceil(height) : null;
}

/// 上次报给后端的高度：内容没变就不去折腾窗口（改尺寸 + 重新摆位是有代价的）。
/// null 表示「这一帧什么都不报」或者「量不出来、用后端标称高度」。
let presentedHeight = null;

async function presentWindow() {
  const height = measureContentHeight();
  if (height === presentedHeight) {
    return;
  }
  presentedHeight = height;
  try {
    await invoke("present_notify_window", { height });
  } catch (error) {
    hintElement.textContent = String(error);
  }
}

/// 主题取值 → 实际生效的亮/暗（与主界面 app.js 同一套规则）
function resolvedTheme(theme) {
  if (theme === "system") {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  return theme === "light" ? "light" : "dark";
}

function applyTheme(settings) {
  document.documentElement.dataset.theme = resolvedTheme(settings && settings.theme);
}

function render(notify) {
  if (!notify) {
    stopLocalTick();
    return;
  }
  titleElement.textContent = notify.title;
  sourceElement.textContent = `来源 ${notify.source}｜渠道 ${notify.channel}｜发起 ${notify.created}`;
  messageElement.textContent = notify.message;

  if (notify.kind === "pending") {
    const total = Math.max(1, notify.totalSeconds);
    const elapsed = Math.max(0, Math.min(total, total - notify.secondsLeft));
    countdownElement.textContent = `${notify.secondsLeft} 秒后自动执行`;
    barFillElement.style.width = `${Math.round((elapsed / total) * 100)}%`;
    actionsElement.classList.remove("hidden");
    hintElement.textContent = "点「立即执行」马上执行；点「取消」或关掉窗口 = 不执行";
    startLocalTick(notify);
  } else {
    countdownElement.textContent = "已执行";
    barFillElement.style.width = "100%";
    actionsElement.classList.add("hidden");
    hintElement.textContent = "这个提示会自己关掉";
    stopLocalTick();
  }
  // 内容画完再量高度，然后把窗口定成「刚好兜住文字」的大小并显示出来
  void presentWindow();
}

function applySnapshot(state, { force = false } = {}) {
  if (!state || typeof state.revision !== "number") {
    return;
  }
  if (!force && state.revision <= lastRevision) {
    return;
  }
  lastRevision = Math.max(lastRevision, state.revision);
  applyTheme(state.settings);
  render(state.notify);
}

document.querySelector("#run").addEventListener("click", async () => {
  try {
    await invoke("confirm_pending");
  } catch (error) {
    hintElement.textContent = String(error);
  }
});

document.querySelector("#cancel").addEventListener("click", async () => {
  try {
    await invoke("cancel_pending");
  } catch (error) {
    hintElement.textContent = String(error);
  }
});

(async () => {
  // 「跟随系统」时系统明暗变了要跟着换
  window.matchMedia("(prefers-color-scheme: light)").addEventListener("change", async () => {
    try {
      applyTheme((await invoke("get_ui_state")).settings);
    } catch (error) {
      void error;
    }
  });

  await listen("ui-state-changed", (event) => applySnapshot(event.payload));
  try {
    applySnapshot(await invoke("get_ui_state"), { force: true });
  } catch (error) {
    messageElement.textContent = String(error);
  }
})();
