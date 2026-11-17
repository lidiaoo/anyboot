<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { call } from "@/api/tauri";
import GrubList from "@/components/boot/GrubList.vue";
import type { EntryData } from "@/components/boot/types";
import type { RemoteProfile } from "@/stores/remote";
import type { RemoteBootInfo } from "@/api/types";
import { useToastStore } from "@/stores/toast";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { CHILD_SOURCE, platformIcon } from "@/pages/boot/bootRows";
import { useI18n } from "vue-i18n";
import { currentLocale } from "@/i18n";
const { t } = useI18n();

/**
 * 电脑列表的一行（**1:1 对齐旧版 `cqRowEl()` hub.html:8347**）。
 *
 * 旧版结构（`hub.html:8634` 起，用户 2026-11-01 定案的「移动端同款上下布局」）：
 *
 * ```
 * .cq-row[data-rid]
 *   .cq-row-head          头部行
 *     .cq-col-dev          图标 + 名称 + 徽章 + 地址
 *     .cq-col-st           状态
 *     .cq-col-more         更多
 *   .cq-row-boots         引导项区（wrap 一行）
 *     .cq-col-boot[data-label="rEFInd 启动项（i18n: hub.boot.col）"]
 *     .cq-col-boot[data-label="RefindPlus 启动项"]   与 rEFInd 并列的独立通道
 *     .cq-col-boot[data-label="GRUB 启动项"]
 *     .cq-col-boot.cq-col-boot--ch[…]   三通道（检测到才有）
 *   .cq-row-pwr           电源区
 *     .cq-col-pwr > .cq-pwr > button.cq-pwr-btn × 4
 * ```
 *
 * **注意：不是六列横排**。我第一版做成了 `.cq-col-*` 六列 grid，
 * 而 CSS 里 `.cq-row` 是 `flex-direction: column`，`.cq-thead { display: none }`。
 */

const props = defineProps<{
  row: RemoteProfile;
  info: RemoteBootInfo | undefined;
  pending: boolean;
  /** 是否选中（.cq-selected） */
  selected: boolean;
  /** 自定义排序模式下显示 ↑↓ */
  sortMode: string;
  /** 该行在 remotes 里的序号（排序用） */
  orderIndex: number;
  /** remotes 总数（判断 ↑↓ 是否到头） */
  orderTotal: number;
  /** 所有电脑里检测到(available)的通道 kind 并集（有序），保证各行通道列对齐（旧版 cqBootColumns） */
  channelKinds?: string[];
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "switchBoot", payload: Record<string, unknown>): void;
  (e: "power", payload: Record<string, unknown>): void;
  (e: "sysInfo", id: string): void;
  (e: "more", id: string): void;
  (e: "view", id: string): void;
  (e: "openBrowser", id: string): void;
  (e: "startTunnel", id: string): void;
  (e: "stopTunnel", id: string): void;
  (e: "move", payload: { id: string; delta: number }): void;
}>();

const isLocal = computed(() => props.row.id === "local");
/** 点启动项列弹出的条目选择器：{ kind, title, options } —— 选中后带 value 调 switchBoot */
const picker = ref<{ kind: string; title: string; entries: EntryData[]; active: string; groupTitles: Record<string, string>; flat: boolean } | null>(null);
const ui = useUiStateStore();
const appSettings = useSettingsStore();
const pickerFmt = ref<string>(""); // 空 = 跟随电脑（settings.grubListFormat）
const isGrubPicker = computed(() => {
  const kind = picker.value?.kind ?? "";
  return kind === "grub" || kind.startsWith("grub@");
});
/** 本机：引导器数据以「系统引导页」同源快照为准（linuxGrub/refindSim/channel.menuPreview），转换规则与 BootList 一致。 */
function localEntries(kind: string): { entries: EntryData[]; active: string; groupTitles: Record<string, string>; flat: boolean } | null {
  if (kind === "grub") {
    const src: any = (ui as any).linuxGrub ?? null;
    const list = (src?.entries ?? []) as any[];
    const groupTitles: Record<string, string> = {};
    for (const g of (src?.groups ?? []) as any[]) if (g.id) groupTitles[g.id] = g.name ?? "";
    const entries: EntryData[] = list.map((e) => ({ selector: e.selector, name: e.name, class: e.class, kernels: e.kernels, variants: e.variants, chainloader: e.chainloader, submenu: e.submenu } as EntryData));
    return { entries, active: String(src?.nextEntry || src?.savedEntry || ""), groupTitles, flat: false };
  }
  if (kind === "refind" || kind === "refindplus") {
    const sim: any = kind === "refind" ? (ui as any).refindSim : (ui as any).refindplusSim;
    const list = (sim?.entries ?? []) as any[];
    const entries: EntryData[] = list.filter((e) => !e.hidden).map((e) => ({ selector: String(e.title ?? ""), name: String(e.title ?? "") } as EntryData));
    const active = String(list.find((e) => e.lastBoot)?.title ?? "");
    return { entries, active, groupTitles: {}, flat: true };
  }
  if (kind.startsWith("channel:")) {
    const id = kind.slice("channel:".length);
    const ch = ((ui as any).channels ?? []).find((c: any) => c.id === id);
    const preview = (ch?.menuPreview?.entries ?? []) as any[];
    const orderSrc = ch?.menuPreview?.orderSource ?? "";
    // 与 BootList.channelEntries 一致：可选中(spec 非空)在前、其余在后（保持各自原序）
    const ordered = [...preview.filter((r) => r.spec), ...preview.filter((r) => !r.spec)];
    const entries: EntryData[] = ordered.map((row) => ({
      selector: String(row.spec || row.title || ""),
      name: String(row.title || ""),
      class: row.kind,
      readonly: !row.spec,
      tags: [String(row.kind || ""), orderSrc === "private" ? t("hub.oc.private") : (orderSrc ? t("hub.oc.firmware") : "")].filter(Boolean),
      sub: row.volume && row.volume !== "—" ? `${row.volume} · ${row.path}` : (row.path || undefined),
      note: row.note || undefined,
    } as EntryData));
    // 当前默认：default 行 或 currentDefault 命中
    const cur = String(ch?.currentDefault || "");
    const hit = ordered.find((r) => r.default === true || (cur && (cur === (r.spec || r.title) || cur.indexOf(String(r.title || "")) >= 0)));
    const active = String(hit ? (hit.spec || hit.title || "") : "");
    return { entries, active, groupTitles: {}, flat: true };
  }
  return null;
}
/** 统一中间结构：本机/非本机的数据都先归一化成它，再由唯一转换产出 EntryData。
 *  这是「电脑列表下拉」唯一的数据转换——本机/非本机只有取数不同，转换规则一套。 */
