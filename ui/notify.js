const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const titleElement = document.querySelector("#title");
const sourceElement = document.querySelector("#source");
const messageElement = document.querySelector("#message");
const barFillElement = document.querySelector("#bar-fill");
const countdownElement = document.querySelector("#countdown");
const actionsElement = document.querySelector("#actions");
const hintElement = document.querySelector("#hint");

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

  // 播放提示音（pending 进入时播一次，不是每秒）
  playSoundNotification(notify);
  tickContext = notify;
}

/// 用 system beep 代替（HTML5 Audio + 系统提示音）
function playSoundNotification(notify) {
  try {
    // Tauri webview 支持 <audio>，路径通过 tauri://fs/ 或直接用本地路径
    const path = notify.soundFilePath;
    if (path && path.trim()) {
      const audio = new Audio(path);
      audio.play().catch(() => {});
    } else {
      // 默认：浏览器 system beep
      const audio = new Audio('data:audio/wav;base64,UklGRigAAABXQVZFZm10IBAAAAABAAEARKEAACAhIQMgFAABOUZpbZM=');
      audio.play().catch(() => {});
    }
  } catch (e) {
    // 忽略
  }
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
}

function applySnapshot(state, { force = false } = {}) {
  if (!state || typeof state.revision !== "number") {
    return;
  }
  if (!force && state.revision <= lastRevision) {
    return;
  }
  lastRevision = Math.max(lastRevision, state.revision);
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
  await listen("ui-state-changed", (event) => applySnapshot(event.payload));
  try {
    applySnapshot(await invoke("get_ui_state"), { force: true });
  } catch (error) {
    messageElement.textContent = String(error);
  }
})();
