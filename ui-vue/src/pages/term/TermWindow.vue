<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 终端窗（原 `ui/hub-term.html`，695 行）—— hub_term* 命令族 6 个。
 *
 * ```
 * .titlebar   连接状态 + 主题 / 字号 / 回 hub
 * .tabbar     多会话标签
 * .term-host  每个会话一个 xterm 实例
 * .foot       光标信息
 * ```
 *
 * xterm.js 走 `public/terminal/`（UMD，脚本标签加载，不进打包）。
 *
 * ## 一条关键注释（从旧版搬来，勿删）
 *
 * ssh 走 ControlMaster 复用旧主连接时，banner + 提示符可能**早于**
 * `hub_term_open` 的 promise 落地就到 —— 不缓存的话终端会永远黑着。
 * 所以 `pendingData` 必须在 open 返回前就缓存。
 */

interface XTermLike {
  open(el: HTMLElement): void;
  write(data: string | Uint8Array): void;
  onData(cb: (data: string) => void): void;
  focus(): void;
  dispose(): void;
  options: Record<string, unknown>;
  loadAddon(addon: unknown): void;
}

interface Session {
  id: string;
  title: string;
  term: XTermLike;
  fit: { fit(): void } | null;
  host: HTMLElement;
  /** open 返回前到达的数据先攒这儿 */
  pendingData: string[];
  opened: boolean;
}

const termHostRef = ref<HTMLElement | null>(null);
const sessions = ref<Session[]>([]);
const activeId = ref("");
const connText = ref("连接中…");
const connOk = ref(false);
const errorText = ref("");

const unlisteners: (() => void)[] = [];

function loadScript(src: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const el = document.createElement("script");
    el.src = src;
    el.onload = () => resolve();
    el.onerror = () => reject(new Error(`加载失败: ${src}`));
    document.head.appendChild(el);
  });
}

function loadStyle(href: string) {
  const link = document.createElement("link");
  link.rel = "stylesheet";
  link.href = href;
  document.head.appendChild(link);
}

function readTermSettings(): { font: number; blink: boolean; cursorStyle: string; scrollback: number } {
  try {
    const raw = localStorage.getItem("anyboot.term-set");
    if (raw) {
      const parsed = JSON.parse(raw) as Record<string, unknown>;
      return {
        font: Number(parsed.font) || 14,
        blink: parsed.blink !== false,
        cursorStyle: String(parsed.cursorStyle || "block"),
        scrollback: Number(parsed.scrollback) || 5000,
      };
    }
  } catch {
    /* 吃默认 */
  }
  return { font: 14, blink: true, cursorStyle: "block", scrollback: 5000 };
}

function applyTermSettings() {
  const settings = readTermSettings();
  for (const session of sessions.value) {
    session.term.options.fontSize = settings.font;
    session.term.options.cursorBlink = settings.blink;
    session.term.options.cursorStyle = settings.cursorStyle;
    session.term.options.scrollback = settings.scrollback;
    session.fit?.fit();
  }
}

/** 建一个会话的 xterm 实例 */
async function createSession(id: string, title: string): Promise<Session | null> {
  if (!termHostRef.value) return null;

  const host = document.createElement("div");
  host.className = "term-slot";
  host.style.display = "none";
  termHostRef.value.appendChild(host);

  const settings = readTermSettings();
  const TermCtor = (window as unknown as { Terminal?: new (o: unknown) => XTermLike }).Terminal;
  if (!TermCtor) {
    errorText.value = "xterm.js 没加载成功";
    return null;
  }
  const term = new TermCtor({
    fontSize: settings.font,
    cursorBlink: settings.blink,
    cursorStyle: settings.cursorStyle,
    scrollback: settings.scrollback,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, "Cascadia Mono", monospace',
    theme: { background: "#0b1220", foreground: "#d7dde6" },
  });

  let fit: { fit(): void } | null = null;
  const FitCtor = (window as unknown as { FitAddon?: { FitAddon: new () => { fit(): void } } })
    .FitAddon;
  if (FitCtor) {
    const addon = new FitCtor.FitAddon();
    term.loadAddon(addon);
    fit = addon;
  }

  term.open(host);
  fit?.fit();

  term.onData((data) => {
    void call("hub_term_write", { id, data }).catch(() => {});
  });

  const session: Session = { id, title, term, fit, host, pendingData: [], opened: false };
  sessions.value = [...sessions.value, session];
  activeId.value = id;
  showActive();

  // 窗口尺寸变化要重新 fit
  const onResize = () => {
    fit?.fit();
    void call("hub_term_resize", {
      id,
      cols: (term as unknown as { cols?: number }).cols ?? 80,
      rows: (term as unknown as { rows?: number }).rows ?? 24,
    }).catch(() => {});
  };
  window.addEventListener("resize", onResize);

  return session;
}

function showActive() {
  for (const session of sessions.value) {
    session.host.style.display = session.id === activeId.value ? "block" : "none";
  }
  sessions.value.find((s) => s.id === activeId.value)?.term.focus();
}