interface PickerSource {
  title: string;
  current: string;
  groupTitles: Record<string, string>;
  /** hasGroups=false = 平铺（无组头行），如 OC；true = 组结构（GRUB/rEFInd） */
  hasGroups: boolean;
  /** 归一化条目：header=true 表示组头行；fixed=true 表示只读兜底行 */
  items: { value: string; label: string; cls?: string; header?: boolean; fixed?: boolean; tag?: string; sub?: string; note?: string; kernels?: string[]; variants?: number; chainloader?: boolean; submenuId?: string }[];
}

async function openPicker(kind: string) {
  pickerFmt.value = "";
  const isGrubKind = kind === "grub";
  const isRefindKind = kind === "refind" || kind === "refindplus";
  const isChannelKind = kind === "sdboot" || kind === "opencore" || kind === "clover";
  const CLASS_LABEL: Record<string, string> = { mac: "macOS", win: "Windows", auto: "hub.class.generic", tool: "hub.class.tool", linux: "Linux", generic: "hub.class.generic" };

  // 多实例（instanceId）统一入口：直接按该实例的 options 生成下拉
  const instGroup: any = channelOf(kind);
  if (instGroup && instGroup.instanceId) {
    const opts: any[] = (instGroup.options ?? []) as any[];
    const entries = opts.map((o: any) => ({
      selector: String(o.value ?? ""),
      name: String(o.label ?? o.value ?? ""),
      class: o.class,
      kernels: Array.isArray(o.kernels) ? o.kernels : [],
      header: !!o.header,
      child: !!o.child,
      tag: o.tag || "",
      sub: o.sub || "",
      note: o.note || "",
      readonly: !!o.fixed || !o.value,
    }));
    picker.value = {
      kind,
      title: instGroup.title || kind,
      entries: entries as any,
      active: String(instGroup.current ?? "").trim(),
      groupTitles: {},
      flat: true,
    };
    return;
  }

  // 通道（sdboot/opencore/clover）统一：所有通道同一套下拉，直接读 info.channels[].options（BootOption）
  if (isChannelKind) {
    const group: any = (props.info?.channels ?? []).find((c: any) => c.kind === kind);
    const opts: any[] = (group?.options ?? []) as any[];
    const entries = opts.map((o: any) => ({
      selector: String(o.value ?? ""),
      name: String(o.label ?? o.value ?? ""),
      class: o.class,
      kernels: Array.isArray(o.kernels) ? o.kernels : [],
      tag: o.tag || "",
      sub: o.sub || "",
      note: o.note || "",
      readonly: !!o.fixed || !o.value,
      header: !!o.header,
      child: !!o.child,
    }));
    picker.value = {
      kind,
      title: group?.title ?? kind,
      entries: entries as any,
      active: String(group?.current ?? "").trim(),
      groupTitles: {},
      flat: true,
    };
    return;
  }

  // ── 1) 取数并归一化为 PickerSource（本机：uiState；非本机：remote_boot_info）──
  let src: PickerSource | null = null;
  if (isLocal.value) {
    try { await (ui as any).pollOnce?.(); } catch { /* 忽略 */ }
    if (isGrubKind) {
      const g: any = (ui as any).linuxGrub ?? null;
      src = {
        title: "GRUB",
        current: String(g?.nextEntry || g?.savedEntry || "").trim(),
        groupTitles: Object.fromEntries(((g?.groups ?? []) as any[]).filter((x) => x.id).map((x) => [x.id, x.name ?? ""])),
        hasGroups: true,
        items: ((g?.entries ?? []) as any[]).map((e) => ({ value: e.selector, label: e.name, cls: e.class, kernels: e.kernels, variants: e.variants, chainloader: e.chainloader, submenuId: e.submenu })),
      };
    } else if (isRefindKind) {
      const sim: any = kind === "refind" ? (ui as any).refindSim : (ui as any).refindplusSim;
      const items: PickerSource["items"] = [];
      const groupTitles: Record<string, string> = {};
      let current = "";
      for (const e of (sim?.entries ?? []) as any[]) {
        if (e.hidden) continue;
        const rawTitle = e.title || "";
        const children = e.children || [];
        if (children.length > 1) {
          groupTitles[rawTitle] = rawTitle;
          items.push({ value: "", label: rawTitle, cls: e.kind, header: true, submenuId: rawTitle });
          for (const c of children) {
            const cname = String(c.title || rawTitle).split(":")[0].trim();
            items.push({ value: rawTitle, label: cname, cls: e.kind, submenuId: rawTitle, tag: CHILD_SOURCE[c.source || ""] ? t(CHILD_SOURCE[c.source || ""]) : "", fixed: false });
          }
        } else {
          items.push({ value: rawTitle, label: rawTitle, cls: e.kind, fixed: false });
        }
        if (e.lastBoot) current = rawTitle;
      }
      src = { title: kind === "refind" ? "rEFInd" : "RefindPlus", current: current.trim(), groupTitles, hasGroups: true, items };
    } else if (isChannelKind) {
      const id = kind.slice("channel:".length);
      const ch = ((ui as any).channels ?? []).find((c: any) => c.id === id);
      const preview = (ch?.menuPreview?.entries ?? []) as any[];
      const orderSrc = ch?.menuPreview?.orderSource ?? "";
      const ordered = [...preview.filter((r) => r.spec), ...preview.filter((r) => !r.spec)];
      src = {
        title: ch?.label ?? id,
        current: String(ch?.currentDefault || ""),
        groupTitles: {},
        hasGroups: false,  // OC：条目 submenu 为空，天然平铺（无组头行）——与 GRUB 行样式一致
        items: ordered.map((row) => ({
          value: String(row.spec || ""),
          label: String(row.title || ""),
          cls: row.kind,
          fixed: !row.spec,
          tag: [t(CLASS_LABEL[row.kind] || "") || String(row.kind || ""), orderSrc === "private" ? t("hub.oc.private") : orderSrc ? t("hub.oc.firmware") : ""].filter(Boolean).join(" · "),
          sub: row.volume && row.volume !== "—" ? `${row.volume} · ${row.path}` : row.path || undefined,
          note: row.note || undefined,
        })),
      };
    }
  } else {
    const info: any = props.info;
    const group = kind === "refind" ? info?.refind : kind === "refindplus" ? info?.refindplus
      : kind === "grub" ? info?.grub
      : (info?.channels ?? []).find((c: any) => c.kind === kind);
    // 统一化：对端原始 JSON 透传（group.raw）→ 与本机**同一套映射**（回显/弹窗一字不差）
    const rawJson = (group as any)?.raw;
    if (rawJson && isGrubKind) {
      const g: any = rawJson;
      src = {
        title: "GRUB",
        current: String(g?.nextEntry || g?.savedEntry || "").trim(),
        groupTitles: Object.fromEntries(((g?.groups ?? []) as any[]).filter((x) => x.id).map((x) => [x.id, x.name ?? ""])),
        hasGroups: true,
        items: ((g?.entries ?? []) as any[]).map((e) => ({ value: e.selector, label: e.name, cls: e.class, kernels: e.kernels, variants: e.variants, chainloader: e.chainloader, submenuId: e.submenu })),
      };
    } else if (rawJson && isRefindKind) {
      const sim: any = rawJson;
      const items: PickerSource["items"] = [];
      const groupTitles: Record<string, string> = {};
      let current = "";
      for (const e of (sim?.entries ?? []) as any[]) {
        if (e.hidden) continue;
        const rawTitle = e.title || "";
        const children = e.children || [];
        if (children.length > 1) {
          groupTitles[rawTitle] = rawTitle;
          items.push({ value: "", label: rawTitle, cls: e.kind, header: true, submenuId: rawTitle });
          for (const c of children) {
            const cname = String(c.title || rawTitle).split(":")[0].trim();
            items.push({ value: rawTitle, label: cname, cls: e.kind, submenuId: rawTitle, tag: CHILD_SOURCE[c.source || ""] ? t(CHILD_SOURCE[c.source || ""]) : "", fixed: false });
          }
        } else {
          items.push({ value: rawTitle, label: rawTitle, cls: e.kind, fixed: false });
        }
        if (e.lastBoot) current = rawTitle;
      }
      src = { title: kind === "refind" ? "rEFInd" : "RefindPlus", current: current.trim(), groupTitles, hasGroups: true, items };
    } else if (!group) { src = null; } else {
      const opts = ((group as { options?: any[] })?.options ?? []) as any[];
      const current = String((group as { current?: string })?.current ?? "").trim();
      // remote options：GRUB/rEFInd 带组结构（header=组头，其后为子项）；OC 平铺（fixed 行）
      const items: PickerSource["items"] = [];
      const groupTitles: Record<string, string> = {};
      let groupLabel = "";
      let hasGroups = false;
      for (const o of opts) {
        if (o.header) {
          groupLabel = String(o.label ?? "").replace(/^[▾▸\s]+/, "").replace(/（\d+\s*(项|个内核)[^）]*）\s*$/, "").trim();
          if (isGrubKind || isRefindKind) { if (groupLabel) { groupTitles[groupLabel] = groupLabel; hasGroups = true; } }
          continue;
        }
        const value = String(o.value ?? "").trim();
        const isFixed = Boolean(o.fixed) || !value;
        items.push({
          value,
          label: String(o.label ?? value).trim(),
          cls: o.class,
          header: false,
          fixed: isFixed,
          submenuId: groupLabel && (isGrubKind || isRefindKind) ? groupLabel : "",
          tag: String(o.tag ?? ""),
          sub: String(o.sub ?? ""),
          note: String(o.note ?? ""),
        });
      }
      src = { title: (group as { title?: string })?.title ?? localTitle(kind), current, groupTitles, hasGroups: hasGroups && !(isChannelKind), items };
    }
  }

  if (!src) return;

  // ── 2) 唯一转换：PickerSource → GrubList 的 EntryData（本机/非本机同一段代码）──
  const entries: EntryData[] = [];
  let active = "";
  if (src.hasGroups) {
    for (const it of src.items) {
      if (it.header) {
        continue;
      }
      const e: EntryData = { selector: it.value.trim(), name: it.label.trim() };
      if (it.cls) e.class = it.cls;
      if (it.kernels) e.kernels = it.kernels;
      if (it.variants) e.variants = it.variants;
      if (it.chainloader !== undefined) e.chainloader = it.chainloader;
      if (it.tag) { e.tags = [it.tag]; e.tag = it.tag; }
      if (it.submenuId) e.submenu = src.groupTitles[it.submenuId] || it.submenuId;
      if (it.fixed) e.readonly = true;
      entries.push(e);
    }
    active = src.current;
  } else {
    for (const it of src.items) {
      const sel = (it.value || it.label).trim();
      const e: EntryData = { selector: sel, name: it.label.trim() };
      if (it.cls) e.class = it.cls;
      if (it.tag) e.tags = [it.tag];
      if (it.sub) e.sub = it.sub;
      if (it.note) e.note = it.note;
      if (it.fixed || !it.value) e.readonly = true;
      entries.push(e);
      if (!active && (it.value === src.current || it.label === src.current)) active = sel;
    }
  }
  if (!entries.length) return;
  picker.value = { kind, title: src.title, entries, active, groupTitles: src.groupTitles, flat: !src.hasGroups };
}
function chooseBoot(value: string) {
  const pk = picker.value; if (!pk) return;
  picker.value = null;
  emit("switchBoot", { serverId: props.row.id, kind: pk.kind, value });
}
/** 旧版：本机恒在线（hub 自身在跑）；远端看 remote_boot_info 的 online */
const online = computed(() => (isLocal.value ? true : (props.info?.online ?? false)));
const loading = computed(() => props.info === undefined && !isLocal.value);

