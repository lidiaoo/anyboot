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

/**
 * 存储 key 与字段名**必须与旧版一致**（`ab-term-settings`）——
 * 终端独立窗读的是同一个 key，改了就互不兼容（旧版保存的设置新版读不到）。
 */
const STORE_KEY = "ab-term-settings";

interface TermSettings {
  /** 旧版字段名：fontSize / cursorBlink / copyOnSelect / scrollback */
  fontSize: number;
  cursorBlink: boolean;
  copyOnSelect: boolean;
  bell: boolean;
  fontFamily: string;
  scrollback: number;
  downDir: string;
}

const DEFAULTS: TermSettings = {
  fontSize: 14,
  cursorBlink: true,
  copyOnSelect: false,
  bell: false,
  fontFamily: "",
  scrollback: 5000,
  downDir: "",
};

/** 下载目录归一化（旧版 normalizeDownDir）：空 / `~` / `~/` 都按「未设置」处理 */
function normalizeDownDir(raw: unknown): string {
  const value = (typeof raw === "string" ? raw : "").trim();
  if (!value || value === "~" || value === "~/") return "";
  return value;
}

/** 读取（旧版 loadTermSettings 的钳制规则：字号 10~24、回滚 500~100000） */
function load(): TermSettings {
  const d = { ...DEFAULTS };
  try {
    const s = JSON.parse(localStorage.getItem(STORE_KEY) || "{}") as Record<string, unknown>;
    return {
      fontSize: Math.min(24, Math.max(10, Number(s.fontSize) || d.fontSize)),
      cursorBlink: s.cursorBlink !== undefined ? Boolean(s.cursorBlink) : d.cursorBlink,
      copyOnSelect: Boolean(s.copyOnSelect),
      bell: Boolean(s.bell),
      scrollback: Math.min(100000, Math.max(500, Number(s.scrollback) || d.scrollback)),
      downDir: normalizeDownDir(s.downDir),
      fontFamily: (typeof s.fontFamily === "string" ? s.fontFamily : "").trim(),
    };
  } catch {
    return d;
  }
}

const settings = ref<TermSettings>(load());
const saved = ref(false);
/** 应用后提示（旧版 toast「已应用：已开的终端窗口与新会话均生效」） */
const toastText = ref("");

watch(
  settings,
  () => {
    // 只标记「未应用」，等点「应用设置」才写盘（旧版就是这样）
    saved.value = false;
  },
  { deep: true },
);

function apply() {
  const s = settings.value;
  const payload: TermSettings = {
    fontSize: Math.min(24, Math.max(10, Number(s.fontSize) || 14)),
    cursorBlink: Boolean(s.cursorBlink),
    copyOnSelect: Boolean(s.copyOnSelect),
    bell: Boolean(s.bell),
    scrollback: Math.min(100000, Math.max(500, Number(s.scrollback) || 5000)),
    downDir: normalizeDownDir(s.downDir),
    fontFamily: String(s.fontFamily ?? "").trim(),
  };
  settings.value = payload;
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(payload));
  } catch {
    /* 存不了就算了 */
  }
  // 通知已开的终端窗热更（旧版事件名）
  void import("@tauri-apps/api/event")
    .then(({ emit }) => emit("ab-term-settings-changed", payload))
    .catch(() => {});
  saved.value = true;
  toastText.value = "已应用：已开的终端窗口与新会话均生效";
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
  </div>

  <div class="ts-wrap">
    <div class="ts-card">
      <div class="ts-row">
        <div>
          <p class="ts-t">字号</p>
          <p class="ts-s">终端字体大小</p>
        </div>
        <span class="ts-val" id="tsFontVal">{{ settings.fontSize }} pt</span>
      </div>
      <input
        id="tsFont"
        v-model.number="settings.fontSize"
        class="ts-range"
        type="range"
        min="10"
        max="24"
        step="1"
        aria-label="字号"
      />
    </div>
    <div class="ts-card">

      <label class="ts-row ts-toggle-row">
        <div>
          <p class="ts-t">光标闪烁</p>
          <p class="ts-s">终端光标呼吸闪烁</p>
        </div>
        <input id="tsCursor" v-model="settings.cursorBlink" type="checkbox" class="ts-check" />
        <span class="ts-switch" aria-hidden="true" />
      </label>

      <label class="ts-row ts-toggle-row">
        <div>
          <p class="ts-t">选中即复制</p>
          <p class="ts-s">划选文本自动进剪贴板</p>
        </div>
        <input id="tsCopy" v-model="settings.copyOnSelect" type="checkbox" class="ts-check" />
        <span class="ts-switch" aria-hidden="true" />
      </label>

      <label class="ts-row ts-toggle-row">
        <div>
          <p class="ts-t">响铃提示</p>
          <p class="ts-s">BEL 字符触发提示音</p>
        </div>
        <input id="tsBell" v-model="settings.bell" type="checkbox" class="ts-check" />
        <span class="ts-switch" aria-hidden="true" />
      </label>

      <div class="ts-row">
        <div>
          <p class="ts-t">字体族</p>
          <p class="ts-s">本机已装的等宽字体名（留空 = 内置等宽栈；字体缺失自动回落，不会坏）</p>
        </div>
        <input
          id="tsFontFamily"
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
          id="tsScroll"
          v-model.number="settings.scrollback"
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
            id="tsDownDir"
            v-model="settings.downDir"
            class="ts-num"
            type="text"
            placeholder="系统下载目录"
            style="width: min(210px, 100%)"
          />
          <button type="button" id="tsDownBrowse" class="btn" style="height: 32px; padding: 0 12px" @click="pickDownDir">
            选择…
          </button>
        </div>
      </div>
    </div>

    <button
      type="button"
      id="tsApply"
      class="btn primary"
      style="height: 40px; width: 100%; justify-content: center"
      @click="apply"
    >
      应用设置
    </button>
    <p v-if="toastText" class="field-hint" style="color: rgb(var(--ok))">{{ toastText }}</p>
  </div>
</template>
