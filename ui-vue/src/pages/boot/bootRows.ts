/**
 * 系统引导页「真实区块行」的统一派生（视觉 1:1）：
 * 旧版 G 主题的分组/计数来自真实区块里的行（boot-adv.js G_ROW_SEL =
 * ".grub-entry, .row"），不是另走一套数据 API。这里把 RefindSection /
 * GrubSection 的行派生抽成共享模块 —— 区块和 G 主题吃同一份行，
 * 计数、排序、选中才可能与旧版一致。
 */
import type { EntryData } from "@/components/boot/types";


/** 旧版 REFIND_SIM_CHILD_SOURCE（app.js:873） */
export const CHILD_SOURCE: Record<string, string> = {
  default: "默认项",
  options: "refind_linux.conf",
  kernel: "折叠内核",
  macos: "macOS 诊断",
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
        out.push({
          selector: c.title || "",
          name: c.title || "",
          class: e.kind,
          submenu: rawTitle,
          // 组子项旧版**没有点击处理器**（不可切，仅展示）——点了后端会拒绝
          readonly: true,
          tag: CHILD_SOURCE[c.source || ""] || "",
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