/** 旧 cqStateCell：online===null → busy(未探测)；true→on / false→off */
const stateClass = computed(() => {
  if (online.value === null || loading.value) return "cq-st--busy";
  return online.value ? "cq-st--on" : "cq-st--off";
});
const statusLabel = computed(() => {
  if (loading.value) return t("hub.state.reading");
  if (online.value === null) return t("hub.state.unprobed");
  return online.value ? t("hub.state.online") : t("hub.state.offline");
});

/** 系统徽标 */
const osBadge = computed(() => {
  const os = String(props.row.machine_os ?? "").toLowerCase();
  // 照旧版：OS 徽标用全称 Windows / macOS / Linux（不是缩写）
  if (os.includes("win")) return { text: "Windows", kind: "windows" };
  if (os.includes("mac")) return { mac: true, text: "macOS", kind: "macos" };
  if (os.includes("linux")) return { text: "Linux", kind: "linux" };
  return null;
});

/** 连接标注（旧版 cq-chip-grp / cq-chip-remote） */
const connLabel = computed(() => (isLocal.value ? t("hub.conn.local") : online.value ? t("hub.conn.direct") : t("hub.conn.disconnected")));

/** 引导项值 */
/** 旧 cqBootCell 状态：label + 是否不可改(--none) + 内核点色 k-{class} */
const fmtChoices = [
  { v: "", label: "hub.picker.fmt.follow" },
  { v: "minimal", label: "hub.picker.fmt.minimal" },
  { v: "simple", label: "hub.picker.fmt.simple" },
  { v: "medium", label: "hub.picker.fmt.medium" },
  { v: "raw", label: "hub.picker.fmt.raw" },
  { v: "detailed", label: "hub.picker.fmt.detailed" },
];

