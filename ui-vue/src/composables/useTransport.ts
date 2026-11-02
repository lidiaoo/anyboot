/**
 * 传输通道的组合式封装。
 *
 * 真正的状态机在 `stores/uiState.ts`（因为快照和通道是同一份生命周期）；
 * 这里只提供「组件视角」的便捷接口：状态标签、手动重连、按通道能力分支。
 */

import { computed } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import {
  TRANSPORT_AUTO,
  TRANSPORT_POLL,
  TRANSPORT_PUSH,
  type TransportMode,
} from "@/api/constants";

export function useTransport() {
  const ui = useUiStateStore();

  /** 人话标签：界面上显示「推送中 / 轮询中 / 断开」 */
  const label = computed(() => {
    switch (ui.activeTransport) {
      case "ipc":
        return "本机直连";
      case "ws":
        return "推送中";
      case "http":
        return "轮询中";
      default:
        return ui.pushError || "未连接";
    }
  });

  /** 通道是否健康（界面上画小圆点用） */
  const healthy = computed(() => ui.activeTransport !== "none");

  /** 当前偏好的中文名 */
  const modeLabel = computed(() => {
    switch (ui.transportMode) {
      case TRANSPORT_PUSH:
        return "推送（WebSocket）";
      case TRANSPORT_POLL:
        return "轮询（HTTP）";
      default:
        return "自动（推送优先，失败回落轮询）";
    }
  });

  /** 是否在走推送（能吃到「PC 一改就推」的好处） */
  const isPush = computed(() => ui.activeTransport === "ws" || ui.activeTransport === "ipc");

  /** 手动重连（界面上「重试」按钮） */
  function reconnect() {
    ui.setTransportMode(ui.transportMode);
  }

  function switchTo(mode: TransportMode) {
    ui.setTransportMode(mode);
  }

  return {
    label,
    healthy,
    modeLabel,
    isPush,
    reconnect,
    switchTo,
    activeTransport: computed(() => ui.activeTransport),
    transportMode: computed(() => ui.transportMode),
    pushError: computed(() => ui.pushError),
  };
}

export { TRANSPORT_AUTO, TRANSPORT_POLL, TRANSPORT_PUSH };
