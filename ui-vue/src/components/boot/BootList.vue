<script setup lang="ts">
import { computed, ref } from "vue";
import GrubList from "./GrubList.vue";
import type { EntryData } from "./types";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import { useToastStore } from "@/stores/toast";
import { call } from "@/api/tauri";
import type { ChannelView, GrubEntry } from "@/api/types";
import { simRows } from "@/pages/boot/bootRows";

/**
 * 一个引导器区块（rEFInd / RefindPlus / GRUB / systemd-boot / OpenCore / Clover）的列表。
 *
 * ## 最重要的行为：按 auto / manual 模式取不同数据源
 *
 * 后端快照里**两套数据并存**（2026-11-02 用户明确指出过这一点）：
 *
 * | 引导器 | 自动模式 | 手动模式 |
 * |---|---|---|
 * | rEFInd | `refindSim`（按扫描规则重算的菜单，含子屏） | `status.variables`（vars 快照） |
 * | RefindPlus | `refindplusSim`（同一引擎的独立通道槽位） | `refindplusVariables`（手动获取的 vars 快照） |
 * | GRUB | `linuxGrub`（Linux 分区自己的 grub.cfg） | `grub`（ESP 上的 grub.cfg） |
 *
 * 选错数据源的表现是「列表显示的条目与引导器里实际看到的不一致」。
 * PC / Android / iOS 三端都已按这个口径修过，这里保持一致。
 */

const props = defineProps<{
  /** refind / refindplus / grub / sdboot / opencore / clover */
  loader: string;
}>();

const ui = useUiStateStore();
const settings = useSettingsStore();
const toast = useToastStore();

const isRefind = computed(() => props.loader === "refind");
/** RefindPlus：与 rEFInd 同一套列表逻辑的独立通道（数据源/命令只差 channel） */
const isRefindPlus = computed(() => props.loader === "refindplus");
const isRefindFamily = computed(() => isRefind.value || isRefindPlus.value);
const isGrub = computed(() => props.loader === "grub");
const isChannel = computed(() => !isRefindFamily.value && !isGrub.value);

/** 本通道的读取方式：refind / refindplus 各自的 Settings 字段 */
const refindMode = computed(() =>
  isRefindPlus.value ? settings.refindplusMode : settings.refindMode,
);
/** 本通道的数据源：自动=各自的模拟扫描；手动=各自的 vars 快照 */
const refindSimSource = computed(() => (isRefindPlus.value ? ui.refindplusSim : ui.refindSim));
const refindVarsSource = computed(() =>
  isRefindPlus.value ? ui.refindplusVariables : ui.status?.variables,
);

/** 点击引导项 = 切换到该条为下次启动（自包含：各主题卡片直接可用，回显靠 IPC 快照） */
async function switchEntry(selector: string) {
  if (!selector) return;
  try {
    if (isRefindFamily.value) {
      if (refindMode.value === "manual") {
        await call("switch_system", { system: selector, channel: props.loader });
      } else {
        // 旧版语义：只接受当前可见模拟条目的标题原文（组头子项本来就没有点击处理器）
        const visible = (refindSimSource.value?.entries ?? []).filter((e) => !e.hidden).map((e) => e.title);
        if (!visible.includes(selector)) return;
        await call("refind_sim_switch", { title: selector, channel: props.loader });
      }
    } else if (isGrub.value) {
      if (settings.grubMode === "manual") await call("switch_grub", { selector });
      else await call("linux_grub_switch", { selector });
    } else {
      // 旧版 bootChannelSpec（boot-list-view.js:614，与后端 parse_switch_spec 值域一致）：
      // 预演行自带完整 spec；sdboot → `sdboot:<id>`；opencore 动作 id 本身是值域；
      // clover 动作 → `clover-oneshot`。裸 id 后端不认。
      const ch = channel.value;
      let spec = selector;
      if (ch) {
        if (ch.id === "sdboot") {
          const hit = (ch.entries ?? []).find((r) => (r.id ?? r.title) === selector);
          spec = `sdboot:${hit?.id ?? selector}`;
        } else if (ch.id === "clover") {
          spec = "clover-oneshot";
        } else if (ch.id === "opencore" && !(ch.menuPreview?.entries ?? []).length) {
          spec = selector; // opencore 动作 id（opencore / opencore-register）
        }
      }
      // title = 选中条目的完整标题原文（不省略任何字），后端据此统一「下次启动：<title>」回执
      const rowTitle =
        entries.value.find((e) => (e.selector ?? "") === selector)?.name ??
        (ch?.menuPreview?.entries ?? []).find((r) => (r.spec || r.title) === selector)?.title ??
        selector;
      await call("switch_boot_channel", { target: spec, title: rowTitle });
    }
  } catch (e) {
    toast.show("切换失败：" + e, "bad");
  }
}