function localTitle(kind: string): string {
  if (kind === "refind") return "rEFInd"; if (kind === "refindplus") return "RefindPlus"; if (kind === "grub") return "GRUB";
  const id = kind.startsWith("channel:") ? kind.slice(8) : kind;
  return id.charAt(0).toUpperCase() + id.slice(1);
}
/** 卡片回显：与弹窗置顶副本同口径 —— 当前项是组内子项时拼「组名 › 子名」，顶层显原名 */
function echoLabel(groupLabel: string, name: string): string {
  return groupLabel ? `${groupLabel} › ${name}` : name;
}
/** 对端 /api/grub JSON → 与本机 linuxGrub 完全同构的 EntryData 集（回显/弹窗一字不差） */
function grubEntriesFromJson(src: any): { entries: EntryData[]; active: string; groupTitles: Record<string, string> } {
  const groupTitles: Record<string, string> = {};
  for (const g of (src?.groups ?? []) as any[]) if (g.id) groupTitles[g.id] = g.name ?? "";
  const entries: EntryData[] = ((src?.entries ?? []) as any[]).map((e) => ({ selector: e.selector ?? e.id, name: e.name, class: e.class, kernels: e.kernels, variants: e.variants, chainloader: e.chainloader, submenu: e.submenu } as EntryData));
  return { entries, active: String(src?.nextEntry || src?.savedEntry || ""), groupTitles };
}
function bootState(kind: string): { label: string; none: boolean; dot: string } {
  if (isLocal.value) {
    const le = localEntries(kind);
    if (!le || !le.entries.length) return { label: t("hub.state.notDetected"), none: true, dot: "" };
    let hit = le.entries.find((e) => String(e.selector) === le.active);
    if (!hit) hit = le.entries.find((e) => le.active && e.name && String(e.name).indexOf(le.active) >= 0);
    const gl = hit?.submenu ? (le.groupTitles?.[String(hit.submenu)] ?? "") : "";
    return { label: hit ? echoLabel(gl, String(hit.name)) : (le.active || t("hub.state.notSelected")), none: false, dot: (hit as any)?.class || "" };
  }
  const group = kind === "refind" || kind === "refindplus" || kind === "grub" ? props.info?.[kind] : undefined;
  if (!group) return { label: online.value === null || loading.value ? t("hub.state.reading") : t("hub.state.notDetected"), none: true, dot: "" };
  if (!group.available) return { label: t("hub.state.notDetected"), none: true, dot: "" };
  if (!online.value) return { label: t("hub.state.offlineReadonly"), none: true, dot: "" };
  // 统一化：对端原始 JSON 透传（group.raw）→ 与本机**同一套规则**（回显一字不差）：
  // GRUB = 「组名 › 完整名」（groupTitles）；rEFInd/RefindPlus 的 lastBoot 在父条目上
  // → 直接显示父条目名（trim 尾空格），与各端页面字面一致。
  const rawJson = (group as any)?.raw;
  if (rawJson) {
    if (kind === "grub") {
      const le = grubEntriesFromJson(rawJson);
      let hit = le.entries.find((e) => String(e.selector) === le.active);
      if (!hit) hit = le.entries.find((e) => le.active && e.name && String(e.name).indexOf(le.active) >= 0);
      const gl = hit?.submenu ? (le.groupTitles?.[String(hit.submenu)] ?? "") : "";
      return { label: hit ? echoLabel(gl, String(hit.name)) : (le.active || t("hub.state.notSelected")), none: false, dot: (hit as any)?.class || "" };
    }
    const lastBoot = String(((rawJson?.entries ?? []) as any[]).find((e) => e.lastBoot)?.title ?? "").trim();
    return { label: lastBoot || t("hub.state.notSelected"), none: false, dot: "" };
  }
  const options = (group as { options?: { value?: string; label?: string; class?: string; header?: boolean }[] }).options || [];
  const current = String(group.current || "");
  const hit =
    options.filter((o) => !o.header && o.value === current)[0] ||
    options.filter((o) => {
      if (o.header || !current) return false;
      const text = String(o.label || "").trim();
      return !!text && (text === current || text.indexOf(current) >= 0 || current.indexOf(text) >= 0);
    })[0];
  // 组名：命中子项时向上找最近的组头行（去 ▾ 前缀与计数括号）
  let gl = "";
  if (hit) {
    const hi = options.indexOf(hit);
    for (let k = hi - 1; k >= 0; k--) {
      if (options[k].header) {
        gl = String(options[k].label ?? "")
          .replace(/^[▾▸\s]+/, "")
          .replace(/（\d+\s*(项|个内核)[^）]*）\s*$/, "")
          .trim();
        break;
      }
    }
  }
  return {
    label: hit ? echoLabel(gl, String(hit.label ?? current ?? "")) : (current || t("hub.state.notSelected")),
    none: options.length === 0,
    dot: hit?.class || "",
  };
}
/** 连接通道连通性行（旧版 cq-dev-line 逐通道探测明细）：受通道粒度 route_probe_hidden 与
 *  配对级 show_probe 控制——被隐藏的不展示，探测/连接照常。 */
