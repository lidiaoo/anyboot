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

function render(notify) {
  if (!notify) {
    return;
  }
  titleElement.textContent = notify.title;
  sourceElement.textContent = `来源 ${notify.source}｜渠道 ${notify.channel}｜发起 ${notify.created}`;
  messageElement.textContent = notify.message;

  if (notify.kind === "pending") {
    countdownElement.textContent = `${notify.secondsLeft} 秒后自动执行`;
    const total = Math.max(1, notify.totalSeconds);
    const elapsed = Math.max(0, Math.min(total, total - notify.secondsLeft));
    barFillElement.style.width = `${Math.round((elapsed / total) * 100)}%`;
    actionsElement.classList.remove("hidden");
    hintElement.textContent = "点「立即执行」马上执行；点「取消」或关掉窗口 = 不执行";
  } else {
    countdownElement.textContent = "已执行";
    barFillElement.style.width = "100%";
    actionsElement.classList.add("hidden");
    hintElement.textContent = "这个提示会自己关掉";
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