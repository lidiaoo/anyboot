/**
 * 快照 store：持有唯一一份 `UiState`，以及传输通道（IPC / WebSocket / HTTP 轮询）。
 *
 * ## 三种来源，一套数据
 *
 * | 场景 | 来源 |
 * |---|---|
 * | Tauri 窗口（PC 主窗 / hub / 独立窗） | `listen("ui-state-changed")` 事件推送 |
 * | 浏览器 / 手机「电脑页」 | `/api/ws` WebSocket 推送 |
 * | 上面两条断了 | `/api/ui-state` HTTP 轮询兜底 |
 *
 * **优先级**：IPC 事件 > WebSocket > 轮询。同一份快照可能从多路到达，
 * 用 `revision` 去重（只接受比当前更新的）。
 *
 * ## 为什么不用「每次覆盖」
 *
 * 旧实现每拍整体替换 DOM，导致滚动位置丢失、列表闪烁。这里快照是
 * **细粒度响应式**的：组件用 `computed` 读自己关心的那一段，Vue 只更新
 * 真正变化的部分。
 */

import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import type { UiState } from "@/api/types";
import {
  DATA_REFRESH_DEFAULT_SEC,
  DATA_REFRESH_KEY,
  TRANSPORT_AUTO,
  TRANSPORT_KEY,
  TRANSPORT_POLL,
  TRANSPORT_PUSH,
  UI_STATE_CHANGED_EVENT,
  type TransportMode,
} from "@/api/constants";

/** 当前实际生效的通道 */
export type ActiveTransport = "none" | "ipc" | "ws" | "http";

function readTransportPreference(): TransportMode {
  // URL 的 ?transport= 优先（手机 App 通过它指定）
  try {
    const fromUrl = (new URLSearchParams(location.search).get("transport") || "").toLowerCase();
    if (fromUrl === TRANSPORT_POLL || fromUrl === TRANSPORT_PUSH || fromUrl === TRANSPORT_AUTO) {
      localStorage.setItem(TRANSPORT_KEY, fromUrl);
      return fromUrl;
    }
  } catch {
    /* location / localStorage 不可用就吃默认 */
  }
  try {
    const saved = (localStorage.getItem(TRANSPORT_KEY) || "").toLowerCase();
    if (saved === TRANSPORT_POLL || saved === TRANSPORT_PUSH || saved === TRANSPORT_AUTO) {
      return saved;
    }
  } catch {
    /* 同上 */
  }
  return TRANSPORT_AUTO;
}

/** 读「数据刷新」间隔（秒）；0 = 关闭 */
export function readDataRefreshSec(): number {
  try {
    const raw = localStorage.getItem(DATA_REFRESH_KEY);
    if (raw !== null) {
      const sec = Number.parseInt(raw, 10);
      if (Number.isFinite(sec) && sec >= 0) return sec;
    }
  } catch {
    /* 吃默认 */
  }
  return DATA_REFRESH_DEFAULT_SEC;
}