// 通道徽章文案（照抄 v0.2.5 ROUTE_LABELS）
const ROUTE_LABELS: Record<string, string> = {
  lan: "hub.route.lan", jump: "hub.route.jump", tunnel: "hub.route.tunnel",
  ts: "hub.route.ts", funnel: "hub.route.funnel", tsnet: "hub.route.tsnet",
};
interface ConnectLine { dataK: string; kindLabel: string; address: string; ok: boolean; ms: number; skipped: boolean;
  badge: string; reason: string; macs: string[]; isLan: boolean; url: string;
  /** 延迟档位（决定 badge 颜色）：ok <80 / warn <300 / bad ≥300（仅“通”时） / skip 未测出(含失败) */
  tone: "" | "ok" | "warn" | "bad" | "skip"; }
const connectLines = computed<ConnectLine[]>(() => {
  // 照抄 v0.2.5：连通性行 = 每个「局域网地址」一行(直连) + 每个「配对」一行(远程通道)；
  // 探测结果从 info.probe_results 查(kind + 地址/配对id)，有无探测都出地址行。
  const info = props.info;
  const r = props.row as Record<string, unknown>;
  const hidden = new Set<string>((r.route_probe_hidden as string[] | undefined) ?? []);
  const disabled = new Set<string>((r.route_disabled as string[] | undefined) ?? []);
  const addresses = ((r.addresses as string[] | undefined) ?? []).map((a) => String(a).trim()).filter(Boolean);
  const macsMap = (r.macs as Record<string, string[]> | undefined) ?? {};
  const pairs = (r.pairs as { id?: string; kind?: string; lane?: string; addr?: string; token?: string; show_probe?: boolean }[] | undefined) ?? [];
  const rowTok = String(r.token ?? "");
  const probes = info?.probe_results ?? [];
  // 探测结果索引：kind\u0000(address) 与 kind\u0000(pair_id)
  const probeByKey = new Map<string, { ok: boolean; ms: number; error: string }>();
  for (const o of probes) {
    probeByKey.set(`${o.kind ?? "lan"}\u0000${o.address}`, { ok: !!o.ok, ms: Number(o.ms ?? 0), error: String(o.error ?? "") });
    if (o.pair_id) probeByKey.set(`${o.kind ?? "lan"}\u0000${o.pair_id}`, { ok: !!o.ok, ms: Number(o.ms ?? 0), error: String(o.error ?? "") });
  }
  const badgeOf = (st?: { ok: boolean; ms: number; error: string }) => {
    const skipped = !!st && !st.ok && /未测出|未探测/.test(st.error || "");
    // 延迟档位与「服务器」页同一套阈值（80 / 300）。以前只有 ok/bad 两态，
    // 200ms 和 20ms 一个颜色，看不出通道快慢。
    // 失败也归「未测出」（灰）——与移动端统一；不再出现"不通"。
    // "通"仍按延迟分档：ok <80 绿 / warn <300 黄 / bad ≥300 红。
    const tone: ConnectLine["tone"] = !st ? "" : !st.ok ? "bad"
      : st.ms < 80 ? "ok" : st.ms < 300 ? "warn" : "bad";
    return {
      ok: !!st?.ok, ms: st?.ms ?? 0, skipped, tone,
      badge: st?.ok ? `${st.ms} ms` : (st ? t("hub.conn.fail") : ""),
      reason: !st || st.ok ? "" : (st.error || t("hub.conn.fail")),
    };
  };
  const out: ConnectLine[] = [];
  // 1) 直连行：route_disabled/hidden 命中 lan → 整行不上；每地址一行
  if (!disabled.has("lan") && !hidden.has("lan") && addresses.length) {
    for (const addr of addresses) {
      const b = badgeOf(probeByKey.get(`lan\u0000${addr}`));
      out.push({ dataK: "lan", kindLabel: t(ROUTE_LABELS.lan), address: addr, ok: b.ok, ms: b.ms, skipped: b.skipped,
        badge: b.badge, reason: b.reason, macs: macsMap[addr] ?? [], isLan: true, tone: b.tone,
        url: `http://${addr}/?token=${encodeURIComponent(rowTok)}` });
    }
  }
  // 2) 配对行：每个配对一条(通道停用/展示关/pair.show_probe=false → 不上)
  for (const p of pairs) {
    if (!p.addr) continue;
    const lane = p.lane || (String(p.addr).includes("ts.net") ? "funnel" : "direct");
    const kind = p.kind === "cloudflare" ? "tunnel" : p.kind === "tsnet" ? "tsnet" : (p.kind === "ts" ? (lane === "funnel" ? "funnel" : "ts") : (p.kind || "tunnel"));
    if (disabled.has(kind) || hidden.has(kind) || p.show_probe === false) continue;
    const b = badgeOf(probeByKey.get(`${kind}\u0000${p.addr}`) || (p.id ? probeByKey.get(`${kind}\u0000${p.id}`) : undefined));
    out.push({ dataK: kind, kindLabel: ROUTE_LABELS[kind] ? t(ROUTE_LABELS[kind]) : (p.kind || t("hub.route.remote")), address: String(p.addr), ok: b.ok, ms: b.ms,
      skipped: b.skipped, badge: b.badge, reason: b.reason, macs: [], isLan: false, tone: b.tone,
      url: `http://${p.addr}/?token=${encodeURIComponent(p.token ? String(p.token) : rowTok)}` });
  }
  return out;
});
// 复制 / 浏览器打开（照 v0.2.5 cqLanActs；url 已按行令牌拼好）
const toast = useToastStore();
/** 复制到剪贴板：优先 navigator.clipboard，失效(http非安全上下文/旧 webview)回退 execCommand。 */
function copyToClipboard(text: string): boolean {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      void navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
      return true;
    }
  } catch { /* 落到 fallback */ }
  return fallbackCopy(text);
}
function fallbackCopy(text: string): boolean {
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.focus();
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch { return false; }
}
function copyUrl(url: string, addr: string) {
  const ok = copyToClipboard(url);
  toast.show(ok ? t("hub.toast.copied", { addr }) : t("hub.toast.copyFail", { url }), ok ? "ok" : "bad");
}
async function openUrl(url: string) {
  try { const u = new URL(url); u.searchParams.set("abLang", currentLocale()); url = u.toString(); } catch { /* 保底原样 */ }
  const { isTauri } = await import("@/api/tauri");
  if (!isTauri()) {
    // 网页/移动端：新标签（popup 可能被拦，提示一下）
    const w = window.open(url, "_blank");
    if (!w) toast.show(t("hub.toast.popupBlocked"), "bad");
    return;
  }
  const { call } = await import("@/api/tauri");
  try {
    await call("remote_open_page", { url });
    toast.show(t("hub.toast.opened"));
  } catch (e) {
    toast.show(t("hub.toast.openFail", { msg: String(e) }), "bad");
  }
}

