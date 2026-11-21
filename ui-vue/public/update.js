// 更新弹窗（独立窗口，与 notify 弹窗同一套 Tauri v2 API）
const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const el = (id) => document.getElementById(id);
const title = el("title");
const current = el("current");
const meta = el("meta");
const notes = el("notes");
const progressWrap = el("progress-wrap");
const barFill = el("bar-fill");
const pct = el("pct");
const bytes = el("bytes");
const source = el("source");
const errorBox = el("error");
const btnDownload = el("download");
const btnInstall = el("install");
const btnSkip = el("skip");
const btnLater = el("later");

let state = null;

function human(n) {
  if (!n) return "0 MB";
  return (n / 1048576).toFixed(1) + " MB";
}

function render(next) {
  if (!next) return;
  state = next;
  title.textContent = next.latestVersion ? `发现新版本 v${next.latestVersion}` : "发现新版本";
  current.textContent = next.currentVersion ? `当前 v${next.currentVersion}` : "";
  meta.textContent = next.publishedAt ? `发布于 ${next.publishedAt}` : "";
  notes.textContent = next.notes || "（这个版本没有填写更新日志）";
  notes.hidden = !next.notes;

  const phase = next.phase || "idle";
  const total = next.total || 0;
  const downloaded = next.downloaded || 0;
  const percent = total ? Math.min(100, Math.round((downloaded / total) * 100)) : 0;
  const downloading = phase === "downloading" || phase === "verifying";
  progressWrap.hidden = !(downloading || phase === "ready_to_install" || phase === "installing");
  barFill.style.width = percent + "%";
  pct.textContent = percent + "%";
  bytes.textContent = total ? `${human(downloaded)}/${human(total)}` : "";
  source.textContent = next.source ? `来源：${next.source}` : "";

  errorBox.hidden = !next.error;
  errorBox.textContent = next.error || "";

  btnDownload.hidden = !(phase === "available" || (phase === "error" && next.assetName));
  btnDownload.disabled = downloading || phase === "installing";
  btnDownload.textContent = downloading ? "下载中…" : "下载";
  btnInstall.hidden = !(phase === "ready_to_install" || phase === "installing");
  btnInstall.disabled = phase === "installing";
  btnInstall.textContent = phase === "installing" ? "安装中…" : "安装并重启";
}

async function refresh() {
  try {
    render(await invoke("update_state"));
  } catch (e) {
    errorBox.hidden = false;
    errorBox.textContent = String(e);
  }
}

btnDownload.addEventListener("click", async () => {
  btnDownload.disabled = true;
  try {
    await invoke("update_download");
  } catch (e) {
    errorBox.hidden = false;
    errorBox.textContent = String(e);
    btnDownload.disabled = false;
  }
  refresh();
});

btnInstall.addEventListener("click", async () => {
  btnInstall.disabled = true;
  try {
    await invoke("update_install");
  } catch (e) {
    errorBox.hidden = false;
    errorBox.textContent = String(e);
    btnInstall.disabled = false;
  }
});

btnSkip.addEventListener("click", async () => {
  const version = state && state.latestVersion ? state.latestVersion : "";
  try { await invoke("update_dismiss", { version }); } catch (e) { /* 忽略 */ }
  closeWindow();
});

btnLater.addEventListener("click", closeWindow);

function closeWindow() {
  try {
    window.__TAURI__.window.getCurrentWindow().close();
  } catch (e) {
    /* 保底：hide 也行 */
    try { window.__TAURI__.window.getCurrentWindow().hide(); } catch (_) { /* noop */ }
  }
}

listen("update-state", (event) => render(event.payload));
// 打开时先拿当前状态；如果还没检查过就主动查一次
invoke("update_state").then((initial) => {
  render(initial);
  if (!initial || initial.phase === "idle") {
    invoke("update_check").catch((e) => {
      errorBox.hidden = false;
      errorBox.textContent = String(e);
    });
  }
}).catch(() => {});
refresh();
setInterval(refresh, 2000);
