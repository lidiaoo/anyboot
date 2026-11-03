import { onMounted, onUnmounted, type Ref } from "vue";

/**
 * 按 Esc 关闭（对齐旧版 `document.addEventListener('keydown', e => {
 * if (e.key === 'Escape') close(); })` 这一类绑定）。
 *
 * 旧版 hub.html 里有 5 处独立的 Esc 处理（各个弹层各写一份）。
 * 新版做成组合式函数，弹层组件在 `open` 为 true 时调用即可。
 *
 * @param open   弹层是否打开（ref）
 * @param close  关闭回调
 * @param enabled 额外开关（默认恒为 true）
 */
export function useEscape(open: Ref<boolean>, close: () => void, enabled?: () => boolean) {
  function onKeydown(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (!open.value) return;
    if (enabled && !enabled()) return;
    // 顶层弹层才响应：如果还有别的弹层开着，让更上层的先处理
    event.stopPropagation();
    close();
  }
  onMounted(() => window.addEventListener("keydown", onKeydown));
  onUnmounted(() => window.removeEventListener("keydown", onKeydown));
}

/**
 * Ctrl/Cmd + Enter 提交（对齐旧版 `if (event.key === 'Enter' &&
 * (event.ctrlKey || event.metaKey)) submit()`）。
 */
export function useCtrlEnter(open: Ref<boolean>, submit: () => void) {
  function onKeydown(event: KeyboardEvent) {
    if (!open.value) return;
    // 输入法选词的回车不算「保存」（旧版 isImeKey：isComposing / keyCode 229）
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key !== "Enter") return;
    if (!(event.ctrlKey || event.metaKey)) return;
    event.preventDefault();
    submit();
  }
  onMounted(() => window.addEventListener("keydown", onKeydown));
  onUnmounted(() => window.removeEventListener("keydown", onKeydown));
}
