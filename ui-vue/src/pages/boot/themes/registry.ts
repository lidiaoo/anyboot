/** 主题注册表（纯逻辑部分，供 store 与组件共用） */

export type ThemeKey = "A" | "B" | "C" | "G" | "I" | "J";
export type LayoutMode = "default" | "wide" | "lr";

/**
 * 各主题可用的布局档位；首项 = 该主题的基准档。
 *
 * 2026-10-30 用户定案：B / G / J **只能加宽** —— 它们本身就吃满两栏标称宽，
 * 窗口窄回 460 时只剩一条缝。
 */
export const THEME_LAYOUT_MODES: Record<string, LayoutMode[]> = {
  K: ["default", "wide", "lr"],
  A: ["default", "wide", "lr"],
  C: ["default", "wide", "lr"],
  B: ["wide"],
  G: ["wide"],
  I: ["default", "wide"],
  J: ["wide"],
};

/** 支持的左右布局 = 档位表里有 lr（B/G/I/J 只有单列） */
export const THEME_H: Record<string, boolean> = Object.fromEntries(
  Object.entries(THEME_LAYOUT_MODES).map(([key, modes]) => [key, modes.includes("lr")]),
);

/**
 * 生效的布局档位：存的档位不在该主题的表里时，退回基准档（首项）。
 *
 * 例：B 只有 wide，用户上次在 A 里选了 lr，切到 B 要按 wide 渲染。
 */
export function effectiveLayout(theme: string, saved: LayoutMode | undefined): LayoutMode {
  const modes = THEME_LAYOUT_MODES[theme] ?? ["default"];
  if (saved && modes.includes(saved)) return saved;
  return modes[0] ?? "default";
}

/** 主题中文名（与旧 `THEMES` 数组一致） */
export const THEME_LABELS: { key: ThemeKey | "K"; label: string }[] = [
  { key: "K", label: "不套用主题" },
  { key: "A", label: "A 卡片流" },
  { key: "B", label: "B 左栏通道" },
  { key: "C", label: "C 顶部固定" },
  { key: "G", label: "G 命令面板" },
  { key: "I", label: "I Bento" },
  { key: "J", label: "J 三栏" },
];
