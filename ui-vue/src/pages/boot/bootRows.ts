/**
 * 系统引导页「真实区块行」的统一派生（视觉 1:1）：
 * 旧版 G 主题的分组/计数来自真实区块里的行（boot-adv.js G_ROW_SEL =
 * ".grub-entry, .row"），不是另走一套数据 API。这里把 RefindSection /
 * GrubSection 的行派生抽成共享模块 —— 区块和 G 主题吃同一份行，
 * 计数、排序、选中才可能与旧版一致。
 */
import type { EntryData } from "@/components/boot/types";
import { t } from "@/i18n";


/** 旧版 REFIND_SIM_CHILD_SOURCE（app.js:873） */
export const CHILD_SOURCE: Record<string, string> = {
  default: "hub.child.default",
  options: "refind_linux.conf",
  kernel: "hub.child.kernel",
  macos: "hub.child.macosDiag",
  stanza: "submenuentry",
};

interface SimChildLike {
  title?: string;
  source?: string;
  options?: string;
  note?: string;
}
interface SimEntryLike {
  title?: string;
  kind?: string;
  hidden?: boolean;
  lastBoot?: boolean;
  children?: SimChildLike[];
}

/** rEFInd 自动扫描行（旧版 refindSimEntryNodes 的行集合）：
 *  children>1 → 组（子项挂 submenu，GrubList 合成组头 ▾ + N 项）；
 *  ≤1 → 普通行。 */
export function simRows(sim: { entries?: SimEntryLike[] } | null | undefined): EntryData[] {
  const out: EntryData[] = [];
  for (const e of (sim?.entries ?? []) as SimEntryLike[]) {
    if (e.hidden) continue;
    // 旧版 app.js:925：selector/name 用 **title 原文**（rEFInd 标题末尾常带一个空格，
    // trim 过后端 refind_sim_switch 就匹配不上 → 点击「没反应」）。显示时空格不可见。
    const rawTitle = e.title || "";
    const children = e.children || [];
    if (children.length > 1) {
      for (const c of children) {
        // 子屏内核现在**可点选**：选了它 = 把「它所属的顶层分区条目」设为下次启动
        // （rEFInd 的 PreviousBoot 只存顶层菜单项标题，不存内核名；原生也不能跨重启
        // 记住具体内核 —— 那取决于该分区 refind_linux.conf 的默认行）。
        // selector 用父条目 title 原文（切换写它），name 显示内核名，submenu=组标题。
        out.push({
          selector: rawTitle,
          name: c.title || rawTitle,
          class: e.kind,
          submenu: rawTitle,
          readonly: false,
          tag: CHILD_SOURCE[c.source || ""] ? t(CHILD_SOURCE[c.source || ""]) : "",
        });
      }
    } else {
      out.push({ selector: rawTitle, name: rawTitle, class: e.kind });
    }
  }
  return out;
}

/** rEFInd 手动行（vars 变量，exists 过滤） */
export function refindVariableRows(
  variables: { source?: string; displayName?: string; exists?: boolean; kind?: string }[] | undefined,
): EntryData[] {
  return (variables ?? [])
    .filter((v) => v.exists)
    .map((v) => ({ selector: String(v.source), name: String(v.displayName), class: v.kind }));
}

/** GRUB 自动模式行：自动扫描（linuxGrub）没结果就是 0（旧版同款：目录没
 *  检测到 → 自动面板空，G chip 计 GRUB·0）；手动模式 → 变量行。 */
export function grubRows(opts: {
  grubMode: string;
  linuxGrubEntries: EntryData[] | undefined;
  grubEntries: EntryData[] | undefined;
  variables: { source?: string; displayName?: string; exists?: boolean; kind?: string }[] | undefined;
}): EntryData[] {
  if (opts.grubMode === "manual") {
    return refindVariableRows(opts.variables);
  }
  return opts.linuxGrubEntries ?? [];
}

/**
 * 多实例（BootOption 契约）「一级 › 二级」回显：给当前选中项找它所属的**一级组头**标题。
 * 返回去掉前导 ▾/▸ 与「（N 项）」尾巴的组名；选中项不是组内子项、或找不到组头时返回 ""。
 *
 * 与 Android `pinnedPlain = "$curHeader › $plainLabel"`、iOS
 * `ComputerListView.swift:1866` 以及 PC `ComputerRow.bootEcho` 完全同口径 ——
 * 置顶行 / 下次启动区 / 卡片回显都靠它，避免「只显二级、丢了一级」。
 */
export function parentHeaderOf(options: any[] | null | undefined, current: string): string {
  const list = options ?? [];
  const cur = String(current ?? "").trim();
  if (!cur) return "";
  let idx = list.findIndex((o: any) => !o.header && String(o.value ?? "") === cur);
  if (idx < 0) idx = list.findIndex((o: any) => !o.header && String(o.label ?? "").includes(cur));
  if (idx < 0 || !list[idx]?.child) return "";
  for (let k = idx - 1; k >= 0; k -= 1) {
    if (list[k]?.header) {
      return String(list[k].label ?? "")
        .replace(/^[▾▸\s]+/, "")
        .replace(/（\d+\s*(项|个内核)[^）]*）\s*$/, "")
        .trim();
    }
  }
  return "";
}

/** 组内子项的显示文案：「一级 › 二级」。
 *  一级缺省（顶层项）或与二级同名时只给二级，避免「GRUB › GRUB」。 */
export function groupedLabel(
  parent: string | null | undefined,
  leaf: string | null | undefined,
  empty = "—",
): string {
  const g = String(parent ?? "").trim();
  const l = String(leaf ?? "").trim();
  if (!g || !l || g === l) return l || empty;
  return `${g} › ${l}`;
}

/** 平台图标（四端唯一映射，对齐后端 grub_os_icon）：class 优先 + 名字关键词兜底。
 *  唯一实现 —— GrubEntry（弹窗/列表行）与 hub ComputerRow（卡片回显）都从这里取，
 *  勿再抄第二份（用户 2026-11-15：图标识别也要统一，别这边一个逻辑那边一个逻辑）。 */
export function platformIcon(kind: string | undefined | null, name: string | undefined | null): string {
  const hay = `${kind ?? ""} ${name ?? ""}`.toLowerCase();
  if (hay.includes("win") || hay.includes("microsoft")) return "🪟 ";
  if (hay.includes("mac") || hay.includes("apple")) return "🍎 ";
  if (hay.includes("linux") || hay.includes("cachyos") || hay.includes("ubuntu")
    || hay.includes("fedora") || hay.includes("mint") || hay.includes("manjaro")
    || hay.includes("arch")) return "🐧 ";
  return "";
}
