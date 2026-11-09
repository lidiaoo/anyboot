<script setup lang="ts">
import { computed, ref } from "vue";
import type { RemoteProfile } from "@/stores/remote";
import type { RemoteBootInfo } from "@/api/types";

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
 *     .cq-col-boot[data-label="rEFInd 启动项"]
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
const picker = ref<{ kind: string; title: string; options: { value: string; label: string }[] } | null>(null);
function openPicker(kind: string) {
  const group = kind === "refind" ? props.info?.refind : kind === "grub" ? props.info?.grub
    : (props.info?.channels ?? []).find((c) => c.kind === kind);
  const opts = ((group as { options?: { value?: string; label?: string; header?: boolean; fixed?: boolean }[] })?.options ?? [])
    .filter((o) => !o.header && (o.value ?? "").trim() !== "")
    .map((o) => ({ value: String(o.value), label: String(o.label ?? o.value) }));
  if (!opts.length) return; // 没有可选项就别弹
  picker.value = { kind, title: (group as { title?: string })?.title ?? kind, options: opts };
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
  if (loading.value) return "读取中…";
  if (online.value === null) return "未探测";
  return online.value ? "在线" : "离线";
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
const connLabel = computed(() => (isLocal.value ? "本机" : online.value ? "直连" : "未连接"));

/** 引导项值 */
/** 旧 cqBootCell 状态：label + 是否不可改(--none) + 内核点色 k-{class} */
function bootState(kind: string): { label: string; none: boolean; dot: string } {
  const group = kind === "refind" || kind === "grub" ? props.info?.[kind] : undefined;
  if (!group) return { label: online.value === null || loading.value ? "读取中…" : "未检测到", none: true, dot: "" };
  if (!group.available) return { label: "未检测到", none: true, dot: "" };
  if (!online.value) return { label: "离线不可改", none: true, dot: "" };
  const options = (group as { options?: { value?: string; label?: string; class?: string; header?: boolean }[] }).options || [];
  const current = String(group.current || "");
  const hit =
    options.filter((o) => !o.header && o.value === current)[0] ||
    options.filter((o) => {
      if (o.header || !current) return false;
      const text = String(o.label || "").trim();
      return !!text && (text === current || text.indexOf(current) >= 0 || current.indexOf(text) >= 0);
    })[0];
  return {
    label: hit?.label || current || "未选择",
    none: options.length === 0,
    dot: hit?.class || "",
  };
}
/** 连接通道连通性行（旧版 cq-dev-line 逐通道探测明细）：受通道粒度 route_probe_hidden 与
 *  配对级 show_probe 控制——被隐藏的不展示，探测/连接照常。 */
// 通道徽章文案（照抄 v0.2.5 ROUTE_LABELS）
const ROUTE_LABELS: Record<string, string> = {
  lan: "直连", jump: "跳板机", tunnel: "Cloudflare 隧道",
  ts: "Tailscale 直连", funnel: "Tailscale Funnel", tsnet: "Tailscale Funnel（内建）",
};
const connectLines = computed(() => {
  const rows = props.info?.probe_results ?? [];
  const r = props.row as Record<string, unknown>;
  const hidden = new Set<string>((r.route_probe_hidden as string[] | undefined) ?? []);
  const disabled = new Set<string>((r.route_disabled as string[] | undefined) ?? []);
  const pairs = (r.pairs as { id?: string; show_probe?: boolean; addr?: string }[] | undefined) ?? [];
  // 配对级隐藏：按地址匹配更可靠(has_jump 分支的 pair_id 可能为空)，同 kind 下地址唯一。
  const hiddenPairs = pairs.filter((pp) => pp.show_probe === false);
  const hiddenAddrs = new Set(hiddenPairs.map((pp) => String(pp.addr ?? "")).filter(Boolean));
  const hiddenPairIds = new Set(hiddenPairs.map((pp) => String(pp.id ?? "")).filter(Boolean));
  const isPairHidden = (x: { pair_id?: string; address: string }) =>
    (!!x.pair_id && hiddenPairIds.has(x.pair_id)) ||
    (hiddenAddrs.has(x.address)) ||
    hiddenPairs.some((pp) => pp.addr && x.address && (x.address.includes(String(pp.addr)) || String(pp.addr).includes(x.address)));
  return rows
    .filter((x) => !(x.kind && disabled.has(x.kind)))     // 通道停用 → 整行不上（v0.2.5）
    .filter((x) => !(x.kind && hidden.has(x.kind)))       // 通道粒度「展示」关 → 整行不上
    .filter((x) => !isPairHidden(x)) // 配对级隐藏(按 id 或地址)
    .map((x) => {
      const kind = x.kind ?? "lan";
      const skipped = !x.ok && /未测出|未探测/.test(x.error || "");
      const macs = (((props.row as Record<string, unknown>).macs as Record<string, string[]>)?.[x.address]) || [];
      return {
        dataK: kind,
        kindLabel: ROUTE_LABELS[kind] ?? (x.remote ? "远程" : kind || "直连"),
        address: x.address,
        ok: x.ok,
        ms: x.ms,
        skipped,
        macs,
        isLan: kind === "lan",
        // 探测 badge 文案（照抄 v0.2.5 cqProbeBadge）：通=N ms / 未测出 / 不通
        badge: x.ok ? `${x.ms} ms` : skipped ? "未测出" : "不通",
        // 失败原因（v0.2.5 cqProbeReason）：未测出/通过不垫
        reason: x.ok || skipped ? "" : (x.error || "不通"),
      };
    });
});
// 直连地址的复制 / 浏览器打开（照 v0.2.5 cqLanActs；URL 带档案 token）
function addrUrl(addr: string): string {
  const tok = String((props.row as Record<string, unknown>).token ?? "");
  return `http://${addr}/?token=${encodeURIComponent(tok)}`;
}
function copyAddr(addr: string) {
  const url = addrUrl(addr);
  try {
    if (navigator.clipboard?.writeText) void navigator.clipboard.writeText(url);
    else window.prompt("复制直连地址", url);
  } catch { /* ignore */ }
}
function openAddr(addr: string) {
  window.open(addrUrl(addr), "_blank");
}

function bootUsable(kind: "refind" | "grub") {
  const group = props.info?.[kind];
  return Boolean(group?.available) && online.value;
}

/** 电源键定义（图标 + 标签 + 提示，对齐旧版 CQ_POWER） */
const POWER = [
  { act: "wake", label: "唤醒", hint: isLocal.value ? "本机无需唤醒" : "魔术包", icon: "power" },
  { act: "sleep", label: "睡眠", hint: isLocal.value ? "挂起到内存" : "挂起到内存", icon: "moon" },
  { act: "reboot", label: "重启", hint: isLocal.value ? "" : "HTTP", icon: "refresh" },
  { act: "shutdown", label: "关机", hint: isLocal.value ? "" : "HTTP", icon: "shutdown" },
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
          ? "魔术包"
          : "未配 MAC"
        : spec.hint,
  })),
);