/** 所有通道（6 个引导器）统一顺序：refind/refindplus/grub + 检测到的 sdboot/opencore/clover */
const instanceAliases = ref<Record<string, string>>((() => {
  try { return JSON.parse(localStorage.getItem("anyboot.instanceAliases") || "{}"); } catch { return {}; }
})());
const hiddenInstances = ref<string[]>([]);
try {
  hiddenInstances.value = JSON.parse(localStorage.getItem("anyboot.hiddenInstances") || "[]");
} catch { /* 忽略 */ }
const localInstances = ref<any[]>([]);
const channelGroups = computed<any[]>(() => {
  if (isLocal.value && localInstances.value.length) return localInstances.value;
  return props.info?.channels ?? [];
});
onMounted(async () => {
  if (!isLocal.value) return;
  try {
    const result: any = await call("boot_channels_all");
    if (Array.isArray(result) && result.length) localInstances.value = result;
  } catch {
    try {
      const token = new URLSearchParams(location.search).get("token") || "";
      const response = await fetch(`/api/boot-channels?token=${encodeURIComponent(token)}`);
      const data = await response.json();
      if (Array.isArray(data) && data.length) localInstances.value = data;
    } catch { /* 忽略 */ }
  }
});

function persistInstances() {
  try {
    localStorage.setItem("anyboot.instanceAliases", JSON.stringify(instanceAliases.value));
    localStorage.setItem("anyboot.hiddenInstances", JSON.stringify(hiddenInstances.value));
  } catch { /* 忽略 */ }
}
function instanceKey(group: any): string {
  return String(group?.instanceId || group?.kind || "");
}
function instanceTitle(group: any): string {
  const key = instanceKey(group);
  return instanceAliases.value[key] || group?.title || group?.kind || "";
}
const allKinds = computed<string[]>(() => {
  const all = channelGroups.value;
  const instanceKeys = all
    .filter((c: any) => !!c.instanceId)
    .filter((c: any) => !c.hidden && !hiddenInstances.value.includes(String(c.instanceId)))
    .map((c: any) => String(c.instanceId));
  if (instanceKeys.length > 0) return instanceKeys; // 多实例：完全平铺
  return ["refind", "refindplus", "grub", "sdboot", "opencore", "clover"];
});
/** 右键实例：重命名（留空 = 隐藏） */
function instanceMenu(kind: string) {
  const group: any = channelOf(kind);
  if (!group) return;
  const key = instanceKey(group);
  const current = instanceAliases.value[key] || group.title || kind;
  const value = window.prompt(`${t("hub.boot.col", { name: current })}\n（留空 = 隐藏该实例）`, current);
  if (value === null) return;
  const name = value.trim();
  if (!name) {
    if (!hiddenInstances.value.includes(key)) hiddenInstances.value = [...hiddenInstances.value, key];
  } else {
    instanceAliases.value = { ...instanceAliases.value, [key]: name };
  }
  persistInstances();
}
/** 统一回显：通道走 channelEcho，其余走 bootState —— 只有数据来源差异，无 UI 特例 */
function bootEcho(kind: string): { label: string; dot: string; none: boolean; title: string } {
  const g: any = channelOf(kind);
  const fallbackTitle = ({ refind: "rEFInd", refindplus: "RefindPlus", grub: "GRUB", sdboot: "systemd-boot", opencore: "OpenCore", clover: "Clover" } as any)[kind] ?? kind;
  const title = g ? instanceTitle(g) : fallbackTitle;
  if (g && g.available && (g.options ?? []).length === 0 && g.note) {
    return { label: String(g.note), dot: "", none: false, title };
  }
  if (g && g.instanceId) {
    if (!g.available) return { label: t("hub.state.notDetected"), dot: "", none: true, title };
    const opts: any[] = (g.options ?? []) as any[];
    const cur = String(g.current ?? "").trim();
    let hitIndex = opts.findIndex((o: any) => !o.header && o.value === cur);
    if (hitIndex < 0 && cur) {
      hitIndex = opts.findIndex((o: any) => !o.header && String(o.label ?? "").includes(cur));
    }
    const hit = hitIndex >= 0 ? opts[hitIndex] : undefined;
    let label = hit?.label || cur || t("hub.state.notSelected");
    if (hit?.child && hitIndex > 0) {
      for (let k = hitIndex - 1; k >= 0; k -= 1) {
        if (opts[k].header) {
          const parent = String(opts[k].label ?? "")
            .replace(/^[▾▸\s]+/, "")
            .replace(/（\d+\s*(项|个内核)[^）]*）\s*$/, "")
            .trim();
          if (parent) label = `${parent} › ${label}`;
          break;
        }
      }
    }
    return { label, dot: hit?.class || "", none: false, title };
  }
  if (kind === "sdboot" || kind === "opencore" || kind === "clover" || kind.startsWith("channel:")) {
    const e = channelEcho(kind);
    return { label: e.label, dot: e.dot, none: !g?.available, title };
  }
  const s = bootState(kind);
  return { label: s.label, dot: s.dot, none: s.none, title };
}
/** 统一切换可用性 */
function bootUsableFor(kind: string): boolean {
  const group: any = channelOf(kind);
  if (group && group.instanceId) return !!(group.available && (group.options ?? []).length);
  if (kind === "sdboot" || kind === "opencore" || kind === "clover" || kind.startsWith("channel:")) {
    const c: any = channelOf(kind);
    return !!(c && c.available && (c.options?.length));
  }
  return bootUsable(kind as "refind" | "refindplus" | "grub");
}
function bootUsable(kind: "refind" | "refindplus" | "grub") {
  const group = props.info?.[kind];
  return Boolean(group?.available) && online.value;
}

