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
  /** 卷标（rEFInd VolName）——按它把同一引导分区下的条目合并成组 */
  volume?: string;
  /** internal / external / fallback / ext4 / windows / macos / protocol / user … */
  source?: string;
  children?: SimChildLike[];
}

/** rEFInd 自动扫描行（旧版 refindSimEntryNodes 的行集合）：
 *  children>1 → 组（子项挂 submenu，GrubList 合成组头 ▾ + N 项）；
 *  ≤1 → 普通行。 */
export function simRows(sim: { entries?: SimEntryLike[] } | null | undefined): EntryData[] {
  const out: EntryData[] = [];
  const list = (sim?.entries ?? []).filter((e) => !e.hidden);

  // 先按「引导分区（volume）」聚合（保持首次出现顺序）。
  // 目的：同一条引导分区下的多个条目（如 efi 卷上 9 个 Arch 内核 .efi + fallback）
  // 合并成一个组头（下拉），子项是各条目 —— 与 GRUB 的分组菜单同构（用户要求
  // 「refind 合并同一引导分区下的所有高级引导，像 grub 的引导菜单一样有层级」）。
  const order: string[] = [];
  const groups = new Map<string, SimEntryLike[]>();
  for (const e of list) {
    const vol = String(e.volume ?? "");
    if (!groups.has(vol)) {
      groups.set(vol, []);
      order.push(vol);
    }
    groups.get(vol)!.push(e);
  }

  for (const vol of order) {
    const members = groups.get(vol) ?? [];
    // 单条（或卷标为空）→ 平铺，不合成组（避免「一项也下拉」的多余层级）。
    if (members.length <= 1 || vol === "") {
      for (const e of members) pushEntry(out, e);
      continue;
    }
    // 同卷多条 → 组头（卷标）+ 每一条作为子项（submenu = 组标题）。
    const groupTitle = vol;
    for (const e of members) {
      const children = e.children || [];
      if (children.length > 1) {
        // 条目自身还带子屏（rEFInd Boot Options 变体）：保留为更深一层，
        // 但仍归到本卷组下——这里的每个 child 也作为本组子项。
        for (const c of children) {
          out.push({
            selector: c.title || "",
            name: c.title || "",
            class: e.kind,
            submenu: groupTitle,
            readonly: true,
            tag: CHILD_SOURCE[c.source || ""] || "",
          });
        }
      } else {
        out.push({
          selector: e.title || "",
          name: e.title || "",
          class: e.kind,
          submenu: groupTitle,
          readonly: false,
        });
      }
    }
  }
  return out;
}

/** 单条 rEFInd 条目 → 一行（自带子屏时展开为不可点子项，但顶层无卷分组时用）。 */
function pushEntry(out: EntryData[], e: SimEntryLike): void {
  const rawTitle = e.title || "";
  const children = e.children || [];
  if (children.length > 1) {
    for (const c of children) {
      out.push({
        selector: c.title || "",
        name: c.title || "",
        class: e.kind,
        submenu: rawTitle,
        readonly: true,
        tag: CHILD_SOURCE[c.source || ""] || "",
      });
    }
  } else {
    out.push({ selector: rawTitle, name: rawTitle, class: e.kind });
  }
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
