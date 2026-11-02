<script setup lang="ts">
import { ref, watch } from "vue";

/**
 * 终端设置页（原 `ui/hub.html` 的 `#page-termset`，3459 行）。
 *
 * ## 1:1 结构（旧版字段比我第一版全得多）
 * ```
 * .page-head > h2 + .sub
 * .ts-wrap
 *   .ts-card
 *     .ts-row > div(p.ts-t + p.ts-s) + .ts-val        字号
 *     input.ts-range#tsFont
 *     .ts-row.ts-toggle-row × 3（光标闪烁 / 选中即复制 / 响铃提示）
 *                     input.ts-check + span.ts-switch
 *     .ts-row 字体族   input.ts-num#tsFontFamily
 *     .ts-row 滚动回滚 input.ts-num#tsScroll
 *     .ts-row 下载路径 input.ts-num#tsDownDir + button#tsDownBrowse
 *   button.btn.primary#tsApply「应用设置」
 * ```
 *
 * 与独立终端窗通过 `ab-term-settings-changed` 事件热更。
 */

const STORE_KEY = "anyboot.term-set";

interface TermSettings {
  font: number;
  cursor: boolean;
  copy: boolean;
  bell: boolean;
  fontFamily: string;
  scroll: number;
  downDir: string;
}

const DEFAULTS: TermSettings = {
  font: 14,
  cursor: true,
  copy: false,
  bell: false,
  fontFamily: "",
  scroll: 5000,
  downDir: "",
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
  () => {
    // 只标记「未应用」，等点「应用设置」才写盘（旧版就是这样）
    saved.value = false;
  },
  { deep: true },
);

function apply() {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(settings.value));
  } catch {
    /* 存不了就算了 */
  }
  // 通知已开的终端窗热更（旧版事件名）
  void import("@tauri-apps/api/event")
    .then(({ emit }) => emit("ab-term-settings-changed", settings.value))
    .catch(() => {});
  saved.value = true;
}

function pickDownDir() {
  void import("@/api/tauri")
    .then(({ call }) => call<string>("pick_grub_dir"))
    .then((dir) => {
      if (dir) settings.value.downDir = dir;
    })
    .catch(() => {});
}
</script>

<template>
  <div class="page-head">
    <h2>终端设置</h2>
    <span class="sub">保存后对所有终端会话即时生效（含已开窗口）</span>
  </div>

  <div class="ts-wrap">
    <div class="ts-card">
      <div class="ts-row">
        <div>
          <p class="ts-t">字号</p>
          <p class="ts-s">终端字体大小</p>
        </div>
        <span class="ts-val">{{ settings.font }} pt</span>
      </div>
      <input
        v-model.number="settings.font"
        class="ts-range"
        type="range"
        min="10"
        max="24"
        step="1"
        aria-label="字号"
      />

      <label class="ts-row ts-toggle-row">
        <div>
          <p class="ts-t">光标闪烁</p>
          <p class="ts-s">终端光标呼吸闪烁</p>
        </div>
        <input v-model="settings.cursor" type="checkbox" class="ts-check" />
        <span class="ts-switch" aria-hidden="true" />
      </label>

      <label class="ts-row ts-toggle-row">
        <div>
          <p class="ts-t">选中即复制</p>
          <p class="ts-s">划选文本自动进剪贴板</p>
        </div>
        <input v-model="settings.copy" type="checkbox" class="ts-check" />
        <span class="ts-switch" aria-hidden="true" />
      </label>

      <label class="ts-row ts-toggle-row">
        <div>
          <p class="ts-t">响铃提示</p>
          <p class="ts-s">BEL 字符触发提示音</p>
        </div>
        <input v-model="settings.bell" type="checkbox" class="ts-check" />
        <span class="ts-switch" aria-hidden="true" />
      </label>

      <div class="ts-row">
        <div>
          <p class="ts-t">字体族</p>
          <p class="ts-s">本机已装的等宽字体名（留空 = 内置等宽栈；字体缺失自动回落，不会坏）</p>
        </div>
        <input
          v-model="settings.fontFamily"
          class="ts-num"
          type="text"
          placeholder="内置等宽"
          style="width: min(210px, 100%)"
        />
      </div>

      <div class="ts-row">
        <div>
          <p class="ts-t">滚动回滚行数</p>
          <p class="ts-s">缓冲区保留的历史输出（500–100000）</p>
        </div>
        <input
          v-model.number="settings.scroll"
          class="ts-num"
          type="number"
          min="500"
          max="100000"
          step="500"
        />
      </div>

      <div class="ts-row">
        <div>
          <p class="ts-t">下载路径</p>
          <p class="ts-s">
            文件下载到本机的目录（留空 = 系统下载目录；<span class="mono">~</span> = 用户主目录）
          </p>
        </div>
        <div style="display: flex; gap: 8px; align-items: center; flex: none">
          <input
            v-model="settings.downDir"
            class="ts-num"
            type="text"
            placeholder="系统下载目录"
            style="width: min(210px, 100%)"
          />
          <button type="button" class="btn" style="height: 32px; padding: 0 12px" @click="pickDownDir">
            选择…
          </button>
        </div>
      </div>
    </div>

    <button
      type="button"
      class="btn primary"
      style="height: 40px; width: 100%; justify-content: center"
      @click="apply"
    >
      应用设置
    </button>
    <p v-if="saved" class="field-hint" style="color: rgb(var(--ok))">已应用到所有终端会话</p>
  </div>
</template>
