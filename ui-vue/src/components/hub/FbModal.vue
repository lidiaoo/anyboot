<script setup lang="ts">
/**
 * 文件窗弹层（对齐旧版 `#fbMkdirModal` / `#fbRenameModal` / `#fbDelModal` /
 * `#fbInfoModal` / `#fbDestModal` 这一批）。
 *
 * ## 旧版结构（5 个弹层完全同构，只有标题/正文/按钮不同）
 * ```
 * .fb-modal[role=dialog]
 *   .fb-modal-card
 *     .fb-modal-head > h3 + button(✕ svg)
 *     .fb-modal-body > label + input   （或自定义内容）
 *     .fb-modal-foot > .fb-btn.fb-plain 取消 + .fb-btn.fb-primary 确认
 * ```
 *
 * 所以做成一个组件，通过 slot 放不同正文。
 */

withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    confirmText?: string;
    cancelText?: string;
    /** 确认按钮是否危险样式 */
    danger?: boolean;
    /** 禁用确认（比如必填项为空） */
    confirmDisabled?: boolean;
  }>(),
  {
    confirmText: "确定",
    cancelText: "取消",
    danger: false,
    confirmDisabled: false,
  },
);

const emit = defineEmits<{
  (e: "confirm"): void;
  (e: "cancel"): void;
}>();
</script>

<template>
  <div v-if="open" class="fb-modal" role="dialog" aria-modal="true" :aria-label="title">
    <div class="fb-modal-card">
      <div class="fb-modal-head">
        <h3>{{ title }}</h3>
        <button type="button" aria-label="关闭" @click="emit('cancel')">
          <svg
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
          >
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>

      <div class="fb-modal-body">
        <slot />
      </div>

      <div class="fb-modal-foot">
        <button type="button" class="fb-btn fb-plain" @click="emit('cancel')">{{ cancelText }}</button>
        <button
          type="button"
          :class="['fb-btn', danger ? 'danger' : 'fb-primary']"
          :disabled="confirmDisabled"
          @click="emit('confirm')"
        >
          {{ confirmText }}
        </button>
      </div>
    </div>
  </div>
</template>