export const useUiStateStore = defineStore("uiState", () => {
  // ---- 快照 --------------------------------------------------------------
  const state = shallowRef<UiState | null>(null);
  const revision = ref(0);

  // ---- 传输 --------------------------------------------------------------
  const transportMode = ref<TransportMode>(readTransportPreference());
  const activeTransport = ref<ActiveTransport>("none");
  /** 推送通道的最近一次错误（界面上提示用） */
  const pushError = ref("");
  const remoteTokenMissing = ref(false);
  /** 全局「底部回执条」文案（引导页各「重新读取」等操作写这里 → BootApp 渲染为 #status-bar，
   *  与 GRUB 手动识别同一条浮窗）。error=true 显示红字。 */
  const bootNotice = ref<{ text: string; error: boolean }>({ text: "", error: false });
  function setBootNotice(text: string, error = false): void {
    bootNotice.value = { text, error };
  }
  function clearBootNotice(): void {
    bootNotice.value = { text: "", error: false };
  }

  let socket: WebSocket | null = null;
  let pushRetryTimer: number | null = null;
  let pushRetryDelay = 1000;
  let pollTimer: number | null = null;
  let ipcUnlisten: (() => void) | null = null;
  let started = false;

  // ---- 快照写入（唯一入口）----------------------------------------------
  /**
   * 接收一份快照。
   *
   * `revision` 只增不减 —— 比当前旧的直接丢弃（多路来源会重复到达）。
   * `force` 用于首次加载（此时本地 revision 可能比服务端大，比如 App 重启后）。
   */
  /**
   * 首帧渲染完成标记。
   *
   * `styles/vendor/legacy.css` 里有一条历史规则：
   *     body { opacity: 0 }              ← 首帧前不可见（防「闪一下旧页面」）
   *     html[data-ready] body { opacity: 1 }
   * 旧 `app.js` 在首份快照渲染完之后给 `<html data-ready>`。
   *
   * **Vue 版必须自己补这一步**：否则 body 永远 opacity:0，窗口一片黑，
   * 而 JS 其实跑得好好的（DOM 都在、控制台无异常）—— 极难排查。
   */
  let readyMarked = false;
  function markReady() {
    if (readyMarked) return;
    readyMarked = true;
    // 等一帧再显示，避免首次渲染的中间态被看到
    requestAnimationFrame(() => {
      document.documentElement.setAttribute("data-ready", "1");
    });
  }

  /**
   * 兜底：无论快照有没有到，**最多等 800ms 就显示**。
   *
   * 只靠「首份快照到达才显示」在异常路径下会永久黑屏 —— 比如没 token、
   * 服务不可达、IPC 失败。宁可先让用户看到界面（哪怕是空态），
   * 也不要一片黑、什么都没有。
   */
  function scheduleReadyFallback() {
    window.setTimeout(markReady, 800);
  }

  function applySnapshot(next: UiState | null | undefined, options: { force?: boolean } = {}) {
    if (!next || typeof next !== "object") return;
    const nextRevision = typeof next.revision === "number" ? next.revision : 0;
    if (!options.force && nextRevision && nextRevision <= revision.value) return;
    state.value = next;
    revision.value = nextRevision;
    markReady();
  }

  // ---- IPC（Tauri 窗口）------------------------------------------------
  async function startIpc(): Promise<boolean> {
    const { isTauri } = await import("@/api/tauri");
    if (!isTauri()) return false;
    try {
      const { listen } = await import("@tauri-apps/api/event");
      ipcUnlisten = await listen<UiState>(UI_STATE_CHANGED_EVENT, (event) => {
        applySnapshot(event.payload);
      });
      const { getUiState } = await import("@/api/tauri");
      applySnapshot(await getUiState<UiState>(), { force: true });
      setActive("ipc");
      return true;
    } catch (error) {
      pushError.value = `IPC 订阅失败：${String(error)}`;
      return false;
    }
  }

  function stopIpc() {
    if (ipcUnlisten) {
      ipcUnlisten();
      ipcUnlisten = null;
    }
  }

  // ---- WebSocket 推送 --------------------------------------------------
  function connectPush() {
    closeSocket();
    if (!canUseHttp()) return;

    void import("@/api/http").then(async ({ wsUrl, getAuthToken }) => {
      const token = getAuthToken();
      if (!token) {
        remoteTokenMissing.value = true;
        setActive("none");
        return;
      }
      remoteTokenMissing.value = false;

      let url: string;
      try {
        url = wsUrl(token);
      } catch (error) {
        pushError.value = String(error);
        return;
      }

      const ws = new WebSocket(url);
      socket = ws;

      ws.onopen = () => {
        pushRetryDelay = 1000;
        pushError.value = "";
        setActive("ws");
      };

      ws.onmessage = (event) => {
        try {
          const payload = JSON.parse(event.data as string) as UiState;
          applySnapshot(payload);
        } catch {
          /* 非 JSON（心跳等）忽略 */
        }
      };

      ws.onclose = () => {
        if (socket !== ws) return; // 主动关闭
        socket = null;
        onPushUnavailable();
      };

      ws.onerror = () => {
        pushError.value = "推送通道出错";
      };
    });
  }

  function closeSocket() {
    if (socket) {
      const dying = socket;
      socket = null;
      try {
        dying.close();
      } catch {
        /* 已经断了就算了 */
      }
    }
  }

  /** 推送断了：自动模式回落轮询并继续后台重连；显式选了推送则只重连 */
  function onPushUnavailable() {
    if (transportMode.value === TRANSPORT_AUTO) {
      startPolling();
      setActive("http");
    } else {
      setActive("none");
      pushError.value = "推送通道已断开，正在重连…";
    }
    schedulePushRetry();
  }

  function schedulePushRetry() {
    if (pushRetryTimer !== null || !canUseHttp()) return;
    pushRetryTimer = window.setTimeout(() => {
      pushRetryTimer = null;
      connectPush();
    }, pushRetryDelay);
    pushRetryDelay = Math.min(pushRetryDelay * 2, 30000);
  }

  // ---- HTTP 轮询 -------------------------------------------------------
  function startPolling() {
    stopPolling();
    const sec = readDataRefreshSec();
    if (sec <= 0) return; // 用户关了数据刷新
    pollTimer = window.setInterval(() => {
      if (document.hidden) return;
      void pollOnce();
    }, sec * 1000);
  }

  function stopPolling() {
    if (pollTimer !== null) {
      window.clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  async function pollOnce() {
    // 桌面端（Tauri）没有 HTTP token，必须走 IPC 的 get_ui_state；
    // 旧实现无条件走 HTTP fetchUiState + token 检查，桌面端因「无 token」
    // 直接 return —— 于是 pollOnce 在桌面端是空转，装/卸完免密后界面不变
    // （用户报「点完还是不变，虽然提示成功」）。
    try {
      const { isTauri } = await import("@/api/tauri");
      if (isTauri()) {
        const { getUiState } = await import("@/api/tauri");
        applySnapshot(await getUiState<UiState>(), { force: true });
        return;
      }
      const { fetchUiState, getAuthToken } = await import("@/api/http");
      if (!getAuthToken()) {
        // 浏览器/手机端无 token：标记缺 token → 触发输入框（旧版就在此时弹 token 输入）
        remoteTokenMissing.value = true;
        return;
      }
      applySnapshot(await fetchUiState(), { force: true });
    } catch (error) {
      if (error instanceof (await import("@/api/http")).AuthError) {
        remoteTokenMissing.value = true;
      }
      // 断线时静默，下一拍再试
    }
  }

  // ---- 状态机 ----------------------------------------------------------
  function setActive(next: ActiveTransport) {
    if (activeTransport.value !== next) activeTransport.value = next;
  }

  function canUseHttp(): boolean {
    // 浏览器 / WebView 里 location.host 就是 App 的 HTTP 服务；
    // Tauri 的 tauri://localhost 下也能打同源 /api（由 Tauri 自己转发），
    // 但优先走 IPC，所以这里只判断「有没有 host」。
    return Boolean(location.host);
  }

  // ---- 启动 / 停止 -----------------------------------------------------
  async function start() {
    if (started) return;
    started = true;
    scheduleReadyFallback();   // 兜底：800ms 后一定显示，避免永久黑屏

    // 1) Tauri 窗口：IPC 事件推送（最可靠）
    if (await startIpc()) return;

    // 2) 浏览器/WebView：推送优先，失败回落轮询
    if (transportMode.value === TRANSPORT_POLL) {
      startPolling();
      setActive("http");
      await pollOnce();
      return;
    }
    connectPush();
    await pollOnce(); // 先抓一份，别让界面空着
  }

  function stop() {
    started = false;
    stopIpc();
    closeSocket();
    stopPolling();
    if (pushRetryTimer !== null) {
      window.clearTimeout(pushRetryTimer);
      pushRetryTimer = null;
    }
    setActive("none");
  }

  /** 切换传输方式（记住偏好并重建通道） */
  function setTransportMode(mode: TransportMode) {
    transportMode.value = mode;
    try {
      localStorage.setItem(TRANSPORT_KEY, mode);
    } catch {
      /* 存不了就本次会话有效 */
    }
    closeSocket();
    stopPolling();
    if (transportMode.value === TRANSPORT_POLL) {
      startPolling();
      setActive("http");
      return;
    }
    setActive("none");
    connectPush();
  }

  // ---- 便捷派生 --------------------------------------------------------
  const ready = computed(() => state.value !== null);
  const status = computed(() => state.value?.status ?? null);
  const settings = computed(() => state.value?.settings ?? null);
  const grub = computed(() => state.value?.grub ?? null);
  const channels = computed(() => state.value?.channels ?? []);
  const refindSim = computed(() => state.value?.refindSim ?? null);
  const linuxGrub = computed(() => state.value?.linuxGrub ?? null);
  /** Linux 免密组件状态（来自快照，随 publish_ui 实时更新） */
  const linuxPriv = computed(() => state.value?.linuxPriv ?? null);
  const busy = computed(() => state.value?.busy ?? null);
  const http = computed(() => state.value?.http ?? null);

  return {
    // 数据
    state,
    revision,
    ready,
    status,
    settings,
    grub,
    channels,
    refindSim,
    linuxGrub,
    linuxPriv,
    busy,
    http,
    // 传输
    transportMode,
    activeTransport,
    pushError,
    remoteTokenMissing,
    bootNotice,
    setBootNotice,
    clearBootNotice,
    // 动作
    applySnapshot,
    start,
    stop,
    setTransportMode,
    startPolling,
    stopPolling,
    pollOnce,
  };
});
