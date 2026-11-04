<script setup lang="ts">
import { computed } from "vue";
import GrubList from "./GrubList.vue";
import type { EntryData } from "./types";
import { useUiStateStore } from "@/stores/uiState";
import { useSettingsStore } from "@/stores/settings";
import type { ChannelView, GrubEntry, SimEntry } from "@/api/types";

/**
 * 一个引导器区块（rEFInd / GRUB / systemd-boot / OpenCore / Clover）的列表。
 *
 * ## 最重要的行为：按 auto / manual 模式取不同数据源
 *
 * 后端快照里**两套数据并存**（2026-11-02 用户明确指出过这一点）：
 *
 * | 引导器 | 自动模式 | 手动模式 |
 * |---|---|---|
 * | rEFInd | `refindSim`（按扫描规则重算的菜单，含子屏） | `status.variables`（vars 快照） |
 * | GRUB | `linuxGrub`（Linux 分区自己的 grub.cfg） | `grub`（ESP 上的 grub.cfg） |
 *
 * 选错数据源的表现是「列表显示的条目与引导器里实际看到的不一致」。
 * PC / Android / iOS 三端都已按这个口径修过，这里保持一致。
 */

const props = defineProps<{
  /** refind / grub / sdboot / opencore / clover */
  loader: string;
}>();

const ui = useUiStateStore();
const settings = useSettingsStore();

const isRefind = computed(() => props.loader === "refind");
const isGrub = computed(() => props.loader === "grub");
const isChannel = computed(() => !isRefind.value && !isGrub.value);

/** 该通道的数据（三通道时才有） */
const channel = computed<ChannelView | null>(
  () => ui.channels.find((item) => item.id === props.loader) ?? null,
);

/** GRUB 相关：按模式选数据源 */
const grubSource = computed(() => {
  if (!isGrub.value) return null;
  const manual = settings.grubMode === "manual";
  return manual ? ui.grub : (ui.linuxGrub ?? ui.grub);
});

/** rEFInd：自动模式看模拟扫描，手动模式看 vars 快照 */
const refindEntries = computed<EntryData[]>(() => {
  if (!isRefind.value) return [];
  const manual = settings.refindMode === "manual";

  if (!manual) {
    const sim = ui.refindSim;
    if (!sim?.entries) return [];
    return sim.entries
      .filter((entry) => !entry.hidden)
      .map((entry: SimEntry) => ({
        selector: entry.title.trim(),
        name: entry.title.trim(),
        class: entry.kind,
      }));
  }

  // 手动模式：vars 快照里的三个系统项
  const vars = ui.status?.variables ?? [];
  return vars
    .filter((item) => item.exists)
    .map((item) => ({
      selector: item.source,
      name: item.displayName,
      class: item.system,
    }));
});

/** 当前选中项 */
const activeSelector = computed(() => {
  if (isRefind.value) {
    // 自动模式：lastBoot 命中的那条（与托盘 / PC 主窗同一口径）
    if (settings.refindMode !== "manual") {
      const sim = ui.refindSim;
      const hit = sim?.entries?.find((entry) => entry.lastBoot);
      return hit ? hit.title.trim() : "";
    }
    const active = ui.status?.variables?.find((item) => item.active);
    return active ? active.source : "";
  }
  if (isGrub.value) {
    // 自动模式看 linuxGrub 的 nextEntry，手动看 ESP 的
    const source = grubSource.value;
    return source?.nextEntry || source?.savedEntry || "";
  }
  // 三通道：currentDefault 或 lastBoot
  const ch = channel.value;
  return ch?.currentDefault || ch?.lastBoot || "";
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
    return preview.map((row) => ({
      selector: row.spec,
      name: row.title,
      class: row.kind,
      submenu: null,
      // 旧版 ocMenuRow：类型标签 + 来源标签 + 卷·路径 + 备注
      tags: [OC_KIND_LABEL[row.kind] || row.kind, ocSourceLabel(row.source, orderSource)].filter(Boolean),
      sub: row.volume && row.volume !== "—" ? `${row.volume} · ${row.path}` : row.path,
      note: row.note || undefined,
    }));
  }
  return (ch.entries ?? []).map((row) => ({
    selector: String(row.id ?? row.title ?? ""),
    name: String(row.title ?? row.id ?? ""),
    class: String(row.kind ?? ""),
    submenu: null,
  }));
});

/** 最终要渲染的条目 */
const entries = computed<EntryData[]>(() => {
  if (isRefind.value) return refindEntries.value;
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
      @select="(selector) => emit('select', selector)"
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
