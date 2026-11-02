import { defineStore } from "pinia";
import { ref } from "vue";

/**
 * 全局 toast（旧版 `toast(msg)` 是每个 hvue 页都能调的全局函数）。
 *
 * 本项目的各路由页（ServersPage / TermPage / KeysPage …）没有自己的
 * toast DOM，旧版却统一调 window.toast —— 这里做成 Pinia store，
 * 由 HubApp 渲染一个 `#toast` 元素，所有页面共享。
 *
 * 语义对齐旧版：默认 2.2 秒淡出；`bad` 变体用于失败提示（红字）。
 */
export interface ToastOptions {
  /** 毫秒；0 = 用默认 2200 */
  duration?: number;
  /** 失败提示（红底/红字） */
  bad?: boolean;
}

export const useToastStore = defineStore("toast", () => {
  const text = ref("");
  const bad = ref(false);
  let timer: number | undefined;

  function show(msg: string, opts: ToastOptions | string = {}) {
    // 兼容旧版 toast(msg, 'bad') 的两参写法
    const o: ToastOptions = typeof opts === "string" ? { bad: opts === "bad" } : opts;
    text.value = String(msg);
    bad.value = Boolean(o.bad);
    if (timer) window.clearTimeout(timer);
    const ms = o.duration && o.duration > 0 ? o.duration : 2200;
    timer = window.setTimeout(() => {
      text.value = "";
    }, ms);
  }

  function clear() {
    text.value = "";
  }

  return { text, bad, show, clear };
});