/** 该通道的数据（三通道时才有） */
const channel = computed<ChannelView | null>(
  () => ui.channels.find((item) => item.id === props.loader) ?? null,
);

/** GRUB 相关：按模式选数据源 */
// 2026-10-31 定案：GRUB 零回落——自动只认 linuxGrub，手动只认 ESP grub，
// 列表与点击命令同源（自动→linux_grub_switch，手动→switch_grub），否则点了没反应。
const grubSource = computed(() => (settings.grubMode === "manual" ? ui.grub : ui.linuxGrub));

/** rEFInd 家族（refind/refindplus）：自动模式看模拟扫描，手动模式看各自的 vars 快照 */
const refindEntries = computed<EntryData[]>(() => {
  if (!isRefindFamily.value) return [];
  const manual = refindMode.value === "manual";

  if (!manual) {
    // 自动模式：交给 simRows —— 它会把「同一引导分区折叠的内核」（children，
    // source=kernel）展开成可点子项（selector=父条目 title，能切到该分区条目），
    // 并处理组头。之前这里内联平铺 entries、忽略 children，导致「内核全挤在一级菜单」
    // （用户：怎么我看还是全都在一级菜单）。
    return simRows(refindSimSource.value);
  }

  // 手动模式：vars 快照里的三个系统项
  // 旧版 app.js:1278：行点击 switch_system {system: item.system}（系统类别，
  // 不是 source）；active 行不可点（switchable = exists && !busy && !error && !active）
  const vars = refindVarsSource.value ?? [];
  return vars
    .filter((item) => item.exists)
    .map((item) => ({
      selector: item.system,
      name: item.displayName,
      class: item.system,
      readonly: Boolean(item.active),
    }));
});

/** 通道「当前项」命中的 selector（applyChannelDefault 回填） */
const channelDefaultSelector = ref("");
/** 当前选中项 */
const activeSelector = computed(() => {
  if (isRefindFamily.value) {
    // 自动模式：lastBoot 命中的那条（与托盘 / PC 主窗同一口径）
    if (refindMode.value !== "manual") {
      const sim = refindSimSource.value;
      const hit = sim?.entries?.find((entry) => entry.lastBoot);
      // 原文返回（selector 也是原文，两者必须相等才能高亮回显）
      return hit ? hit.title : "";
    }
    const active = refindVarsSource.value?.find((item) => item.active);
    // 与行 selector 同口径（system 类别）才能命中高亮
    return active ? active.system : "";
  }
  if (isGrub.value) {
    // 自动模式看 linuxGrub 的 nextEntry，手动看 ESP 的
    const source = grubSource.value;
    return source?.nextEntry || source?.savedEntry || "";
  }
  // 三通道：走 applyChannelDefault 算出的命中行 selector（三来源语义，同旧版）
  return channelDefaultSelector.value || "";
});

/** 组标题映射（submenu id → 标题） */
const groupTitles = computed(() => {
  const map: Record<string, string> = {};
  if (!isGrub.value) return map;
  const source = grubSource.value;
  for (const group of source?.groups ?? []) {
    map[group.id ?? ""] = group.name ?? "";
  }
  return map;
});

/** GRUB 条目转成通用 EntryData */
const grubEntries = computed<EntryData[]>(() => {
  const source = grubSource.value;
  if (!isGrub.value || !source?.entries) return [];
  return source.entries.map((entry: GrubEntry) => ({
    selector: entry.selector,
    name: entry.name,
    class: entry.class,
    kernels: entry.kernels,
    variants: entry.variants,
    chainloader: entry.chainloader,
    submenu: entry.submenu,
  }));
});

