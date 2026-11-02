<script setup lang="ts">
import { ref, watch } from "vue";

/**
 * 终端设置页（原 `ui/hub.html` 的 `#page-termset`）。
 *
 * 全部存 localStorage，保存后**对所有终端会话即时生效**（含已开的窗口）——
 * 独立终端窗监听 `storage` 事件热更新。
 */

const STORE_KEY = "anyboot.term-set";

interface TermSettings {
  font: number;
  blink: boolean;
  cursorStyle: string;
  scrollback: number;
  copyOnSelect: boolean;
}

const DEFAULTS: TermSettings = {
  font: 14,
  blink: true,
  cursorStyle: "block",
  scrollback: 5000,
  copyOnSelect: false,
};

function load(): TermSettings {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    if (raw) return { ...DEFAULTS, ...(JSON.parse(raw) as Partial<TermSettings>) };
  } catch {
    /* 吃默认 */
  }
  return { ...DEFAULTS };
}

const settings = ref<TermSettings>(load());
const saved = ref(false);

watch(
  settings,
  (value) => {
    try {
      localStorage.setItem(STORE_KEY, JSON.stringify(value));
      saved.value = true;
      window.setTimeout(() => (saved.value = false), 1200);
    } catch {
      /* 存不了就算了 */
    }
  },
  { deep: true },
);
</script>

<template>
  <div class="ts-page">
    <div class="ts-card">
      <div class="ts-row">
        <div>
          <p class="ts-t">字号</p>
          <p class="ts-s">终端字体大小</p>
        </div>
        <span class="ts-val">{{ settings.font }} pt</span>
      </div>
      <input v-model.number="settings.font" class="ts-range" type="range" min="10" max="24" step="1" />
    </div>

    <div class="ts-card">
      <label class="ts-row">
        <div>
          <p class="ts-t">光标闪烁</p>
          <p class="ts-s">终端光标呼吸闪烁</p>
        </div>
        <input v-model="settings.blink" type="checkbox" />
      </label>
    </div>

    <div class="ts-card">
      <div class="ts-row">
        <div>
          <p class="ts-t">光标样式</p>
          <p class="ts-s">方块 / 竖线 / 下划线</p>
        </div>
        <select v-model="settings.cursorStyle" class="ts-select">
          <option value="block">方块</option>
          <option value="bar">竖线</option>
          <option value="underline">下划线</option>
        </select>
      </div>
    </div>

    <div class="ts-card">
      <div class="ts-row">
        <div>
          <p class="ts-t">回滚行数</p>
          <p class="ts-s">保留多少行历史输出</p>
        </div>
        <span class="ts-val">{{ settings.scrollback }} 行</span>
      </div>
      <input v-model.number="settings.scrollback" class="ts-range" type="range" min="1000" max="20000" step="1000" />
    </div>

    <div class="ts-card">
      <label class="ts-row">
        <div>
          <p class="ts-t">选中即复制</p>
          <p class="ts-s">鼠标选中文本后自动复制到剪贴板</p>
        </div>
        <input v-model="settings.copyOnSelect" type="checkbox" />
      </label>
    </div>

    <p class="hint">保存后对所有终端会话即时生效（含已打开的窗口）。</p>
    <p v-if="saved" class="ok">已保存</p>
  </div>
</template>

<style scoped>
.ts-page {
  max-width: 560px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.ts-card {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  background: var(--panel, #fff);
  padding: 12px 16px;
}
.ts-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.ts-t {
  font-size: 13px;
  margin: 0;
  font-weight: 500;
}
.ts-s {
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
  margin: 2px 0 0;
}
.ts-val {
  font-size: 12px;
  color: var(--fg-dim, #6b7280);
  font-family: ui-monospace, Menlo, monospace;
}
.ts-range {
  width: 100%;
  margin-top: 8px;
}
.ts-select {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 7px;
  padding: 4px 8px;
  font-size: 12.5px;
  background: var(--panel, #fff);
}
.hint {
  font-size: 12px;
  color: var(--fg-faint, #9ca3af);
  margin: 4px 0 0;
}
.ok {
  font-size: 12.5px;
  color: #15803d;
  margin: 0;
}
</style>
