/**
 * 强调色应用（对齐旧版 `ui/app.js` 的 shadeHex / defaultAccent / applyAccent）。
 *
 * 根据 accent（用户自定义色）与主题亮暗，派生 4 个 CSS 变量写到 documentElement：
 *   --accent / --accent-hover / --accent-link / --accent-link-hover
 */

const DEFAULT_ACCENT_DARK = "#6366f1";
const DEFAULT_ACCENT_LIGHT = "#4f46e5";

/** resolvedTheme：system 走系统偏好，否则 light/dark */
export function resolvedTheme(theme: string): "light" | "dark" {
  if (theme === "system") {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  return theme === "light" ? "light" : "dark";
}

/** shadeHex：把 #rrggbb 提亮(amount>0)/压暗(amount<0)；非法值返回 null */
export function shadeHex(hex: string | undefined, amount = 0): string | null {
  const match = /^#?([0-9a-f]{6})$/i.exec(String(hex ?? "").trim());
  if (!match) return null;
  const value = Number.parseInt(match[1]!, 16);
  const channels = [(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff].map((c) =>
    Math.round(amount >= 0 ? c + (255 - c) * amount : c * (1 + amount)),
  );
  if (channels.some((c) => Number.isNaN(c))) return null;
  return "#" + channels.map((c) => c.toString(16).padStart(2, "0")).join("");
}

function defaultAccent(theme: string): string {
  return resolvedTheme(theme) === "dark" ? DEFAULT_ACCENT_DARK : DEFAULT_ACCENT_LIGHT;
}

/** 应用 accent + 派生色（旧版 applyAccent） */
export function applyAccent(accent: string | undefined, theme: string): void {
  const base = shadeHex(accent) || defaultAccent(theme);
  const dark = resolvedTheme(theme) === "dark";
  const hover = shadeHex(base, dark ? 0.08 : -0.12) || base;
  const link = shadeHex(base, dark ? 0.18 : -0.18) || base;
  const linkHover = shadeHex(base, dark ? 0.3 : -0.3) || base;
  const root = document.documentElement.style;
  root.setProperty("--accent", base);
  root.setProperty("--accent-hover", hover);
  root.setProperty("--accent-link", link);
  root.setProperty("--accent-link-hover", linkHover);
}