// 旧版 boot-list-view.js 的 OC 菜单标签映射
const OC_KIND_LABEL: Record<string, string> = {
  macos: "macOS", windows: "Windows", generic: "泛型", linux: "Linux", tool: "工具项", unknown: "未知",
};
const OC_SOURCE_LABEL: Record<string, string> = {
  bootorder: "固件 BootOrder", bless: "bless 扫描", preboot: "APFS Preboot", protocol: "协议驱动",
};
function ocSourceLabel(source: string, orderSource: string): string {
  if (source === "bootorder") return orderSource === "private" ? "OC 私有 OCBtOrder" : "固件 BootOrder";
  return OC_SOURCE_LABEL[source] || source;
}
/** 三通道的条目（OpenCore 的动作行在 menuPreview 里） */
const channelEntries = computed<EntryData[]>(() => {
  if (!isChannel.value) return [];
  const ch = channel.value;
  if (!ch) return [];
  // OpenCore 的顶层 entries 恒为空，动作全在 menuPreview
  const preview = ch.menuPreview?.entries ?? [];
  const orderSource = ch.menuPreview?.orderSource ?? "";
  if (preview.length) {
    // **固定排序：可选中项一组在前，不可选中项一组在后。**
    // 只按「能不能选」（row.spec 非空 = 可写回 OCBt* 设默认）分组，
    // **与是否被选中无关** —— 选中只加 class 高亮，绝不改变任何条目的顺序，
    // 所以顺序是恒定的，不会「一选就变」。两组内部各自保持 OC 原本 BootOrder 顺序。
    // （列表顶端那个 sticky 置顶副本由 GrubList 单独渲染，不参与这里的排序。）
    const ordered = [
      ...preview.filter((row) => row.spec),
      ...preview.filter((row) => !row.spec),
    ];
    const rows = ordered.map((row) => ({
      // 标识用 spec，空则退 title（旧版 ocEntrySelector 兜底）——default 行 spec
      // 常为空，selector 空串会让高亮匹配失效
      selector: row.spec || row.title,
      // 旧版 ocMenuRow：entry.spec 为空的行不挂 is-clickable（不可点）
      readonly: !row.spec,
      name: row.title,
      class: row.kind,
      submenu: null,
      // 旧版 ocMenuRow：类型标签 + 来源标签 + 卷·路径 + 备注
      tags: [OC_KIND_LABEL[row.kind] || row.kind, ocSourceLabel(row.source, orderSource)].filter(Boolean),
      sub: row.volume && row.volume !== "—" ? `${row.volume} · ${row.path}` : row.path,
      note: row.note || undefined,
    }));
    // ① entry.default（OC 的 OCBtOrder 落在预演行 default 上）等三来源判定
    applyChannelDefault(
      rows,
      ordered.map((row) => (row as { default?: boolean }).default),
      ch,
    );
    return rows;
  }
  const source = ch.entries ?? [];
  const rows = source.map((row) => ({
    selector: String(row.id ?? row.title ?? ""),
    name: String(row.title ?? row.id ?? ""),
    class: String(row.kind ?? ""),
    submenu: null,
  }));
  applyChannelDefault(rows, source.map((row) => (row as { default?: boolean }).default), ch);
  return rows;
});
/**
 * 通道「当前项」三种来源（旧版 boot-list-view.js:496，与后端
 * channel_entry_is_default / OC currentDefault 兜底同一套语义）：
 *   ① entry.default === true（OC 的 OCBtOrder 落在预演行 default 上）
 *   ② currentDefault === entry.id
 *   ③ currentDefault 是一段包含标题的串
 * 命中行即「当前项」——activeSelector 用它的 selector（strict 相等才能
 * 触发 GrubList 的置顶副本 + .active 高亮）。
 */
function applyChannelDefault(
  rows: EntryData[],
  defaults: unknown[],
  ch: ChannelView,
): void {
  const cur = (ch.currentDefault || "").toString();
  const hit = rows.find(
    (row, index) =>
      defaults[index] === true ||
      (cur && (cur === row.selector || cur.includes(row.name))),
  );
  if (hit?.selector) channelDefaultSelector.value = hit.selector;
}

/** 最终要渲染的条目 */
const entries = computed<EntryData[]>(() => {
  if (isRefindFamily.value) return refindEntries.value;
  if (isGrub.value) return grubEntries.value;
  return channelEntries.value;
});

/** 三通道不显示组头（OC 的 menuPreview 是平铺的） */
const showGroups = computed(() => !isChannel.value);

/** 有动作在跑时禁用 */
const disabled = computed(() => Boolean(ui.busy?.switching));

const emit = defineEmits<{
  (e: "select", selector: string): void;
}>();
</script>

<template>
  <div class="boot-list-block">
    <GrubList
      :entries="entries"
      :active-selector="activeSelector"
      :group-titles="groupTitles"
      :show-groups="showGroups"
      :disabled="disabled"
      @select="(selector) => { switchEntry(selector); emit('select', selector); }"
    />
    <p v-if="!entries.length" class="boot-list-empty">没有可用条目</p>
  </div>
</template>

<style scoped>
.boot-list-empty {
  font-size: 12px;
  color: var(--fg-faint, #9ca3af);
  padding: 10px 4px;
  margin: 0;
}
</style>