/** 收到数据：opened 前先攒着，避免终端黑屏 */
function writeData(id: string, base64: string) {
  const session = sessions.value.find((s) => s.id === id);
  if (!session) return;
  const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  if (session.opened) {
    session.term.write(bytes);
  } else {
    session.pendingData.push(base64);
  }
}

function flushPending(session: Session) {
  session.opened = true;
  for (const base64 of session.pendingData) {
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    session.term.write(bytes);
  }
  session.pendingData = [];
}

onMounted(async () => {
  loadStyle("./terminal/xterm.css");
  try {
    await loadScript("./terminal/xterm.js");
    await loadScript("./terminal/addon-fit.js");
  } catch (error) {
    errorText.value = String(error);
    return;
  }

  const { listen } = await import("@tauri-apps/api/event");

  // 数据流
  unlisteners.push(
    await listen<{ id: string; data: string }>("hub-term-data", (event) => {
      writeData(event.payload.id, event.payload.data);
    }),
  );

  // 会话关闭
  unlisteners.push(
    await listen<{ id: string }>("hub-term-closed", (event) => {
      const session = sessions.value.find((s) => s.id === event.payload.id);
      if (!session) return;
      session.term.dispose();
      session.host.remove();
      sessions.value = sessions.value.filter((s) => s.id !== session.id);
      if (activeId.value === session.id) {
        activeId.value = sessions.value[0]?.id ?? "";
        showActive();
      }
    }),
  );

  // 设置热更新
  unlisteners.push(
    await listen("ab-term-settings-changed", () => applyTermSettings()),
  );

  // 从 hub 传过来的待打开服务器
  try {
    const pending = await call<string | null>("hub_term_take_pending");
    if (pending) await openServer(pending);
  } catch {
    /* 没有待打开 */
  }
});

async function openServer(serverId: string) {
  connText.value = "连接中…";
  connOk.value = false;
  try {
    const result = await call<{ id: string; title?: string }>("hub_term_open", {
      server: serverId,
    });
    const session = await createSession(result.id, result.title || "终端");
    if (!session) return;
    // open 已返回 → 把攒着的数据写进去
    flushPending(session);
    connText.value = "已连接";
    connOk.value = true;
  } catch (error) {
    connText.value = "连接失败";
    connOk.value = false;
    errorText.value = String(error);
  }
}

onUnmounted(() => {
  for (const off of unlisteners) off();
  for (const session of sessions.value) session.term.dispose();
});

function selectTab(id: string) {
  activeId.value = id;
  showActive();
}

async function closeSession(id: string) {
  try {
    await call("hub_term_close", { id });
  } catch {
    /* 关不掉就算了 */
  }
}
</script>

<template>
  <div class="term-window">
    <div class="titlebar">
      <span :class="['conn', connOk ? 'ok' : 'pending']">
        <i class="dot" />
        {{ connText }}
      </span>
      <span class="spacer" />
      <button type="button" class="btn ghost sm" @click="applyTermSettings">字号</button>
    </div>

    <div v-if="sessions.length > 1" class="tabbar">
      <button
        v-for="session in sessions"
        :key="session.id"
        type="button"
        :class="['tab', { 'is-on': session.id === activeId }]"
        @click="selectTab(session.id)"
      >
        {{ session.title }}
        <span class="x" @click.stop="closeSession(session.id)">×</span>
      </button>
    </div>

    <div ref="termHostRef" class="term-host" />

    <div v-if="!sessions.length && !errorText" class="empty">
      <p>等待连接…</p>
      <p class="hint">在「服务器」页选一台 SSH 服务器打开终端。</p>
    </div>
    <p v-if="errorText" class="err">{{ errorText }}</p>
  </div>
</template>

<style scoped>
.term-window {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background: #0b1220;
  color: #d7dde6;
}
.titlebar {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 6px 12px;
  border-bottom: 1px solid #1e293b;
  font-size: 12px;
}
.conn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.conn .dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #f59e0b;
}
.conn.ok .dot {
  background: #22c55e;
}
.spacer {
  flex: 1;
}
.tabbar {
  display: flex;
  gap: 2px;
  padding: 4px 8px;
  border-bottom: 1px solid #1e293b;
}
.tab {
  border: 0;
  background: transparent;
  color: #94a3b8;
  font-size: 12px;
  padding: 4px 10px;
  border-radius: 6px;
  cursor: pointer;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.tab.is-on {
  background: #1e293b;
  color: #e2e8f0;
}
.tab .x {
  opacity: 0.5;
}
.tab .x:hover {
  opacity: 1;
}
.term-host {
  flex: 1;
  min-height: 0;
  padding: 6px 8px;
  overflow: hidden;
}
.term-slot {
  width: 100%;
  height: 100%;
}
.empty {
  position: absolute;
  inset: 0;
  display: grid;
  place-content: center;
  text-align: center;
  color: #64748b;
  font-size: 13px;
  pointer-events: none;
}
.hint {
  font-size: 12px;
  color: #475569;
}
.err {
  position: absolute;
  bottom: 10px;
  left: 12px;
  color: #f87171;
  font-size: 12px;
  margin: 0;
}
</style>