/** 电源键定义（图标 + 标签 + 提示，对齐旧版 CQ_POWER） */
const POWER = [
  { act: "wake", label: "hub.power.wake", hint: isLocal.value ? "hub.power.localNoWake" : "hub.power.magic", icon: "power" },
  { act: "sleep", label: "hub.power.sleep", hint: "hub.power.hibernate", icon: "moon" },
  { act: "reboot", label: "hub.power.restart", hint: isLocal.value ? "" : "HTTP", icon: "refresh" },
  { act: "shutdown", label: "hub.power.shutdown", hint: isLocal.value ? "" : "HTTP", icon: "shutdown" },
] as const;

/** 电源图标（对齐旧版 CQ_POWER 的 svg） */
const POWER_ICONS: Record<string, string> = {
  wake: '<path d="M12 3v9"/><path d="M6.4 6.4a8 8 0 1 0 11.2 0"/>',
  sleep: '<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/>',
  reboot: '<path d="M21 12a9 9 0 1 1-2.6-6.4"/><path d="M21 3v6h-6"/>',
  shutdown: '<path d="M12 3v9"/><path d="M18 6a9 9 0 1 1-12 0"/>',
};

const powerHints = computed(() =>
  POWER.map((spec) => ({
    ...spec,
    hint:
      spec.act === "wake" && !isLocal.value
        ? online.value
          ? t("hub.power.magic")
          : t("hub.power.noMac")
        : (spec.hint ? t(spec.hint) : spec.hint),
  })),
);

/** 该行 info.channels 里对应 kind 的通道（无则 undefined） */
/** 平台图标：唯一实现在 bootRows.ts 的 platformIcon（此壳为模板调用 convenience）。 */
function osIcon(kind: string, name: string): string {
  return platformIcon(kind, name);
}

function channelOf(kind: string) {
  const list = channelGroups.value;
  return (
    list.find((c: any) => (c.instanceId && c.instanceId === kind) || (!c.instanceId && c.kind === kind)) ||
    list.find((c: any) => c.kind === kind)
  );
}
/** 通道列回显：current 是归一后的切换值域（opencore-default:… 裸 spec），直接渲染
 *  会把 spec 原串印出来。照主界面（系统引导页）的口径——显示**命中的条目名**：
 *  按 value 精确命中，再按标题互相包含兜底；都不中才原样显示。 */
function channelEcho(kind: string): { label: string; dot: string } {
  const ch = channelOf(kind);
  if (!ch || !ch.available) return { label: t("hub.state.notDetected"), dot: "" };
  const opts = ((ch.options ?? []) as { value?: string; label?: string; header?: boolean; class?: string }[]);
  if (!opts.length) return { label: t("hub.state.notSelected"), dot: "" };
  const cur = String(ch.current || "");
  const hit =
    opts.filter((o) => !o.header && o.value === cur)[0] ||
    opts.filter((o) => {
      if (o.header || !cur) return false;
      const text = String(o.label || "").trim();
      return !!text && (text === cur || text.indexOf(cur) >= 0 || cur.indexOf(text) >= 0);
    })[0];
  return { label: hit?.label || cur || t("hub.state.notSelected"), dot: hit?.class || "" };
}
function onRowClick() {
  emit("select", props.row.id);
}
</script>

