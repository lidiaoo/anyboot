/**
 * 引导页主题状态（`.ba-adv` 的 Vue 版）。
 *
 * 对齐旧 `ui/boot-adv.js` 的 `advState` + `localStorage["anyboot.boot-adv"]`，
 * **storage key 与字段名完全一致** —— 迁移后用户原来的主题偏好照常生效。
 */

import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import { BOOT_ADV_KEY } from "@/api/constants";
import type { LayoutMode, ThemeKey } from "@/pages/boot/themes/registry";
import { THEME_LAYOUT_MODES } from "@/pages/boot/themes/registry";

/** 全部引导器 id（顺序即默认排序） */
export const LOADER_IDS = ["refind", "grub", "sdboot", "opencore", "clover"] as const;
export type LoaderId = (typeof LOADER_IDS)[number];

/** 「下次启动」区域的显示方式（2026-11-01 三档定案） */
export type NextBootMode = "off" | "fold" | "show";

export interface BootAdvState {
  visible: Record<LoaderId, boolean>;
  order: LoaderId[];
  /** 左右布局时每个引导器分到哪一列（'l' / 'r'） */
  side: Record<string, "l" | "r">;
  layout: LayoutMode;
  /** 左右布局的分割比例 */
  ratio: number;
  /** 密度：full 舒适 / compact 紧凑 */
  style: "full" | "compact";
  theme: ThemeKey | "K";
  /** 「下次启动」显示方式 */
  nextBoot: NextBootMode;
  /** 访客主题（远程查看时由 URL 的 abTheme 带入，不写回本地） */
  guestTheme?: string;
}

const DEFAULT_STATE: BootAdvState = {
  visible: { refind: true, grub: true, sdboot: true, opencore: true, clover: true },
  order: [...LOADER_IDS],
  side: {},
  layout: "default",
  ratio: 0.5,
  style: "full",
  // 出厂默认：旧版 hub 里首次进来是 K（不套用主题，显示完整卡片），
  // 主题偏好存在 localStorage，用户选过就用自己的。
  theme: "K",
  nextBoot: "off",
};

function loadState(): BootAdvState {
  try {
    const raw = localStorage.getItem(BOOT_ADV_KEY);
    if (!raw) return { ...DEFAULT_STATE, order: [...LOADER_IDS] };
    const parsed = JSON.parse(raw) as Partial<BootAdvState>;
    return {
      ...DEFAULT_STATE,
      ...parsed,
      visible: { ...DEFAULT_STATE.visible, ...(parsed.visible ?? {}) },
      side: { ...(parsed.side ?? {}) },
      order: Array.isArray(parsed.order) && parsed.order.length ? parsed.order : [...LOADER_IDS],
    };
  } catch {
    return { ...DEFAULT_STATE, order: [...LOADER_IDS] };
  }
}

export const useBootAdvStore = defineStore("bootAdv", () => {
  const state = ref<BootAdvState>(loadState());

  /** 访客主题：远程查看时 URL 带 `abTheme=<字母>`，只影响渲染、不写回本地 */
  const guestTheme = ref<string>("");
  try {
    const fromUrl = (new URLSearchParams(location.search).get("abTheme") || "").toUpperCase();
    if (/^[A-K]$/.test(fromUrl)) guestTheme.value = fromUrl;
  } catch {
    /* location 不可用时忽略 */
  }

  /** 生效主题：访客 > 本地 */
  const effectiveTheme = computed<ThemeKey | "K">(() => {
    if (guestTheme.value) return guestTheme.value as ThemeKey;
    return state.value.theme;
  });

  /** 生效布局档位：存的档位不在该主题的表里时退回基准档 */
  const effectiveLayout = computed<LayoutMode>(() => {
    const theme = effectiveTheme.value;
    const modes = THEME_LAYOUT_MODES[theme] ?? ["default"];
    const saved = state.value.layout;
    return modes.includes(saved) ? saved : (modes[0] as LayoutMode);
  });

  /** 是否支持左右布局 */
  const supportsSplit = computed(() => {
    const modes = THEME_LAYOUT_MODES[effectiveTheme.value] ?? [];
    return modes.includes("lr");
  });

  /** 按 order + visible 排出的启用引导器 id */
  const enabledLoaders = computed<LoaderId[]>(() => {
    const seen = new Set<string>();
    const out: LoaderId[] = [];
    for (const id of state.value.order) {
      if (!LOADER_IDS.includes(id) || seen.has(id)) continue;
      seen.add(id);
      if (state.value.visible[id]) out.push(id);
    }
    return out;
  });

  function save() {
    if (guestTheme.value) return; // 访客模式不写回
    try {
      localStorage.setItem(BOOT_ADV_KEY, JSON.stringify(state.value));
    } catch {
      /* 存不了就本次会话有效 */
    }
  }

  // 任何字段变动都自动保存（旧实现是每个 setter 里手动调 saveState）
  watch(state, save, { deep: true });

  function setTheme(theme: ThemeKey | "K") {
    state.value.theme = theme;
  }

  function setLayout(layout: LayoutMode) {
    state.value.layout = layout;
  }

  function setVisible(id: LoaderId, visible: boolean) {
    state.value.visible[id] = visible;
  }

  function setOrder(order: LoaderId[]) {
    state.value.order = order;
  }

  function setRatio(ratio: number) {
    state.value.ratio = Math.min(0.85, Math.max(0.15, ratio));
  }

  function setStyle(style: "full" | "compact") {
    state.value.style = style;
  }

  function setNextBoot(mode: NextBootMode) {
    state.value.nextBoot = mode;
  }

  return {
    state,
    guestTheme,
    effectiveTheme,
    effectiveLayout,
    supportsSplit,
    enabledLoaders,
    setTheme,
    setLayout,
    setVisible,
    setOrder,
    setRatio,
    setStyle,
    setNextBoot,
  };
});
