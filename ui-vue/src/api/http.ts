/**
 * `/api/*` 的 HTTP 封装。
 *
 * **用途**：浏览器 / 手机端「电脑页」走这条路（WebView 加载的是 App 的 HTTP 服务）。
 * Tauri 窗口里虽然也能打本机 HTTP，但优先走 IPC（更快、不用经过网络栈）。
 *
 * **鉴权**：所有请求带 `X-Auth-Token` 头；`?token=` 查询参数也支持（浏览器里
 * 没法定制头时用，见 `src/constants` 的 URL 解析）。
 *
 * **缓存**：一律 `cache: "no-store"` —— 快照必须是最新的，浏览器缓存会让人
 * 看到过时数据。
 */

import type { UiState } from "./types";

export const API_BASE = "";

export class AuthError extends Error {
  constructor(message = "访问令牌无效（401）") {
    super(message);
    this.name = "AuthError";
  }
}

export class HttpError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
    this.name = "HttpError";
  }
}

let authToken = "";

export function setAuthToken(token: string) {
  authToken = token.trim();
}

export function getAuthToken(): string {
  return authToken;
}

async function request(path: string, init: RequestInit = {}): Promise<Response> {
  const headers = new Headers(init.headers);
  if (authToken) headers.set("X-Auth-Token", authToken);

  const response = await fetch(path, {
    ...init,
    headers,
    cache: "no-store",
  });

  if (response.status === 401) throw new AuthError();
  if (!response.ok) {
    throw new HttpError(`${path} → ${response.status}`, response.status);
  }
  return response;
}

async function requestJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await request(path, init);
  return (await response.json()) as T;
}

// ---------------------------------------------------------------------------
// 快照
// ---------------------------------------------------------------------------

/** 完整快照（电脑页 2 秒轮询打的就是它） */
export const fetchUiState = () => requestJson<UiState>("/api/ui-state");

// ---------------------------------------------------------------------------
// 引导器相关（浏览器/手机端读用）
// ---------------------------------------------------------------------------

export const fetchStatus = () => requestJson<unknown>("/api/status");
export const fetchGrub = () => requestJson<unknown>("/api/grub");
export const fetchBootChannels = () => requestJson<unknown>("/api/boot-channels");

// ---------------------------------------------------------------------------
// 健康检查（探测一个地址是否在线：TCP + /health）
// ---------------------------------------------------------------------------

export const fetchHealth = () => requestJson<unknown>("/health");

// ---------------------------------------------------------------------------
// 命令通道（浏览器里改设置用；Tauri 窗口走 IPC）
// ---------------------------------------------------------------------------

export const postCommand = <T>(command: string, payload: Record<string, unknown> = {}) =>
  requestJson<T>("/api/ui-command", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ command, ...payload }),
  });

// ---------------------------------------------------------------------------
// WebSocket 推送通道
// ---------------------------------------------------------------------------

/** 推送路径（与 `src-tauri/src/http.rs` 的 `WS_PATH` 一致） */
export const WS_PATH = "/api/ws";

/** 拼出带 token 的 ws URL（WebSocket 握手喝不进自定义头，只能走查询参数） */
export function wsUrl(token: string): string {
  const protocol = location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${location.host}${WS_PATH}?token=${encodeURIComponent(token)}`;
}