<template>
  <div
    class="cq-row"
    :class="{ 'cq-selected': selected }"
    :data-rid="row.id"
    role="row"
    @click="onRowClick"
  >
    <!-- 头部行 -->
    <div class="cq-row-head">
      <div class="cq-col cq-col-dev" role="cell">
        <div v-if="sortMode === 'custom'" class="cq-move">
          <template v-if="!isLocal">
            <button
              type="button"
              :title="t('hub.col.moveUp')"
              :disabled="orderIndex <= 0"
              @click.stop="emit('move', { id: row.id, delta: -1 })"
            >
              ▴
            </button>
            <button
              type="button"
              :title="t('hub.col.moveDown')"
              :disabled="orderIndex >= orderTotal - 1"
              @click.stop="emit('move', { id: row.id, delta: 1 })"
            >
              ▾
            </button>
          </template>
          <span v-else style="width: 18px" />
        </div>

        <div class="cq-dev">
          <div class="cq-dev-ico">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
              <rect x="2" y="4" width="20" height="13" rx="2" />
              <path d="M8 21h8M12 17v4" />
            </svg>
          </div>
          <div class="cq-dev-txt">
            <div class="cq-dev-name">
              <span class="nm">{{ row.name || (row.id === "local" ? t("hub.home.local") : "") }}</span>
              <button
                v-if="!isLocal && osBadge"
                type="button"
                class="os-badge"
                :data-os="osBadge.kind"
                :title="t('hub.col.sysinfo')"
                @click.stop="emit('sysInfo', row.id)"
              >
                {{ osBadge.text }}
              </button>
              <span v-if="selected" class="cq-chip-cur">{{ t("hub.state.current") }}</span>
              <span
                class="cq-chip-grp"
                :class="{ 'cq-chip-remote': false }"
                :title="connLabel === t('hub.conn.direct') ? t('hub.conn.lanDirect') : t('hub.conn.notReachable')"
              >
                {{ connLabel }}
              </span>
            </div>

          </div>
        </div>
      </div>

      <!-- 状态（旧 cqStateCell：i 圆点 + 在线/离线 + 本机地址 small） -->
      <div class="cq-col cq-col-st" role="cell">
        <span class="cq-st" :class="stateClass">
          <i />
          {{ statusLabel }}
          <small v-if="isLocal && online && info?.addr">{{ info.addr }}</small>
        </span>
      </div>

      <!-- 更多 -->
      <div class="cq-col cq-col-more" role="cell">
        <button
          type="button"
          class="cq-more"
          :title="t('hub.col.more')"
          :aria-label="t('hub.col.more')"
          @click.stop="emit('more', row.id)"
        >
          <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <circle cx="12" cy="5" r="1.7" />
            <circle cx="12" cy="12" r="1.7" />
            <circle cx="12" cy="19" r="1.7" />
          </svg>
        </button>
      </div>
    </div>

    <div v-if="connectLines.length" class="cq-boot-meta" style="display:flex;flex-direction:column;gap:5px;width:100%">
      <div v-for="(ln, i) in connectLines" :key="i" class="cq-dev-line">
        <div class="cq-dev-top">
          <span class="cq-remote-kind" :data-k="ln.dataK">{{ ln.kindLabel }}</span>
          <span class="cq-probe-ms" :class="ln.tone" :title="ln.ok ? t('hub.probe.passed') : (ln.skipped ? t('hub.probe.skipped') : t('hub.probe.failed', { reason: ln.reason || t('hub.conn.fail') }))">{{ ln.badge }}</span>
          <button type="button" class="cq-remote-act" :title="t('hub.conn.copyTitle')" @click.stop="copyUrl(ln.url, ln.address)">{{ t("common.copy") }}</button>
          <button type="button" class="cq-remote-act" :title="t('hub.conn.openTitle')" @click.stop="openUrl(ln.url)">{{ t("hub.conn.openBrowser") }}</button>
        </div>
        <span class="cq-dev-addr" :title="ln.address">{{ ln.address }}</span>
        <div v-if="ln.macs && ln.macs.length" class="cq-dev-macrow">
          <span v-for="(m, j) in ln.macs" :key="j" class="cq-dev-mac">{{ m }}</span>
        </div>
        <span v-if="ln.reason" class="cq-dev-reason">{{ ln.reason }}</span>
      </div>
    </div>


    <!-- 引导项区：所有引导器统一为「独立通道」，同一个按钮/同一个下拉，无特例 -->
    <div class="cq-row-boots">
      <div
        v-for="kind in allKinds"
        :key="kind"
        class="cq-col cq-col-boot"
        :class="{ 'cq-col-boot--ch': (kind === 'sdboot' || kind === 'opencore' || kind === 'clover') }"
        role="cell"
        :data-label="t('hub.boot.col', { name: bootEcho(kind).title })"
      >
        <div class="cq-boot" :class="{ 'cq-boot--none': bootEcho(kind).none }">
          <button
            type="button"
            class="cq-boot-btn"
            :disabled="!bootUsableFor(kind)"
            @click.stop="openPicker(kind)"
            @contextmenu.prevent.stop="instanceMenu(kind)"
          >
            <span class="cq-boot-name">{{ bootEcho(kind).title }}</span>
            <span v-if="osIcon(bootEcho(kind).dot, bootEcho(kind).label)">{{ osIcon(bootEcho(kind).dot, bootEcho(kind).label) }}</span>
            <span class="t">{{ bootEcho(kind).label }}</span>
          </button>
        </div>
      </div>
    </div>

    <!-- 连接通道连通性（旧版 cq-dev-line 逐地址/通道探测明细）：受「是否展示连通性」控制 -->
    <!-- 电源区 -->
    <div class="cq-row-pwr">
      <div class="cq-col cq-col-pwr" role="cell">
        <div class="cq-pwr">
          <button
            v-for="spec in powerHints"
            :key="spec.act"
            type="button"
            class="cq-pwr-btn"
            :title="spec.hint ? `${t(spec.label)} · ${t(spec.hint)}` : t(spec.label)"
            :aria-label="spec.hint ? `${t(spec.label)} · ${t(spec.hint)}` : t(spec.label)"
            @click.stop="emit('power', { serverId: row.id, action: spec.act })"
          >
            <span class="lbl">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
                   v-html="POWER_ICONS[spec.act]" />
              {{ t(spec.label) }}
            </span>
            <span class="hint">{{ t(spec.hint) }}</span>
          </button>
        </div>
      </div>
    </div>

    <!-- 启动项选择器：点启动项列弹出该引导器的可选条目，选中带 value 切换下次启动 -->
    <div v-if="picker" class="cq-picker-overlay" @click.self="picker = null">
      <div class="cq-picker" role="dialog" aria-modal="true" :aria-label="t('hub.picker.ariaTitle', { title: picker.title })">
        <div class="cq-picker-head">{{ t("hub.picker.title", { loader: picker.title }) }}</div>
        <div v-if="isGrubPicker" class="cq-picker-fmt">
          <span class="cq-picker-fmt-label">{{ t("hub.picker.fmtTitle") }}</span>
          <div class="cq-picker-fmt-chips">
            <button v-for="c in fmtChoices" :key="c.v" type="button"
              :class="['cq-fmt-chip', { 'is-on': pickerFmt === c.v }]"
              @click="pickerFmt = c.v">{{ t(c.label) }}</button>
          </div>
        </div>
        <div class="cq-picker-list">
          <GrubList
            :format="pickerFmt || appSettings.grubListFormat"
            :entries="picker.entries"
            :active-selector="picker.active"
            :group-titles="picker.groupTitles"
            :show-groups="!picker.flat"
            @select="chooseBoot"
          />
        </div>
        <button type="button" class="cq-picker-cancel" @click="picker = null">{{ t("common.cancel") }}</button>
      </div>
    </div>
  </div>
</template>
