/**
 * 前端常量。
 *
 * 与 Rust / 旧前端保持一致的那些「魔法值」集中在这里，避免散落各处。
 */

/** 快照推送事件名（`src-tauri/src/lib.rs:320` 的 `UI_STATE_CHANGED_EVENT`） */
export const UI_STATE_CHANGED_EVENT = "ui-state-changed";

// ---------------------------------------------------------------------------
// 传输方式
// ---------------------------------------------------------------------------

/** 自动：先试推送，失败回落轮询 */
export const TRANSPORT_AUTO = "auto";
/** 推送：WebSocket */
export const TRANSPORT_PUSH = "ws";
/** 轮询：HTTP */
export const TRANSPORT_POLL = "http";

export type TransportMode =
  | typeof TRANSPORT_AUTO
  | typeof TRANSPORT_PUSH
  | typeof TRANSPORT_POLL;

/** 传输方式偏好的 localStorage key（与旧前端同 key，迁移后读得到用户的旧设置） */
export const TRANSPORT_KEY = "anyboot.transport";

// ---------------------------------------------------------------------------
// 数据刷新（2026-11-02 用户定案：「数据刷新」与「连不上时自动重载」是两件事）
// ---------------------------------------------------------------------------

/** 数据刷新间隔（秒）；0 = 关闭。默认 2 秒（与电脑页轮询一致） */
export const DATA_REFRESH_KEY = "pcDataRefreshSec";
export const DATA_REFRESH_DEFAULT_SEC = 2;

/** 刷新时是否每次重新探测地址：true = 每次重探；false = 复用上次端点（默认） */
export const REPROBE_ALWAYS_KEY = "pcReprobeMode"; // 旧版键（值 reuse/fresh）
export const REPROBE_ALWAYS_DEFAULT = false;

/** 连续失败多少次才判离线（弱网容忍）；默认 3 */
export const OFFLINE_RETRY_KEY = "offlineRetryCount";
export const OFFLINE_RETRY_DEFAULT = 3;

// ---------------------------------------------------------------------------
// 连不上时自动重载（与数据刷新独立）
// ---------------------------------------------------------------------------

/** 失败重试间隔（秒）；0 = 永不 */
export const FAIL_RELOAD_KEY = "pcAutoReloadSec"; // 旧版键
export const FAIL_RELOAD_DEFAULT_SEC = 5;

// ---------------------------------------------------------------------------
// 引导页主题（`boot-adv` 的持久化 key）
// ---------------------------------------------------------------------------

export const BOOT_ADV_KEY = "anyboot.boot-adv";

// ---------------------------------------------------------------------------
// WebSocket 重连退避
// ---------------------------------------------------------------------------

export const PUSH_RETRY_BASE_MS = 1000;
export const PUSH_RETRY_MAX_MS = 30000;

// ---------------------------------------------------------------------------
// 响应式断点（与旧 `applyVwClasses` 的三档一致）
// ---------------------------------------------------------------------------

export const VW_900 = 900;
export const VW_760 = 760;
export const VW_520 = 520;
