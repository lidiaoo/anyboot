import { createI18n } from "vue-i18n";
import zhCN from "./messages/zh-CN";
import zhTW from "./messages/zh-TW";
import enUS from "./messages/en-US";
import jaJP from "./messages/ja-JP";
import koKR from "./messages/ko-KR";
import deDE from "./messages/de-DE";
import ruRU from "./messages/ru-RU";
import ptBR from "./messages/pt-BR";
import esES from "./messages/es-ES";

/**
 * 支持的语言（Tier 0 + Tier 1，见 docs/i18n-语言清单.md）。
 * zh-HK 复用 zh-TW（差异词后续单独补 zh-HK.ts）；pt-PT 复用 pt-BR；es-419 复用 es-ES。
 */
export const SUPPORTED_LOCALES = [
  "zh-CN",
  "zh-TW",
  "en-US",
  "ja-JP",
  "ko-KR",
  "de-DE",
  "ru-RU",
  "pt-BR",
  "es-ES",
] as const;
export type Locale = (typeof SUPPORTED_LOCALES)[number];

/** 语言菜单用**本族名**显示（不显示英文名） */
export const LOCALE_LABELS: Record<Locale, string> = {
  "zh-CN": "简体中文",
  "zh-TW": "繁體中文",
  "en-US": "English",
  "ja-JP": "日本語",
  "ko-KR": "한국어",
  "de-DE": "Deutsch",
  "ru-RU": "Русский",
  "pt-BR": "Português (Brasil)",
  "es-ES": "Español",
};

/** 回退链：变体 → 主语言 → 英文 */
const FALLBACK_CHAIN: Record<string, string[]> = {
  "zh-HK": ["zh-TW", "zh-CN", "en-US"],
  "zh-TW": ["zh-CN", "en-US"],
  "pt-PT": ["pt-BR", "en-US"],
  "es-419": ["es-ES", "en-US"],
};

/** 把系统语言（如 zh-HK / en-GB / pt-PT）归一到支持的语言 */
export function normalizeLocale(raw: string): Locale {
  const tag = String(raw || "").trim();
  if (!tag) return "zh-CN";
  if ((SUPPORTED_LOCALES as readonly string[]).includes(tag)) return tag as Locale;
  for (const chain of Object.values(FALLBACK_CHAIN)) {
    // 由具体变体键命中（如 zh-HK）
    const hitKey = Object.keys(FALLBACK_CHAIN).find((k) => k.toLowerCase() === tag.toLowerCase());
    if (hitKey) return chain[0] as Locale;
  }
  const lower = tag.toLowerCase();
  if (lower.startsWith("zh")) return lower.includes("tw") || lower.includes("hk") || lower.includes("hant") ? "zh-TW" : "zh-CN";
  if (lower.startsWith("ja")) return "ja-JP";
  if (lower.startsWith("ko")) return "ko-KR";
  if (lower.startsWith("de")) return "de-DE";
  if (lower.startsWith("ru")) return "ru-RU";
  if (lower.startsWith("pt")) return "pt-BR";
  if (lower.startsWith("es")) return "es-ES";
  if (lower.startsWith("en")) return "en-US";
  return "en-US";
}

/** 语言偏好：'auto' = 跟随系统 */
export type LangPref = Locale | "auto";
const STORAGE_KEY = "anyboot.lang";

function readPref(): LangPref {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    if (!v) return "auto";
    if (v === "auto") return "auto";
    return normalizeLocale(v);
  } catch {
    return "auto";
  }
}

export function initialLocale(): Locale {
  const pref = readPref();
  if (pref !== "auto") return pref;
  return normalizeLocale(navigator.language || "zh-CN");
}

export const i18n = createI18n({
  legacy: false,
  globalInjection: true,
  locale: initialLocale(),
  fallbackLocale: "zh-CN",
  messages: {
    "zh-CN": zhCN,
    "zh-TW": zhTW,
    "en-US": enUS,
    "ja-JP": jaJP,
    "ko-KR": koKR,
    "de-DE": deDE,
    "ru-RU": ruRU,
    "pt-BR": ptBR,
    "es-ES": esES,
  },
  // 数字/日期统一走 Intl（业务里用 n()/d()，不手拼）
  numberFormats: {
    "zh-CN": { ms: { style: "unit", unit: "millisecond", unitDisplay: "short" } },
    "en-US": { ms: { style: "unit", unit: "millisecond", unitDisplay: "short" } },
  },
});

/** 非组件（store / toast / 工具函数）里取译文 */
export const t = (key: string, args?: Record<string, unknown>): string =>
  args ? (i18n.global.t as any)(key, args) : (i18n.global.t as any)(key);

/**
 * 切换语言：
 * 1) vue-i18n locale  2) <html lang>  3) localStorage（'auto' 表示跟随系统）
 * 4) 同步后端（托盘/通知窗跟随本机设置）  5) 广播事件（远程 iframe 追加 abLang）
 */
export function setLocale(pref: LangPref) {
  const effective: Locale = pref === "auto" ? normalizeLocale(navigator.language || "zh-CN") : pref;
  i18n.global.locale.value = effective;
  document.documentElement.lang = effective;
  try {
    localStorage.setItem(STORAGE_KEY, pref === "auto" ? "auto" : pref);
    localStorage.setItem("anyboot.lang.effective", effective);
  } catch { /* 忽略 */ }
  void import("@/api/tauri")
    .then(({ call }) => call("set_language", { lang: effective }).catch(() => {}))
    .catch(() => {});
  try { window.dispatchEvent(new CustomEvent("anyboot:locale", { detail: effective })); } catch { /* 忽略 */ }
}

export function currentLocale(): Locale {
  return i18n.global.locale.value as Locale;
}

export function currentPref(): LangPref {
  return readPref();
}

/** 各入口统一安装（接受 Vue App 实例） */
export function installI18n(app: import("vue").App) {
  app.use(i18n);
  document.documentElement.lang = i18n.global.locale.value as string;
}
