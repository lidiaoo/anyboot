/**
 * 设置 store：把「后端设置（`UiState.settings`）」与「前端本地偏好
 * （localStorage）」分开管。
 *
 * ## 为什么分两块
 *
 * | 类型 | 存在哪 | 例子 |
 * |---|---|---|
 * | **后端设置** | Rust 的 `settings.json` | `refindMode`、`grubMode`、`httpPort`、`theme` |
 * | **前端偏好** | 浏览器 localStorage | 数据刷新间隔、失败重载间隔、传输方式、引导页主题布局 |
 *
 * 前端偏好**不进 Rust**：它们只影响本机这个 WebView 的刷新节奏，跟「这台电脑是怎么配的」无关。
 * 手机端和 PC 端各存各的，互不干扰。
 */

import { defineStore } from "pinia";
import { computed, ref } from "vue";
import {
  DATA_REFRESH_DEFAULT_SEC,
  DATA_REFRESH_KEY,
  FAIL_RELOAD_DEFAULT_SEC,
  FAIL_RELOAD_KEY,
  OFFLINE_RETRY_DEFAULT,
  OFFLINE_RETRY_KEY,
  REPROBE_ALWAYS_KEY,
} from "@/api/constants";
import { useUiStateStore } from "./uiState";

function readNumber(key: string, fallback: number, min = 0): number {
  try {
    const raw = localStorage.getItem(key);
    if (raw !== null) {
      const value = Number.parseInt(raw, 10);
      if (Number.isFinite(value) && value >= min) return value;
    }
  } catch {
    /* 吃默认 */
  }
  return fallback;
}


function writeValue(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* 存不了就本次会话有效 */
  }
}

export const useSettingsStore = defineStore("settings", () => {
  const ui = useUiStateStore();

  // ---- 后端设置（只读派生；写要靠 IPC / HTTP 命令）--------------------
  /** rEFInd 读取方式：auto / manual */
  const refindMode = computed(() => ui.settings?.refindMode ?? "auto");
  /** GRUB 读取方式：auto / manual */
  const grubMode = computed(() => ui.settings?.grubMode ?? "auto");
  const theme = computed(() => ui.settings?.theme ?? "light");
  /** 强调色（旧版 applyAccent 的 accent） */
  const accentColor = computed(() => ui.settings?.accentColor ?? "#6366f1");
  /** GRUB 条目文字格式（极简 / 完整 …） */
  const grubListFormat = computed(() => ui.settings?.grubListFormat ?? "minimal");

  // ---- 前端偏好：数据刷新 ---------------------------------------------
  const dataRefreshSec = ref(readNumber(DATA_REFRESH_KEY, DATA_REFRESH_DEFAULT_SEC));
  // 旧版键 pcReprobeMode 的值是 reuse/fresh（reuse = 复用上次成功地址，默认）
  const reprobeAlways = ref(
    (localStorage.getItem(REPROBE_ALWAYS_KEY) || "reuse") === "fresh",
  );
  /** 连续失败多少次才判离线（弱网容忍） */
  const offlineRetryCount = ref(readNumber(OFFLINE_RETRY_KEY, OFFLINE_RETRY_DEFAULT, 1));

  // ---- 前端偏好：失败重试 ---------------------------------------------
  const failReloadSec = ref(readNumber(FAIL_RELOAD_KEY, FAIL_RELOAD_DEFAULT_SEC));

  function setDataRefreshSec(sec: number) {
    dataRefreshSec.value = Math.max(0, Math.floor(sec));
    writeValue(DATA_REFRESH_KEY, String(dataRefreshSec.value));
    // 间隔变了要重建轮询定时器（否则旧间隔会一直跑到刷新页面）
    if (ui.activeTransport === "http") {
      ui.stopPolling();
      ui.startPolling();
    }
  }

  function setReprobeAlways(value: boolean) {
    reprobeAlways.value = value;
    writeValue(REPROBE_ALWAYS_KEY, value ? "fresh" : "reuse");
  }

  function setOfflineRetryCount(value: number) {
    offlineRetryCount.value = Math.max(1, Math.floor(value));
    writeValue(OFFLINE_RETRY_KEY, String(offlineRetryCount.value));
  }

  function setFailReloadSec(sec: number) {
    failReloadSec.value = Math.max(0, Math.floor(sec));
    writeValue(FAIL_RELOAD_KEY, String(failReloadSec.value));
  }

  return {
    // 后端
    refindMode,
    grubMode,
    theme,
    accentColor,
    grubListFormat,
    // 前端偏好
    dataRefreshSec,
    reprobeAlways,
    offlineRetryCount,
    failReloadSec,
    // 动作
    setDataRefreshSec,
    setReprobeAlways,
    setOfflineRetryCount,
    setFailReloadSec,
  };
});
