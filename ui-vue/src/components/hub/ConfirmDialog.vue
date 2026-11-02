<script setup lang="ts">
/**
 * 通用确认弹窗（对齐旧版 `#deleteOverlay` 等一批 `.overlay > .dialog`）。
 *
 * ```
 * .overlay > .dialog
 *   .dialog-head > h3 + button.icon-btn ✕
 *   .dialog-body > p.d-text
 *   .dialog-foot > .spacer + 取消 + 确认
 * ```
 *
 * 旧版有一批同构弹窗（删除服务器 / 删除电脑 / 各种二次确认），
 * 它们结构完全一样，只是标题、正文、按钮文案不同 —— 所以做成一个组件。
 */

withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    /** 正文；`danger` 为 true 时确认按钮是红色 */
    message?: string;
    confirmText?: string;
    cancelText?: string;
    danger?: boolean;
  }>(),
  {
    message: "",
    confirmText: "确定",
    cancelText: "取消",
    danger: false,
  },
);

const emit = defineEmits<{
  (e: "confirm"): void;
  (e: "cancel"): void;
}>();
</script>

<template>
  <div v-if="open" class="overlay">
    <div class="dialog" role="dialog" aria-modal="true" :aria-label="title">
      <div class="dialog-head">
        <h3>{{ title }}</h3>
        <button type="button" class="icon-btn" aria-label="关闭" @click="emit('cancel')">✕</button>
      </div>
      <div class="dialog-body">
        <p class="d-text" style="margin: 0; font-size: 13px">{{ message }}</p>
        <slot />
      </div>
      <div class="dialog-foot">
        <span class="spacer" />
        <button type="button" class="btn ghost" @click="emit('cancel')">{{ cancelText }}</button>
        <button :class="['btn', danger ? 'danger' : 'primary']" @click="emit('confirm')">
          {{ confirmText }}
        </button>
      </div>
    </div>
  </div>
</template>
