<script setup lang="ts">
import { computed } from "vue";
import { t } from "@/i18n";
import { useEscape } from "@/composables/useEscape";
/**
 * 日志弹层（对齐旧版 `#gwLogOverlay` / `#tsLogOverlay` / `#tsnLogOverlay`，
 * 三个完全同构，只有标题不同）。
 *
 * ## 旧版结构
 * ```
 * .token-modal-overlay > .token-modal.gw-log-modal
 *   h2                      标题
 *   pre.gw-log              日志正文（等宽、可滚）
 *   div > 复制全部 / 清空 / 关闭（右对齐）
 * ```
 */

const props = defineProps<{
  open: boolean;
  title: string;
  lines: string[];
  /**
   * 旧版三个日志弹层是**各自独立的 DOM**（`#gwLogOverlay` / `#tsLogOverlay` /
   * `#tsnLogOverlay`），各有 `<prefix>Body/Copy/Clear/Close` 四个 id。
   * 这里用一个前缀参数区分。
   */
  idPrefix: string;
  /** 按钮 id 的后缀风格：'plain' → Copy/Clear/Close；'modal' → ModalCopy/ModalClear/ModalClose */
  buttonStyle?: "plain" | "modal";
}>();

useEscape(computed(() => props.open), () => emit("close"));
const emit = defineEmits<{
  (e: "copy", text: string): void;
  (e: "clear"): void;
  (e: "close"): void;
}>();
</script>

<template>
  <Teleport to="body">
    <div v-if="open" :id="`${idPrefix}Overlay`" class="token-modal-overlay" @click.self="emit('close')">
      <div class="token-modal gw-log-modal">
        <h2>{{ title }}</h2>
        <pre :id="`${idPrefix}Body`" class="gw-log">{{ lines.join("\n") || t("log.empty") }}</pre>
        <div style="display: flex; gap: 8px; justify-content: flex-end; margin-top: 12px">
          <button type="button" :id="`${idPrefix}${buttonStyle === 'modal' ? 'ModalCopy' : 'Copy'}`" class="btn" @click="emit('copy', lines.join('\n'))">
            {{ t("log.copyAll") }}
          </button>
          <button type="button" :id="`${idPrefix}${buttonStyle === 'modal' ? 'ModalClear' : 'Clear'}`" class="btn" @click="emit('clear')">{{ t("log.clear") }}</button>
          <button type="button" :id="`${idPrefix}${buttonStyle === 'modal' ? 'ModalClose' : 'Close'}`" class="btn primary" @click="emit('close')">{{ t("common.close") }}</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>
