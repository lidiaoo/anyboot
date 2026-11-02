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
/** 服务器选择弹层（旧版 #pick） */
const pickOpen = ref(false);
const servers = ref<{ id: string; name: string; host: string; protocol: string }[]>([]);
/** 底部状态栏 */
const footHost = ref("—");
const footSize = ref("—");

const dark = ref(false);
try {
  dark.value = localStorage.getItem("anyboot.theme") === "dark";
} catch {
  /* 吃默认 */
}
function applyTheme(isDark: boolean) {
  const html = document.documentElement;
  html.setAttribute("data-theme", isDark ? "dark" : "light");
  html.classList.toggle("light", !isDark);
  html.classList.toggle("dark", isDark);
}
applyTheme(dark.value);

function toggleTheme() {
  dark.value = !dark.value;
  applyTheme(dark.value);
  try {
    localStorage.setItem("anyboot.theme", dark.value ? "dark" : "light");
  } catch {
    /* 忽略 */
  }
}

function openHub() {
  void call("hub_open_window", {}).catch(() => {});
}

function cycleFont() {
  const current = readTermSettings();
  const sizes = [12, 14, 16, 18, 20];
  const index = sizes.indexOf(current.fontSize);
  const next = sizes[(index + 1) % sizes.length] ?? 14;
  try {
    const raw = localStorage.getItem("ab-term-settings");
    const base = raw ? (JSON.parse(raw) as Record<string, unknown>) : {};
    localStorage.setItem("ab-term-settings", JSON.stringify({ ...base, fontSize: next }));
  } catch {
    /* 忽略 */
  }
  applyTermSettings();
}

async function pickServer(id: string) {
  pickOpen.value = false;
  await openServer(id);
}

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

/**
 * 读终端设置。**key 与字段名必须与旧版一致**（`ab-term-settings`）：
 *   fontSize / cursorBlink / copyOnSelect / bell / scrollback / downDir / fontFamily
 * 这样才能和「终端设置」页共用同一份配置。
 */
function readTermSettings(): {
  fontSize: number;
  cursorBlink: boolean;
  scrollback: number;
  fontFamily: string;
} {
  try {
    const raw = localStorage.getItem("ab-term-settings");
    if (raw) {
      const parsed = JSON.parse(raw) as Record<string, unknown>;
      return {
        fontSize: Math.min(24, Math.max(10, Number(parsed.fontSize) || 14)),
        cursorBlink: parsed.cursorBlink !== undefined ? Boolean(parsed.cursorBlink) : true,
        scrollback: Math.min(100000, Math.max(500, Number(parsed.scrollback) || 5000)),
        fontFamily: typeof parsed.fontFamily === "string" ? parsed.fontFamily : "",
      };
    }
  } catch {
    /* 吃默认 */
  }
  return { fontSize: 14, cursorBlink: true, scrollback: 5000, fontFamily: "" };
}

