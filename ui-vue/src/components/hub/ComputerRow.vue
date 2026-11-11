<script setup lang="ts">
import { computed, ref } from "vue";
import GrubList from "@/components/boot/GrubList.vue";
import type { EntryData } from "@/components/boot/types";
import type { RemoteProfile } from "@/stores/remote";
import type { RemoteBootInfo } from "@/api/types";
import { useToastStore } from "@/stores/toast";

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
const picker = ref<{ kind: string; title: string; entries: EntryData[]; active: string } | null>(null);
function openPicker(kind: string) {
  const group = kind === "refind" ? props.info?.refind : kind === "grub" ? props.info?.grub
    : (props.info?.channels ?? []).find((c) => c.kind === kind);
  const opts = ((group as { options?: { value?: string; label?: string; header?: boolean; class?: string }[] })?.options ?? []);
  if (!opts.some((o) => !o.header && (o.value ?? "").trim() !== "")) return; // 没有可选项就别弹
  // **直接复用主菜单（系统引导页）rEFInd 同款的 GrubList**：把后端 options 转成
  // GrubList 的行 —— header 行是组（submenu），其后的内核子项挂到该组下；组头之前
  // 的平铺项保持普通行。渲染出的「▸ 组名（N 项）+ 缩进子项」与主菜单完全一致。
  const entries: EntryData[] = [];
  let groupLabel = "";
  for (const o of opts) {
    if (o.header) {
      groupLabel = String(o.label ?? "");
      continue;
    }
    const value = String(o.value ?? "");
    if (!value) continue;
    const row: EntryData = { selector: value, name: String(o.label ?? value) };
    if (o.class) row.class = String(o.class);
    if (groupLabel) row.submenu = groupLabel;
    entries.push(row);
  }
  picker.value = {
    kind,
    title: (group as { title?: string })?.title ?? kind,
    entries,
    active: String((group as { current?: string })?.current ?? ""),
  };
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
interface ConnectLine { dataK: string; kindLabel: string; address: string; ok: boolean; ms: number; skipped: boolean;
  badge: string; reason: string; macs: string[]; isLan: boolean; url: string; }
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
  // 探测结果索引：kind (address) 与 kind (pair_id)
  const probeByKey = new Map<string, { ok: boolean; ms: number; error: string }>();
  for (const o of probes) {
    probeByKey.set(`${o.kind ?? "lan"}\u0000${o.address}`, { ok: !!o.ok, ms: Number(o.ms ?? 0), error: String(o.error ?? "") });
    if (o.pair_id) probeByKey.set(`${o.kind ?? "lan"}\u0000${o.pair_id}`, { ok: !!o.ok, ms: Number(o.ms ?? 0), error: String(o.error ?? "") });
  }
  const badgeOf = (st?: { ok: boolean; ms: number; error: string }) => {
    const skipped = !!st && !st.ok && /未测出|未探测/.test(st.error || "");
    return {
      ok: !!st?.ok, ms: st?.ms ?? 0, skipped,
      badge: st?.ok ? `${st.ms} ms` : skipped ? "未测出" : (st ? "不通" : ""),
      reason: !st || st.ok || skipped ? "" : (st.error || "不通"),
    };
  };
  const out: ConnectLine[] = [];
  // 1) 直连行：route_disabled/hidden 命中 lan → 整行不上；每地址一行
  if (!disabled.has("lan") && !hidden.has("lan") && addresses.length) {
    for (const addr of addresses) {
      const b = badgeOf(probeByKey.get(`lan\u0000${addr}`));
      out.push({ dataK: "lan", kindLabel: ROUTE_LABELS.lan, address: addr, ok: b.ok, ms: b.ms, skipped: b.skipped,
        badge: b.badge, reason: b.reason, macs: macsMap[addr] ?? [], isLan: true,
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
    out.push({ dataK: kind, kindLabel: ROUTE_LABELS[kind] ?? (p.kind || "远程"), address: String(p.addr), ok: b.ok, ms: b.ms,
      skipped: b.skipped, badge: b.badge, reason: b.reason, macs: [], isLan: false,
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
  toast.show(ok ? ("已复制 " + addr) : ("复制失败，请手动复制：" + url), ok ? "ok" : "bad");
}
async function openUrl(url: string) {
  const { isTauri } = await import("@/api/tauri");
  if (!isTauri()) {
    // 网页/移动端：新标签（popup 可能被拦，提示一下）
    const w = window.open(url, "_blank");
    if (!w) toast.show("浏览器拦截了弹窗，请允许后重试", "bad");
    return;
  }
  const { call } = await import("@/api/tauri");
  try {
    await call("remote_open_page", { url });
    toast.show("已在系统浏览器打开");
  } catch (e) {
    toast.show("打开浏览器失败：" + e, "bad");
  }
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
/** 平台图标（与 GrubEntry/后端 grub_os_icon 同一映射）：圆点弃用，改 emoji 图标 */
function osIcon(kind: string, name: string): string {
  const hay = `${kind} ${name}`.toLowerCase();
  if (hay.includes("win") || hay.includes("microsoft")) return "🪟 ";
  if (hay.includes("mac") || hay.includes("apple")) return "🍎 ";
  if (hay.includes("linux") || hay.includes("cachyos") || hay.includes("ubuntu")
    || hay.includes("fedora") || hay.includes("mint") || hay.includes("manjaro")
    || hay.includes("arch")) return "🐧 ";
  return "";
}

function channelOf(kind: string) {
  return (props.info?.channels ?? []).find((c) => c.kind === kind);
}
/** 通道列回显：current 是归一后的切换值域（opencore-default:… 裸 spec），直接渲染
 *  会把 spec 原串印出来。照主界面（系统引导页）的口径——显示**命中的条目名**：
 *  按 value 精确命中，再按标题互相包含兜底；都不中才原样显示。 */
function channelEcho(kind: string): { label: string; dot: string } {
  const ch = channelOf(kind);
  if (!ch || !ch.available) return { label: "未检测到", dot: "" };
  const opts = ((ch.options ?? []) as { value?: string; label?: string; header?: boolean; class?: string }[]);
  if (!opts.length) return { label: "未选择", dot: "" };
  const cur = String(ch.current || "");
  const hit =
    opts.filter((o) => !o.header && o.value === cur)[0] ||
    opts.filter((o) => {
      if (o.header || !cur) return false;
      const text = String(o.label || "").trim();
      return !!text && (text === cur || text.indexOf(cur) >= 0 || cur.indexOf(text) >= 0);
    })[0];
  return { label: hit?.label || cur || "未选择", dot: hit?.class || "" };
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
            <span v-if="osIcon(bootState('refind').dot, bootState('refind').label)">{{ osIcon(bootState('refind').dot, bootState('refind').label) }}</span>
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
            <span v-if="osIcon(bootState('grub').dot, bootState('grub').label)">{{ osIcon(bootState('grub').dot, bootState('grub').label) }}</span>
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
            <span v-if="osIcon(channelEcho(kind).dot, channelEcho(kind).label)">{{ osIcon(channelEcho(kind).dot, channelEcho(kind).label) }}</span>
            <span class="t">{{ channelEcho(kind).label }}</span>
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
          <button type="button" class="cq-remote-act" title="复制这个地址（含访问令牌）" @click.stop="copyUrl(ln.url, ln.address)">复制</button>
          <button type="button" class="cq-remote-act" title="用系统默认浏览器打开这个地址" @click.stop="openUrl(ln.url)">浏览器打开</button>
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
          <GrubList
            :entries="picker.entries"
            :active-selector="picker.active"
            @select="chooseBoot"
          />
        </div>
        <button type="button" class="cq-picker-cancel" @click="picker = null">取消</button>
      </div>
    </div>
  </div>
</template>