/** 该行 info.channels 里对应 kind 的通道（无则 undefined） */
function channelOf(kind: string) {
  return (props.info?.channels ?? []).find((c) => c.kind === kind);
}
/** 通道 kind → 显示名（channel:opencore → OpenCore） */
function channelTitle(kind: string): string {
  const base = kind.startsWith("channel:") ? kind.slice(8) : kind;
  const map: Record<string, string> = { sdboot: "systemd-boot", opencore: "OpenCore", clover: "Clover" };
  return map[base] ?? channelOf(kind)?.title ?? base;
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
              title="上移一位（自定义排序）"
              :disabled="orderIndex <= 0"
              @click.stop="emit('move', { id: row.id, delta: -1 })"
            >
              ▴
            </button>
            <button
              type="button"
              title="下移一位（自定义排序）"
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
              <span class="nm">{{ row.name }}</span>
              <button
                v-if="!isLocal && osBadge"
                type="button"
                class="os-badge"
                :data-os="osBadge.kind"
                title="查看系统信息"
                @click.stop="emit('sysInfo', row.id)"
              >
                {{ osBadge.text }}
              </button>
              <span v-if="selected" class="cq-chip-cur">当前</span>
              <span
                class="cq-chip-grp"
                :class="{ 'cq-chip-remote': false }"
                :title="connLabel === '直连' ? '局域网直连' : '当前没探通 —— 目标机可能离线'"
              >
                {{ connLabel }}
              </span>
            </div>
            <div class="cq-dev-meta">
              <div class="cq-dev-line">
                <div class="cq-dev-top">
                  <span class="cq-dev-addr">{{ info?.addr || row.addresses?.[0] || "" }}</span>
                </div>
              </div>
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
          title="更多操作"
          aria-label="更多操作"
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

    <!-- 引导项区 -->
    <div class="cq-row-boots">
      <div class="cq-col cq-col-boot" role="cell" data-label="rEFInd 启动项">
        <div class="cq-boot" :class="{ 'cq-boot--none': bootState('refind').none }">
          <button
            type="button"
            class="cq-boot-btn"
            :disabled="!bootUsable('refind')"
            @click.stop="openPicker('refind')"
          >
            <span v-if="bootState('refind').dot" class="k" :class="'k-' + bootState('refind').dot" />
            <span class="t">{{ bootState("refind").label }}</span>
          </button>
        </div>
      </div>
      <div class="cq-col cq-col-boot" role="cell" data-label="GRUB 启动项">
        <div class="cq-boot" :class="{ 'cq-boot--none': bootState('grub').none }">
          <button
            type="button"
            class="cq-boot-btn"
            :disabled="!bootUsable('grub')"
            @click.stop="openPicker('grub')"
          >
            <span v-if="bootState('grub').dot" class="k" :class="'k-' + bootState('grub').dot" />
            <span class="t">{{ bootState("grub").label }}</span>
          </button>
        </div>
      </div>
      <!-- 通道列（systemd-boot / OpenCore / Clover）：按「所有电脑检测到的通道并集」渲染，
           保证各行通道列对齐（旧版 cqBootColumns 语义）。某行没检测到该通道 → 该列显示「未检测到」。 -->
      <div
        v-for="kind in (channelKinds ?? [])"
        :key="kind"
        class="cq-col cq-col-boot cq-col-boot--ch"
        role="cell"
        :data-label="`${channelTitle(kind)} 启动项`"
      >
        <div class="cq-boot" :class="{ 'cq-boot--none': !channelOf(kind)?.available }">
          <button
            type="button"
            class="cq-boot-btn"
            :disabled="!channelOf(kind)?.available || !(channelOf(kind)?.options?.length)"
            @click.stop="openPicker(kind)"
          >
            <span class="t">{{ channelOf(kind)?.current || (channelOf(kind)?.available ? "未选择" : "未检测到") }}</span>
          </button>
        </div>
      </div>
    </div>

    <!-- 连接通道连通性（旧版 cq-dev-line 逐地址/通道探测明细）：受「是否展示连通性」控制 -->
    <div v-if="connectLines.length" class="cq-boot-meta" style="display:flex;flex-direction:column;gap:5px;width:100%">
      <div v-for="(ln, i) in connectLines" :key="i" class="cq-dev-line">
        <div class="cq-dev-top">
          <span class="cq-remote-kind" :data-k="ln.dataK">{{ ln.kindLabel }}</span>
          <span class="cq-probe-ms" :class="ln.ok ? 'ok' : (ln.skipped ? '' : 'bad')" :title="ln.ok ? '最近一次探测通过' : (ln.skipped ? '这条通道没轮到探测' : ('最近一次探测失败：' + (ln.reason || '不通')))">{{ ln.badge }}</span>
          <button v-if="ln.isLan" type="button" class="cq-remote-act" title="复制这个局域网地址（含访问令牌）" @click.stop="copyAddr(ln.address)">复制</button>
          <button v-if="ln.isLan" type="button" class="cq-remote-act" title="用系统默认浏览器打开这个局域网地址" @click.stop="openAddr(ln.address)">浏览器打开</button>
        </div>
        <span class="cq-dev-addr" :title="ln.address">{{ ln.address }}</span>
        <div v-if="ln.macs && ln.macs.length" class="cq-dev-macrow">
          <span v-for="(m, j) in ln.macs" :key="j" class="cq-dev-mac">{{ m }}</span>
        </div>
        <span v-if="ln.reason" class="cq-dev-reason">{{ ln.reason }}</span>
      </div>
    </div>

    <!-- 电源区 -->
    <div class="cq-row-pwr">
      <div class="cq-col cq-col-pwr" role="cell">
        <div class="cq-pwr">
          <button
            v-for="spec in powerHints"
            :key="spec.act"
            type="button"
            class="cq-pwr-btn"
            :title="spec.hint ? `${spec.label} · ${spec.hint}` : spec.label"
            :aria-label="spec.hint ? `${spec.label} · ${spec.hint}` : spec.label"
            @click.stop="emit('power', { serverId: row.id, action: spec.act })"
          >
            <span class="lbl">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
                   v-html="POWER_ICONS[spec.act]" />
              {{ spec.label }}
            </span>
            <span class="hint">{{ spec.hint }}</span>
          </button>
        </div>
      </div>
    </div>

    <!-- 启动项选择器：点启动项列弹出该引导器的可选条目，选中带 value 切换下次启动 -->
    <div v-if="picker" class="cq-picker-overlay" @click.self="picker = null">
      <div class="cq-picker" role="dialog" aria-modal="true" :aria-label="`${picker.title} · 选择启动项`">
        <div class="cq-picker-head">{{ picker.title }} · 选择下次启动</div>
        <div class="cq-picker-list">
          <button
            v-for="opt in picker.options"
            :key="opt.value"
            type="button"
            class="cq-picker-item"
            @click="chooseBoot(opt.value)"
          >{{ opt.label }}</button>
        </div>
        <button type="button" class="cq-picker-cancel" @click="picker = null">取消</button>
      </div>
    </div>
  </div>
</template>