function applyTermSettings() {
  const settings = readTermSettings();
  for (const session of sessions.value) {
    session.term.options.fontSize = settings.fontSize;
    session.term.options.cursorBlink = settings.cursorBlink;
    session.term.options.scrollback = settings.scrollback;
    if (settings.fontFamily) {
      session.term.options.fontFamily = settings.fontFamily;
    }
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
    fontSize: settings.fontSize,
    cursorBlink: settings.cursorBlink,
    scrollback: settings.scrollback,
    fontFamily:
      settings.fontFamily || 'ui-monospace, SFMono-Regular, Menlo, "Cascadia Mono", monospace',
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

  // 服务器列表（选择弹层用）
  try {
    servers.value = (await call<{ id: string; name: string; host: string; protocol: string }[]>(
      "hub_list_servers",
    )) ?? [];
  } catch {
    servers.value = [];
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
    footHost.value = result.title || serverId;
    if (session) {
      const cols = (session.term as unknown as { cols?: number }).cols ?? 80;
      const rows = (session.term as unknown as { rows?: number }).rows ?? 24;
      footSize.value = `${cols} × ${rows}`;
    }
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
  <header>
    <span class="appicon" aria-hidden="true">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <rect x="2" y="4" width="20" height="14" rx="2" />
        <path d="M6 9l3 3-3 3" />
      </svg>
    </span>
    <h1>终端</h1>
    <span v-if="connText !== '—'" class="pill" :class="connOk ? 'ok' : 'bad'">
      <span class="dot" />
      <span>{{ connText }}</span>
    </span>
    <div class="tools">
      <button type="button" class="toolbtn" aria-label="打开远程管理" title="打开远程管理" @click="openHub">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect x="2" y="3" width="20" height="7" rx="2" />
          <rect x="2" y="14" width="20" height="7" rx="2" />
          <path d="M6 6.5h.01M6 17.5h.01" />
        </svg>
        远程管理
      </button>
      <button type="button" class="toolbtn" aria-label="切换主题" @click="toggleTheme">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 3a9 9 0 0 1 0 18z" fill="currentColor" stroke="none" />
        </svg>
        主题
      </button>
      <button type="button" class="toolbtn" aria-label="调整字号" title="切换字号" @click="cycleFont">
        <span class="a">A</span><span class="b">A</span>
      </button>
    </div>
  </header>

  <div class="tabbar" role="tablist" aria-label="终端会话">
    <button
      v-for="session in sessions"
      :key="session.id"
      type="button"
      role="tab"
      :class="{ 'is-on': session.id === activeId }"
      @click="selectTab(session.id)"
    >
      {{ session.title }}
      <span class="x" @click.stop="closeSession(session.id)">×</span>
    </button>
    <button type="button" class="tabbar-add" aria-label="新建会话" @click="pickOpen = true">＋</button>
  </div>

  <div class="term-wrap">
    <div ref="termHostRef" />
    <div class="crt-fade" aria-hidden="true" />
    <div v-if="!sessions.length" class="empty">
      <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
        <rect x="2" y="4" width="20" height="14" rx="2" />
        <path d="M6 9l3 3-3 3" />
        <path d="M12 15h6" />
      </svg>
      <div>还没有会话 —— 点上方 <b>＋</b> 选择一台服务器</div>
    </div>
  </div>

  <footer>
    <span><span class="dot" /><span>{{ footHost }}</span></span>
    <span aria-hidden="true">·</span><span>xterm-256color</span>
    <span aria-hidden="true">·</span><span>{{ footSize }}</span>
    <span class="lat"><span class="dot" />延迟 <span>—</span></span>
  </footer>

  <!-- 服务器选择弹层（旧版 #pick） -->
  <div v-if="pickOpen" class="modal" @click.self="pickOpen = false">
    <div class="modal-card" role="dialog" aria-modal="true">
      <div class="modal-head">
        <h3>选择要连接的服务器</h3>
        <button type="button" aria-label="关闭" @click="pickOpen = false">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>
      <div class="modal-body">
        <p v-if="!servers.length" class="hint">还没有 SSH 服务器 —— 到「服务器」页添加一台</p>
        <button
          v-for="s in servers"
          :key="s.id"
          type="button"
          class="btn"
          style="width: 100%; justify-content: flex-start; margin-bottom: 6px"
          @click="pickServer(s.id)"
        >
          {{ s.name }} · {{ s.host }}
        </button>
      </div>
    </div>
  </div>

  <p v-if="errorText" class="hint" style="color: rgb(var(--bad)); padding: 0 12px">{{ errorText }}</p>
</template>

<!--
  样式**不写在这里**：`.appicon` / `.tools` / `.toolbtn` / `.tabbar` /
  `.term-wrap` / `.empty` / `footer` / `.modal` 全部由
  `styles/vendor/legacy.css` + `hub-inline.css` 定义（旧版就是这么走的）。
-->
