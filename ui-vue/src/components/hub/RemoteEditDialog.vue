<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useServersStore } from "@/stores/servers";
import { useToastStore } from "@/stores/toast";
import type { RemoteProfile } from "@/stores/remote";
import { call } from "@/api/tauri";
import { useEscape } from "@/composables/useEscape";
const { t } = useI18n();

/**
 * 添加 / 编辑 AnyBoot 服务端弹窗（原 `ui/hub.html` 的 `#rsEditOverlay`，2533 行）。
 *
 * 注意：这是**电脑（AnyBoot 服务端）**，不是 SSH 服务器（那个是 `#editOverlay`）。
 *
 * ## 1:1 结构
 * ```
 * .overlay > .dialog.rs-edit
 *   .dialog-head > h3#rsEditTitle + button.icon-btn
 *   .dialog-body
 *     .field 名称#rsName
 *     .field 地址快速识别（#rsAddrQuick + #rsAddrRecognize + #rsAddrHint）
 *     .field 地址列表（#rsAddrList，每行 host:port + MAC 多值）
 *   .dialog-foot > 测试 + 取消 + 保存
 * ```
 */

const serversStore = useServersStore();
const toast = useToastStore();
/** 跳板机选项（旧版 jumpServerOptions：SSH 服务器列表） */
const jumpOptions = computed(() =>
  (serversStore.servers ?? []).filter((s) => s.protocol === "ssh").map((s) => ({ id: s.id, name: (s.name || t("server.untitled")) + " (" + s.host + ":" + (s.port || 22) + ")" })),
);

const props = defineProps<{
  open: boolean;
  remote?: RemoteProfile | null;
}>();

const emit = defineEmits<{
  (e: "save", payload: Record<string, unknown>): void;
  (e: "close"): void;
}>();
useEscape(computed(() => props.open), () => emit("close"));

const name = ref("");
const quick = ref("");

/**
 * 映射表里**混着品牌名与 i18n key**（Cloudflare / Tailscale 要原样，tsnet 等要走词典）。
 * 模板里渲染这些 label 时必须过它 —— 直接插值会把 `remote.tsnetLabel` 这种 key
 * 原样画到界面上（2026-11-19 用户报的「i18n 只显示了 key」）。
 */
const tr = (v: string | undefined | null): string => {
  const s = String(v ?? "");
  return s.includes(".") ? t(s) : s;
};

/** 连接优先级六通道（旧版 ROUTE_IDS / ROUTE_LABELS） */
const PAIR_BLOCKS = [
  { id: "cloudflare", label: "Cloudflare" },
  { id: "ts", label: "Tailscale" },
  { id: "tsnet", label: "remote.tsnetLabel" },
];
const PAIR_MODES = [
  { id: "parallel_first", long: "remote.probeFirst" },
  { id: "parallel_all", long: "remote.probeAll" },
  { id: "serial", long: "remote.probeSerial" },
];
const blockProbe = ref<Record<string, string>>({});
const blockTimeouts = ref<Record<string, string>>({});
function setBlockProbe(blockId: string, modeId: string) {
  const next = { ...blockProbe.value };
  if (modeId === "parallel_first") delete next[blockId];
  else next[blockId] = modeId;
  blockProbe.value = next;
}
function setBlockTimeout(blockId: string, raw: string) {
  const sec = parseFloat(raw);
  const next = { ...blockTimeouts.value };
  if (!isFinite(sec) || sec <= 0) delete next[blockId];
  else next[blockId] = String(Math.min(30, Math.max(0.2, sec)));
  blockTimeouts.value = next;
}

const ROUTE_LABELS: Record<string, string> = {
  lan: "hub.route.lan", jump: "hub.route.jump", tunnel: "hub.route.tunnel",
  ts: "hub.route.ts", funnel: "hub.route.funnel", tsnet: "hub.route.tsnet",
};
const ROUTE_IDS = ["lan", "jump", "tunnel", "ts", "funnel", "tsnet"];
const routeOrder = ref<string[]>([...ROUTE_IDS]);
const routeDisabled = ref<Record<string, boolean>>({});
const routeTimeout = ref<Record<string, number | undefined>>({});
let dragFrom = -1;

function onRouteDragStart(i: number, e: DragEvent) {
  dragFrom = i;
  if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
}
function onRouteDragOver(i: number, e: DragEvent) {
  // 视觉提示由浏览器处理，这里只阻止默认以允许 drop
  void i;
  if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
}
function onRouteDragLeave(e: DragEvent) {
  void e;
}
function onRouteDrop(to: number, e: DragEvent) {
  void e;
  if (dragFrom < 0 || dragFrom === to) return;
  const arr = [...routeOrder.value];
  const [moved] = arr.splice(dragFrom, 1);
  if (moved) arr.splice(to, 0, moved);
  routeOrder.value = arr;
  dragFrom = -1;
}
function onRouteDragEnd() { dragFrom = -1; }
function moveRoute(i: number, dir: number) {
  const j = i + dir;
  if (j < 0 || j >= routeOrder.value.length) return;
  const arr = [...routeOrder.value];
  [arr[i], arr[j]] = [arr[j]!, arr[i]!];
  routeOrder.value = arr;
}
/** 通道粒度「是否展示」：命中 routeProbeHidden → 电脑列表不展示该通道连通性（探测/连接照常） */
function toggleRouteShow(kind: string) {
  if (routeProbeHidden.value.includes(kind)) {
    routeProbeHidden.value = routeProbeHidden.value.filter((k) => k !== kind);
  } else {
    routeProbeHidden.value = [...routeProbeHidden.value, kind];
  }
}
function toggleRouteEnabled(kind: string) {
  routeDisabled.value = { ...routeDisabled.value, [kind]: !routeDisabled.value[kind] };
}
const hint = ref("");
/** 地址行：[{ addr, macs }] */
const rows = ref<{ addr: string; macList: string[]; jump: string }[]>([{ addr: "", macList: [], jump: "" }]);
/** 新增地址行的待填值 */
const addrHost = ref("");
const addrMacs = ref("");
/** 远程配对区 */
const remoteText = ref("");
const remoteHint = ref("");
interface PairRowItem {
  id?: string;
  kind: string;
  addr: string;
  lane?: string;
  note: string;
  token: string;
  probe_timeout_ms: number;
  enabled: boolean;
  show_probe: boolean;
  mid?: string;
  os?: string;
  host?: string;
  tunnel_id?: string;
}
const pairs = ref<PairRowItem[]>([]);
/** 通道粒度的「是否展示」隐藏列表（旧版 route_probe_hidden）：命中的通道在电脑列表整行不展示连通性，探测/连接照常。 */
const routeProbeHidden = ref<string[]>([]);
const PAIR_BLOCKS_UI = [
  { id: "cloudflare", label: "Cloudflare" },
  { id: "ts", label: "Tailscale" },
  { id: "tsnet", label: "remote.tsnetLabel" },
];
function pairsOf(kind: string) {
  return pairs.value.filter((p) => p.kind === kind);
}
function filledCount(kind: string) {
  return pairsOf(kind).filter((p) => !!p.addr).length;
}
function addPair(kind: string) {
  if (pairs.value.length >= 32) {
    toast.show(t("remote.limit32"), "bad");
    return;
  }
  pairs.value.push({ kind, addr: "", note: "", token: "", probe_timeout_ms: 0, enabled: true, show_probe: true });
}
function movePair(pair: PairRowItem, dir: number) {
  const idx = pairs.value.indexOf(pair);
  const j = idx + dir;
  if (j < 0 || j >= pairs.value.length) return;
  const arr = [...pairs.value];
  [arr[idx], arr[j]] = [arr[j]!, arr[idx]!];
  pairs.value = arr;
}
function removePairItem(pair: PairRowItem) {
  pairs.value = pairs.value.filter((p) => p !== pair);
}
/** 令牌区 */
const token = ref("");
const tokenVisible = ref(false);
/** 已存令牌明文（旧版 rsTokenSaved，回显/复制用） */
const savedToken = ref("");
/** 令牌打码：前 4 后 4 可见（旧版 rsMaskToken） */
function maskToken(s: string): string {
  if (!s) return "";
  if (s.length <= 8) return "•".repeat(s.length);
  return s.slice(0, 4) + "•".repeat(s.length - 8) + s.slice(-4);
}
const hasToken = ref(false);
/** 折叠区状态 */
const foldConn = ref(false);
const foldPower = ref(false);
/** 连接策略 */
const routeProbe = ref<"first" | "all" | "serial">("first");
const probeMode = ref<"first" | "all" | "serial">("first");
const probeTimeout = ref(3);
const connCount = computed(() => rows.value.filter((r) => r.addr.trim()).length);
interface PowerRoute {
  action: string;
  mode?: string;
  target: string;
  port: number;
  macs: string[];
  enabled: boolean;
  burst: number;
  retry: number;
  /** 广播包链路：direct / jump / both（默认 both） */
  broadcastLink: string;
  /** HTTP 链路：direct / jump / both（默认 both） */
  httpLink: string;
}
const POWER_ACTIONS = [
  { value: "wake", label: "hub.power.wake" },
  { value: "reboot", label: "hub.power.restart" },
  { value: "shutdown", label: "hub.power.shutdown" },
  { value: "sleep", label: "hub.power.sleep" },
];
const ROUTE_MODES = [
  { value: "auto", label: "remote.powerAuto" },
  { value: "http_first", label: "remote.httpFirst" },
  { value: "http_only", label: "remote.httpOnly" },
  { value: "packet_only", label: "remote.packetOnly" },
];
const powerRoutes = ref<PowerRoute[]>([]);
/** 电源 HTTP 同时发的通道（多选，默认全选） */
const powerHttpKinds = ref<string[]>([...ROUTE_IDS]);
const POWER_LINKS = [
  { value: "both", label: "remote.linkBoth" },
  { value: "direct", label: "remote.linkDirect" },
  { value: "jump", label: "remote.linkJump" },
];
function togglePowerKind(kind: string): void {
  powerHttpKinds.value = powerHttpKinds.value.includes(kind)
    ? powerHttpKinds.value.filter((k) => k !== kind)
    : [...powerHttpKinds.value, kind];
}
function newPowerRoute(): PowerRoute {
  return { action: "wake", mode: "auto", target: "", port: 9, macs: [], enabled: true, burst: 0, retry: 0, broadcastLink: "both", httpLink: "both" };
}
function routeDefaultBurst(r: PowerRoute): string {
  return r.action === "wake" ? "10" : "3";
}
function routeDefaultRetry(): string {
  return "10";
}
/** 底部 */
const editError = ref("");
const testOut = ref("");

function addAddr() {
  if (!addrHost.value.trim()) return;
  const ml = addrMacs.value.split(/[,;\s]+/).map((m) => m.trim()).filter(Boolean);
  rows.value.push({ addr: addrHost.value.trim(), macList: ml, jump: "" });
  addrHost.value = "";
  addrMacs.value = "";
}
function dropMac(i: number, mac: string) {
  const row = rows.value[i];
  if (row) row.macList = row.macList.filter((m) => m !== mac);
}
function removeRow(i: number) {
  rows.value.splice(i, 1);
  if (!rows.value.length) rows.value = [{ addr: "", macList: [], jump: "" }];
}
function remoteSplit() {
  const text = remoteText.value.trim();
  if (!text) return;
  const m = text.match(/https?:\/\/[^\s]+|anyboot:\/\/[^\s]+|\d+\.\d+\.\d+\.\d+(?::\d+)?/);
  if (m) {
    rows.value.push({ addr: m[0], macList: [], jump: "" });
    remoteHint.value = t("remote.hintAddedRecognized");
  } else {
    remoteHint.value = t("remote.hintNoAddr");
  }
}
async function remoteGet() {
  // 旧版：向第一个已填地址发起无凭据配对请求，对方点「同意」后回填（最多等 80 秒）
  const first = rows.value.map((r) => r.addr.trim()).filter(Boolean)[0];
  if (!first) {
    editError.value = t("remote.needLanFirst");
    return;
  }
  remoteHint.value = t("remote.pairing", { addr: first });
  try {
    const pair = (await call<string>("gateway_pair_fetch", { peer: first })) ?? "";
    remoteText.value = pair;
    remoteHint.value = pair ? t("remote.pairGot") : t("remote.pairNone");
  } catch (e) {
    remoteHint.value = "";
    editError.value = String(e);
  }
}
function remoteClear() {
  remoteText.value = "";
  pairs.value = [];
  remoteHint.value = "";
}
async function copyToken() {
  try {
    await navigator.clipboard.writeText(token.value || "");
  } catch {
    /* 忽略 */
  }
}
async function doTest() {
  testOut.value = t("remote.testing");
  try {
    const { call } = await import("@/api/tauri");
    await call("remote_test_via", { serverId: props.remote?.id ?? "", address: null });
    testOut.value = t("remote.testOk");
  } catch (e) {
    editError.value = String(e);
    testOut.value = "";
  }
}
function doDelete() {
  emit("save", { __delete: true, id: props.remote?.id });
}

watch(
  () => props.remote,
  (r) => {
    if (!r) {
      // 新建：清空所有编辑状态（含配对/路由/令牌/策略），避免残留上一次编辑
      name.value = "";
      quick.value = "";
      hint.value = "";
      remoteText.value = "";
      token.value = "";
      hasToken.value = false;
      rows.value = [{ addr: "", macList: [], jump: "" }];
      pairs.value = [];
      // 新建 = 六个通道「展示」默认全关（照抄 v0.2.5）：新电脑先不涌上电脑列表，要展示哪个再单独开；
      // 编辑 = 档案里已存的原样回显，不动。routeProbeHidden 存「关掉的」kind。
      routeProbeHidden.value = [...ROUTE_IDS];
      routeOrder.value = [...ROUTE_IDS];
      routeDisabled.value = {};
      routeTimeout.value = {};
      blockProbe.value = {};
      blockTimeouts.value = {};
      editError.value = "";
      testOut.value = "";
      return;
    }
    name.value = r.name;
    savedToken.value = String((r as Record<string, unknown>).token ?? "");
    tokenVisible.value = false;
    // 后端字段是 snake_case：route_order / route_disabled(Vec) / route_timeouts_ms(map,ms) /
    // block_probe_mode / block_timeouts_ms(map,ms)。之前读 camelCase → 读不到 → 策略/展示全丢。
    const rawRo = (r as Record<string, unknown>).route_order ?? (r as Record<string, unknown>).routeOrder;
    if (Array.isArray(rawRo) && rawRo.length) {
      const ordered = rawRo.filter((k) => (ROUTE_IDS as string[]).includes(String(k))).map(String);
      for (const k of ROUTE_IDS) if (!ordered.includes(k)) ordered.push(k);
      routeOrder.value = ordered;
    }
    // route_disabled 是 Vec<String>(停用的 kind 数组) → 转 {kind:true}
    const rdArr = (r as Record<string, unknown>).route_disabled;
    routeDisabled.value = {};
    if (Array.isArray(rdArr)) for (const k of rdArr) routeDisabled.value = { ...routeDisabled.value, [String(k)]: true };
    // 默认探测超时 ms→秒
    const ptm = Number((r as Record<string, unknown>).probe_timeout_ms ?? 0);
    probeTimeout.value = ptm > 0 ? ptm / 1000 : 3;
    // route_timeouts_ms(map,ms) → 秒
    const rtm = (r as Record<string, unknown>).route_timeouts_ms as Record<string, number> | undefined;
    routeTimeout.value = {};
    if (rtm && typeof rtm === "object") {
      for (const [k, ms] of Object.entries(rtm)) if (ms > 0) routeTimeout.value = { ...routeTimeout.value, [k]: ms / 1000 };
    }
    const pr = (r as Record<string, unknown>).power_routes;
    if (Array.isArray(pr)) {
      powerRoutes.value = pr.map((x) => {
        const o = x as Record<string, unknown>;
        return {
          action: String(o.action ?? "wake"),
          mode: String(o.mode ?? "auto"),
          target: String(o.target ?? ""),
          port: Number(o.port ?? 9),
          macs: Array.isArray(o.macs) ? (o.macs as string[]) : [],
          enabled: o.enabled !== false,
          burst: Number(o.burst ?? 0),
          retry: Number(o.retry ?? 0),
          broadcastLink: String(o.broadcast_link ?? "both") || "both",
          httpLink: String(o.http_link ?? "both") || "both",
        };
      });
    } else {
      powerRoutes.value = [];
    }
    const kinds = (r as Record<string, unknown>).power_http_kinds;
    powerHttpKinds.value = Array.isArray(kinds) && kinds.length
      ? (kinds as string[]).filter((k) => ROUTE_IDS.includes(k))
      : [...ROUTE_IDS];
    // 连接优先级探测方式 / 多网卡探测（之前只改 ref 不读档 → 重开回默认，看起来"选中不了"）
    const shortOf = (raw: unknown): "first" | "all" | "serial" => {
      const v = String(raw ?? "").trim();
      if (v === "parallel_all" || v === "all") return "all";
      if (v === "serial") return "serial";
      return "first";
    };
    routeProbe.value = shortOf((r as Record<string, unknown>).route_probe_mode);
    probeMode.value = shortOf((r as Record<string, unknown>).probe_mode);
    const bpm = (r as Record<string, unknown>).block_probe_mode as Record<string, string> | undefined;
    blockProbe.value = bpm && typeof bpm === "object" ? { ...bpm } : {};
    // block_timeouts_ms(map,ms) → 秒(字符串)
    const btm = (r as Record<string, unknown>).block_timeouts_ms as Record<string, number> | undefined;
    blockTimeouts.value = {};
    if (btm && typeof btm === "object") {
      for (const [k, ms] of Object.entries(btm)) if (ms > 0) blockTimeouts.value = { ...blockTimeouts.value, [k]: String(ms / 1000) };
    }
    // 读入连接配对（含 show_probe）与通道粒度展示隐藏——否则保存时会被后端 default 清空（数据丢失）
    const pr2 = (r as Record<string, unknown>).pairs;
    if (Array.isArray(pr2)) {
      pairs.value = pr2.map((x) => {
        const o = x as Record<string, unknown>;
        return {
          id: o.id ? String(o.id) : undefined,
          kind: String(o.kind ?? ""), addr: String(o.addr ?? ""), lane: o.lane ? String(o.lane) : undefined,
          note: String(o.note ?? ""), token: String(o.token ?? ""),
          probe_timeout_ms: Number(o.probe_timeout_ms ?? 0), enabled: o.enabled !== false,
          show_probe: o.show_probe !== false, mid: o.mid ? String(o.mid) : undefined,
          os: o.os ? String(o.os) : undefined, host: o.host ? String(o.host) : undefined,
          tunnel_id: o.tunnel_id ? String(o.tunnel_id) : undefined,
        } as PairRowItem;
      });
    } else { pairs.value = []; }
    const rph = (r as Record<string, unknown>).route_probe_hidden;
    routeProbeHidden.value = Array.isArray(rph) ? rph.map(String) : [];
    // 把 addresses + macs 摊平成行
    const addrs = r.addresses ?? [];
    const macs = (r.macs ?? {}) as Record<string, string[]>;
    const addrJump = ((r as Record<string, unknown>).addrJump ?? {}) as Record<string, string>;
    rows.value = addrs.length
      ? addrs.map((addr) => ({ addr, macList: macs[addr] ?? [], jump: addrJump[addr] ?? "" }))
      : [{ addr: "", macList: [], jump: "" }];
  },
  { immediate: true },
);

/** 地址快速识别：粘贴一整条 host:port 或带 token 的 URL */
function onQuickPaste() {
  window.setTimeout(() => recognize(), 0);
}

function recognize() {
  const text = quick.value.trim();
  if (!text) return;
  const match = text.match(/(\d+\.\d+\.\d+\.\d+(?::\d+)?)/);
  if (!match) {
    hint.value = t("remote.hintNoAddrManual");
    return;
  }
  const addr = match[1]!;
  const exists = rows.value.some((r) => r.addr === addr);
  if (exists) {
    hint.value = t("remote.hintDup");
    return;
  }
  const empty = rows.value.find((r) => !r.addr);
  if (empty) empty.addr = addr;
  else rows.value.push({ addr, macList: [], jump: "" });
  hint.value = t("remote.hintAdded");
  quick.value = "";
}

function submit() {
  const addresses: string[] = [];
  const macs: Record<string, string[]> = {};
  for (const row of rows.value) {
    const addr = row.addr.trim();
    if (!addr) continue;
    addresses.push(addr);
    if (row.macList.length) macs[addr] = [...row.macList];
  }
  emit("save", {
    id: props.remote?.id ?? "",
    name: name.value.trim(),
    addresses,
    macs,
    route_order: routeOrder.value,
    // 后端 route_disabled 是 Vec<String>（只存停用的 kind）；之前发对象 {kind:bool} → map≠sequence 保存失败
    route_disabled: ROUTE_IDS.filter((k) => (routeDisabled.value as Record<string, boolean>)[k]),
    route_probe_hidden: routeProbeHidden.value,
    pairs: pairs.value.map((p) => ({
      id: p.id ?? "", kind: p.kind, addr: p.addr, lane: p.lane ?? "",
      note: p.note, token: p.token, probe_timeout_ms: p.probe_timeout_ms,
      enabled: p.enabled, show_probe: p.show_probe, mid: p.mid ?? "",
      os: p.os ?? "", host: p.host ?? "", tunnel_id: p.tunnel_id ?? "",
    })),
    // 字段名/单位对齐后端：route_timeouts_ms(秒→ms) / block_probe_mode / block_timeouts_ms(秒→ms)
    route_timeouts_ms: Object.fromEntries(
      Object.entries(routeTimeout.value)
        .map(([k, v]) => [k, typeof v === "number" && v > 0 ? Math.round(v * 1000) : 0] as [string, number])
        .filter(([, v]) => v > 0),
    ),
    power_routes: powerRoutes.value.map((r) => ({
      action: r.action, mode: r.mode, target: r.target, port: r.port,
      macs: r.macs, enabled: r.enabled, burst: r.burst, retry: r.retry,
      broadcast_link: r.broadcastLink || "both",
      http_link: r.httpLink || "both",
    })),
    power_http_kinds: powerHttpKinds.value.length ? [...powerHttpKinds.value] : [...ROUTE_IDS],
    // 默认探测超时（秒→ms；通道那格留空时用它）
    probe_timeout_ms: probeTimeout.value > 0 ? Math.round(probeTimeout.value * 1000) : 0,
    // 连接优先级探测方式 / 多网卡探测（first↔parallel_first / all↔parallel_all / serial）
    route_probe_mode: routeProbe.value === "all" ? "parallel_all" : routeProbe.value === "serial" ? "serial" : "parallel_first",
    probe_mode: probeMode.value === "all" ? "parallel_all" : probeMode.value === "serial" ? "serial" : "parallel_first",
    block_probe_mode: blockProbe.value,
    block_timeouts_ms: Object.fromEntries(
      Object.entries(blockTimeouts.value)
        .map(([k, v]) => {
          const n = Number(v);
          return [k, Number.isFinite(n) && n > 0 ? Math.round(n * 1000) : 0] as [string, number];
        })
        .filter(([, v]) => v > 0),
    ),
  });
}
</script>

<template>
  <div v-if="open" id="rsEditOverlay" class="overlay">
    <div class="dialog rs-edit" role="dialog" aria-modal="true" aria-labelledby="rsEditTitle">
      <div class="dialog-head">
        <h3 id="rsEditTitle">{{ remote?.id ? t("remote.editTitle") : t("remote.connectTitle") }}</h3>
        <button type="button" class="icon-btn" id="rsEditClose" :aria-label="t('common.close')" @click="emit('close')">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>

      <div class="dialog-body">
        <div class="field-row">
          <div class="field">
            <label for="rsName">{{ t("remote.name") }}</label>
            <input id="rsName" v-model="name" type="text" :placeholder="t('remote.namePh')" />
          </div>
        </div>

        <div class="field">
          <label for="rsAddrQuick">{{ t("remote.quick") }}</label>
          <textarea
            id="rsAddrQuick"
            v-model="quick"
            rows="2"
            class="mono"
            spellcheck="false"
            :placeholder="t('remote.quickPh')"
            @keydown.ctrl.enter.prevent="recognize"
            @keydown.meta.enter.prevent="recognize"
            @paste="onQuickPaste"
          />
          <div class="addr-quick-actions">
            <button type="button" class="btn small" id="rsAddrRecognize" @click="recognize">{{ t("remote.recognize") }}</button>
            <span class="addr-quick-hint" id="rsAddrHint">{{ t("remote.quickHint") }}</span>
          </div>
        </div>

        <div class="field">
          <label>{{ t("remote.lanLabel") }}</label>
          <div id="rsAddrList" class="addr-list">
            <p v-if="!rows.some((r) => r.addr)" class="addr-empty">{{ t("remote.addrEmpty") }}</p>
            <div v-for="(row, i) in rows" :key="i" class="addr-row">
              <div class="addr-main">
                <span class="addr-host">{{ row.addr || t("remote.addrUnfilled") }}</span>
                <span v-if="i === 0" class="addr-badge">{{ t("remote.badgeConnect") }}</span>
                <button type="button" class="addr-del" :aria-label="t('remote.delAddr')" :title="t('remote.delAddr')" @click="removeRow(i)">✕</button>
              </div>
              <div class="addr-macs">
                <span class="route-macs-label">MAC</span>
                <template v-if="row.macList.length">
                  <button
                    v-for="mac in row.macList"
                    :key="mac"
                    type="button"
                    class="mac-tag"
                    :title="t('remote.delNic')"
                    @click="dropMac(i, mac)"
                  >{{ mac }} <i>✕</i></button>
                </template>
                <span v-else class="route-macs-label">{{ t("remote.noMac") }}</span>
              </div>
              <div class="addr-jump">
                <span class="route-macs-label">{{ t("remote.jumpLabel") }}</span>
                <select v-model="row.jump" class="addr-jump-select" :title="t('remote.jumpSelectTitle')">
                  <option value="">{{ t("server.direct") }}</option>
                  <option v-for="opt in jumpOptions" :key="opt.id" :value="opt.id">{{ opt.name }}</option>
                </select>
              </div>
            </div>
          </div>
          <div class="addr-add">
            <input id="rsAddrHost" v-model="addrHost" type="text" spellcheck="false" :placeholder="t('remote.hostPh')" @keydown.enter.prevent="addAddr" />
            <input id="rsAddrMacs" v-model="addrMacs" type="text" spellcheck="false" :placeholder="t('remote.macsPh')" @keydown.enter.prevent="addAddr" />
            <button type="button" class="btn small" id="rsAddrAdd" @click="addAddr">{{ t("common.add") }}</button>
          </div>
          <p class="field-hint">{{ t("remote.addrHelp") }}</p>
        </div>

        <div class="field">
          <label for="rsRemote">{{ t("remote.remoteLabel") }}</label>
          <textarea id="rsRemote" v-model="remoteText" rows="2" class="mono" spellcheck="false" placeholder="anyboot://pair?v=2&amp;mid=…&amp;type=cloudflare&amp;tunnel=…&amp;token=…&#10;https://xxx.trycloudflare.com / http://100.x.y.z:12300 / https://xxx.ts.net" />
          <div class="addr-quick-actions">
            <button type="button" class="btn small" id="rsRemoteSplit" @click="remoteSplit">{{ t("remote.split") }}</button>
            <button type="button" class="btn small" id="rsRemoteGet" @click="remoteGet">{{ t("remote.getFromPeer") }}</button>
            <button type="button" class="btn small" id="rsRemoteClear" @click="remoteClear">{{ t("remote.clearAll") }}</button>
            <span class="addr-quick-hint" id="rsRemoteHint">{{ remoteHint }}</span>
          </div>
          <p class="field-hint">{{ t("remote.blockHelp1") }}</p>
          <p class="field-hint">{{ t("remote.blockHelp2") }}</p>
          <div id="rsPairsBox" class="pair-blocks">
            <div v-for="block in PAIR_BLOCKS_UI" :key="block.id" class="pb-block">
              <div class="pb-head">
                <span class="pb-title">{{ tr(block.label) }}</span>
                <span class="pb-count">{{ filledCount(block.id) ? t("remote.blockCount", { n: filledCount(block.id) }) : t("remote.blockUnset") }}</span>
                <button type="button" class="pb-add" @click="addPair(block.id)">{{ t("remote.pairAdd") }}</button>
              </div>
              <div v-if="!pairsOf(block.id).length" class="pb-empty">{{ t("remote.pairEmpty") }}</div>
              <div v-for="(pair, pi) in pairsOf(block.id)" :key="pi" :class="['pb-row', { 'pb-off': pair.enabled === false }]">
                <!-- 第一行：地址 + 通道徽章 + 超时 + ↑↓✕ -->
                <div class="pb-main">
                  <input
                    class="pb-addr mono"
                    type="text"
                    spellcheck="false"
                    :placeholder="block.id === 'ts' ? 'http://100.x.y.z:12300 or https://xxx.ts.net' : 'https://xxx.trycloudflare.com'"
                    :value="pair.addr"
                    @change="pair.addr = ($event.target as HTMLInputElement).value.trim()"
                  />
                  <span v-if="pair.kind === 'ts'" class="pb-lane">{{ (pair.lane || 'direct') === 'funnel' ? 'funnel' : t('server.direct') }}</span>
                  <input
                    class="rp-timeout mono"
                    type="number"
                    step="0.5"
                    min="0.2"
                    max="30"
                    :placeholder="t('srvEdit.portPh')"
                    :title="t('remote.pairTimeoutTitle')"
                    :value="pair.probe_timeout_ms ? String(pair.probe_timeout_ms / 1000) : ''"
                    @change="pair.probe_timeout_ms = Math.round(Math.min(30, Math.max(0.2, parseFloat(($event.target as HTMLInputElement).value) || 0)) * 1000)"
                  />
                  <span class="pb-act">
                    <button type="button" class="rp-btn" :title="t('remote.moveUp')" @click="movePair(pair, -1)">↑</button>
                    <button type="button" class="rp-btn" :title="t('remote.moveDown')" @click="movePair(pair, 1)">↓</button>
                    <button type="button" class="rp-btn" :title="t('remote.delPair')" @click="removePairItem(pair)">✕</button>
                  </span>
                </div>
                <!-- 第二行：备注 + 令牌 + 启用 / 展示 开关 -->
                <div class="pb-meta">
                  <input class="pb-note" type="text" :placeholder="t('remote.notePh')" :value="pair.note" @change="pair.note = ($event.target as HTMLInputElement).value.trim()" />
                  <input class="pb-note mono" type="password" spellcheck="false" :placeholder="t('remote.tokenPh')" :value="''" :title="pair.token ? '已存令牌（留空保持不变）' : '配对时对方签发的令牌'" @change="pair.token = ($event.target as HTMLInputElement).value.trim()" />
                  <span class="pb-swtag">
                    <label class="switch" :title="t('remote.pairDisableTitle')">
                      <input type="checkbox" :checked="pair.enabled !== false" @change="pair.enabled = !(pair.enabled !== false)" />
                      <span class="trk" aria-hidden="true" />
                    </label>
                    <span class="pb-swtag-l">{{ t("remote.enable") }}</span>
                  </span>
                  <span class="pb-swtag">
                    <label class="switch" :title="t('remote.pairShowTitle')">
                      <input type="checkbox" :checked="pair.show_probe !== false" @change="pair.show_probe = !(pair.show_probe !== false)" />
                      <span class="trk" aria-hidden="true" />
                    </label>
                    <span class="pb-swtag-l">{{ t("remote.show") }}</span>
                  </span>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div class="field">
          <div id="rsTokenRow" class="token-row" :hidden="!hasToken">
            <span class="slot-chip">{{ t("remote.tokenStored") }}</span>
            <span :class="['mono', { masked: !tokenVisible }]" id="rsTokenSavedText">{{ tokenVisible ? savedToken : maskToken(savedToken) }}</span>
            <button type="button" class="token-eye" id="rsTokenCopy" :aria-label="t('remote.copyToken')" :title="t('remote.copyToken')" :hidden="!tokenVisible" @click="copyToken">⧉</button>
            <button type="button" class="token-eye" id="rsTokenEye" :aria-label="tokenVisible ? t('remote.hideToken') : t('remote.showToken')" :title="t('remote.showHideTokenTitle')" @click="tokenVisible = !tokenVisible">
              <svg v-show="!tokenVisible" class="eye-on" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7Z" /><circle cx="12" cy="12" r="3" /></svg>
              <svg v-show="tokenVisible" class="eye-off" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" style="width:14px;height:14px"><path d="M9.9 4.24A10.9 10.9 0 0 1 12 4c6.5 0 10 7 10 7a17.6 17.6 0 0 1-2.29 2.95M6.61 6.61A13.5 13.5 0 0 0 2 12s3.5 7 10 7a10.7 10.7 0 0 0 5.39-1.39" /><path d="M2 2l20 20" /><path d="M9.88 9.88a3 3 0 1 0 4.24 4.24" /></svg>
            </button>
          </div>
          <label for="rsToken">{{ t("remote.tokenLabel") }}</label>
          <input id="rsToken" v-model="token" class="mono" :placeholder="t('remote.tokenPh')" style="margin-top: 6px" />
        </div>

        <div class="fold-head" id="rsFoldConnHead" @click="foldConn = !foldConn">
          <span class="chev">{{ foldConn ? "▼" : "▶" }}</span>{{ t("remote.connPolicy") }}
          <span class="fold-count" id="rsFoldConnCount">{{ connCount }}</span>
        </div>
        <div class="fold-body" id="rsFoldConn" :hidden="!foldConn">
          <div class="field">
            <label><b>{{ t("remote.priorityShort") }}</b>{{ t("remote.priorityNote") }}</label>
            <div class="route-prio" id="rsRouteList">
              <div
                v-for="(kind, index) in routeOrder"
                :key="kind"
                :class="['rp-row', { 'rp-off': routeDisabled[kind] }]"
                draggable="true"
                :data-kind="kind"
                @dragstart="onRouteDragStart(index, $event)"
                @dragover.prevent="onRouteDragOver(index, $event)"
                @dragleave="onRouteDragLeave($event)"
                @drop.prevent="onRouteDrop(index, $event)"
                @dragend="onRouteDragEnd"
              >
                <span class="rp-grip" :title="t('remote.dragSort')">☰</span>
                <span class="rp-name">{{ tr(ROUTE_LABELS[kind]) }}</span>
                <input
                  class="rp-timeout mono"
                  type="number"
                  step="0.5"
                  min="0.2"
                  max="30"
                  :value="routeTimeout[kind] ?? ''"
                  :aria-label="t('remote.routeTimeoutAria', { route: t(ROUTE_LABELS[kind] || '') || kind })"
                  @change="routeTimeout[kind] = Number(($event.target as HTMLInputElement).value) || undefined"
                />
                <button type="button" class="rp-btn" :title="t('remote.moveUp')" @click="moveRoute(index, -1)">↑</button>
                <button type="button" class="rp-btn" :title="t('remote.moveDown')" @click="moveRoute(index, 1)">↓</button>
                <!-- 启用开关（v0.2.5：pb-swtag 包裹 + "启用"文字） -->
                <span class="pb-swtag" :title="t('remote.enableChannelTitle')">
                  <label class="switch">
                    <input type="checkbox" :checked="!routeDisabled[kind]" @change="toggleRouteEnabled(kind)" />
                    <span class="trk" aria-hidden="true" />
                  </label>
                  <span class="pb-swtag-l">{{ t("remote.enable") }}</span>
                </span>
                <!-- 「是否展示」开关（v0.2.5：pb-showprobe 包裹 + "展示"文字） -->
                <span class="pb-showprobe" :title="t('remote.showProbeTitle', { route: t(ROUTE_LABELS[kind] || '') || kind })">
                  <label class="switch">
                    <input type="checkbox" :checked="!routeProbeHidden.includes(kind)" @change="toggleRouteShow(kind)" />
                    <span class="trk" aria-hidden="true" />
                  </label>
                  <span class="pb-showprobe-l">{{ t("remote.show") }}</span>
                </span>
              </div>
              <p class="field-hint">{{ t("remote.priorityHelp") }}</p>
            </div>
          </div>
          <div class="field">
            <label><b>{{ t("remote.powerHttpKinds") }}</b></label>
            <div style="display:flex;flex-wrap:wrap;gap:10px">
              <label v-for="kind in ROUTE_IDS" :key="'pk-' + kind" style="display:inline-flex;align-items:center;gap:5px">
                <input type="checkbox" :checked="powerHttpKinds.includes(kind)" @change="togglePowerKind(kind)" />
                <span>{{ tr(ROUTE_LABELS[kind]) }}</span>
              </label>
            </div>
            <p class="field-hint">{{ t("remote.powerHttpKindsHint") }}</p>
          </div>
          <div class="field">
            <label><b>{{ t("remote.priorityProbeLabel") }}</b></label>
            <div class="seg-row" role="group" :aria-label="t('remote.priorityProbeAria')">
              <button type="button" class="btn small" :class="{ 'seg-on': routeProbe === 'first' }" id="rsRouteProbeFirst" @click="routeProbe = 'first'">{{ t("remote.probeFirst") }}</button>
              <button type="button" class="btn small" :class="{ 'seg-on': routeProbe === 'all' }" id="rsRouteProbeAll" @click="routeProbe = 'all'">{{ t("remote.probeAll") }}</button>
              <button type="button" class="btn small" :class="{ 'seg-on': routeProbe === 'serial' }" id="rsRouteProbeSerial" @click="routeProbe = 'serial'">{{ t("remote.probeSerial") }}</button>
            </div>
            <p class="field-hint">{{ t("remote.probeHelp") }}</p>
          </div>
          <div class="field" id="rsBlockPolicy">
            <label>{{ t("remote.blockStrategyLabel") }}</label>
            <div class="block-policy" id="rsBlockPolicyBox">
              <div v-for="block in PAIR_BLOCKS" :key="block.id" class="bp-row">
                <span class="bp-name">{{ tr(block.label) }}</span>
                <span class="bp-seg">
                  <button
                    v-for="mode in PAIR_MODES"
                    :key="mode.id"
                    type="button"
                    :class="['btn', 'small', { primary: (blockProbe[block.id] || 'parallel_first') === mode.id }]"
                    style="flex: 1"
                    @click="setBlockProbe(block.id, mode.id)"
                  >{{ tr(mode.long) }}</button>
                </span>
                <input
                  class="bp-timeout mono"
                  type="number"
                  step="0.5"
                  min="0.2"
                  max="30"
                  :placeholder="t('srvEdit.portPh')"
                  :title="t('remote.blockTimeoutTitle')"
                  :value="blockTimeouts[block.id] || ''"
                  @change="setBlockTimeout(block.id, ($event.target as HTMLInputElement).value)"
                />
              </div>
            </div>
            <p class="field-hint">{{ t("remote.blockStrategyHelp") }}</p>
          </div>
          <div class="field">
            <label><b>{{ t("remote.multiNicLabel") }}</b></label>
            <div class="seg-row" role="group" :aria-label="t('remote.multiNicAria')">
              <button type="button" class="btn small" :class="{ 'seg-on': probeMode === 'first' }" id="rsProbeFirst" @click="probeMode = 'first'">{{ t("remote.probeFirst") }}</button>
              <button type="button" class="btn small" :class="{ 'seg-on': probeMode === 'all' }" id="rsProbeAll" @click="probeMode = 'all'">{{ t("remote.probeAll") }}</button>
              <button type="button" class="btn small" :class="{ 'seg-on': probeMode === 'serial' }" id="rsProbeSerial" @click="probeMode = 'serial'">{{ t("remote.probeSerial") }}</button>
            </div>
            <p class="field-hint">{{ t("remote.multiNicHelp") }}</p>
          </div>
          <div class="field">
            <label for="rsProbeTimeout">{{ t("remote.defaultTimeoutLabel") }}</label>
            <input id="rsProbeTimeout" v-model.number="probeTimeout" class="mono num-narrow" type="number" step="0.5" min="0.2" max="30" />
            <p class="field-hint">{{ t("remote.defaultTimeoutHelp") }}</p>
          </div>
          <div class="field">
            <label>{{ t("remote.viaJumpLabel") }}</label>
            <p class="field-hint">{{ t("remote.viaJumpHelp") }}</p>
          </div>
        </div>

        <div class="fold-head" id="rsFoldPowerHead" @click="foldPower = !foldPower">
          <span class="chev">{{ foldPower ? "▼" : "▶" }}</span>{{ t("remote.powerRouteTitle") }}
          <span class="fold-count" id="rsFoldPowerCount">{{ powerRoutes.length }}</span>
        </div>
        <div class="fold-body" id="rsFoldPower" :hidden="!foldPower">
          <div id="rsPowerRoutes" class="route-list">
            <div v-for="(r, i) in powerRoutes" :key="i" :class="['route-row', { 'route-off': !r.enabled }]">
              <!-- 行头：序号 + 动作下拉 + 模式下拉 + 启用 + 删除 -->
              <div class="route-head">
                <span class="route-no">{{ i + 1 }}</span>
                <span class="route-zone-inline">
                  <select v-model="r.action" class="route-act" :title="t('remote.routeActionTitle')">
                    <option v-for="a in POWER_ACTIONS" :key="a.value" :value="a.value">{{ tr(a.label) }}</option>
                  </select>
                </span>
                <span class="route-zone-inline">
                  <select v-model="r.mode" class="route-mode" :title="t('remote.routeModeTitle')">
                    <option v-for="m in ROUTE_MODES" :key="m.value" :value="m.value">{{ tr(m.label) }}</option>
                  </select>
                </span>
                <span class="route-head-spacer" />
                <label class="switch" :title="t('remote.routeDisableTitle')">
                  <input type="checkbox" class="route-enabled" v-model="r.enabled" />
                  <span class="trk" aria-hidden="true" />
                </label>
                <button type="button" class="route-del" :aria-label="t('remote.routeDel')" :title="t('remote.routeDel')" @click="powerRoutes.splice(i, 1)">✕</button>
              </div>
              <!-- 广播包区 -->
              <div class="route-zone route-zone-packet">
                <div class="route-zone-title">{{ t("remote.broadcast") }}</div>
                <div class="route-main">
                  <span class="route-num-label">{{ t("remote.broadcastAddr") }}</span>
                  <input v-model="r.target" class="route-target mono" type="text" spellcheck="false" :placeholder="t('remote.broadcastAddrPh')" />
                  <span class="route-num-label">{{ t("server.kPort") }}</span>
                  <input v-model.number="r.port" class="route-port mono" type="number" min="0" max="65535" placeholder="9" />
                </div>
                <div class="route-main">
                  <span class="route-num-label">{{ t("remote.broadcastSend") }}</span>
                  <select v-model="r.broadcastLink" class="route-mode">
                    <option v-for="l in POWER_LINKS" :key="l.value" :value="l.value">{{ tr(l.label) }}</option>
                  </select>
                </div>
                <div class="route-main">
                  <span class="route-num-label">{{ t("remote.perNic") }}</span>
                  <input v-model.number="r.burst" class="route-burst mono" type="number" min="0" max="20" :placeholder="routeDefaultBurst(r)" />
                  <span class="route-num-label">{{ t("remote.retryIfFail") }}</span>
                  <input v-model.number="r.retry" class="route-retry mono" type="number" min="0" max="20" :placeholder="routeDefaultRetry()" />
                </div>
                <div class="route-main">
                  <span class="route-macs-label">MAC</span>
                  <span class="route-macs">{{ r.macs.length ? r.macs.join(" · ") : t("remote.allMacs") }}</span>
                </div>
              </div>
              <!-- HTTP 区 -->
              <div class="route-zone route-zone-http">
                <div class="route-zone-title">HTTP</div>
                <div class="route-main">
                  <span class="route-num-label">{{ t("remote.httpSend") }}</span>
                  <select v-model="r.httpLink" class="route-mode">
                    <option v-for="l in POWER_LINKS" :key="l.value" :value="l.value">{{ tr(l.label) }}</option>
                  </select>
                </div>
                <p class="route-zone-hint">{{ t("remote.routeZoneHint") }}</p>
              </div>
            </div>
          </div>
          <p class="field-hint">{{ t("remote.routeHelp") }}</p>
          <button type="button" class="btn small" id="rsPowerAdd" style="margin-top: 8px" @click="powerRoutes.push(newPowerRoute())">{{ t("remote.addRoute") }}</button>
        </div>

        <div class="field" id="rsRouteList">
          <label><b>{{ t("remote.priorityShort") }}</b></label>
        </div>

        <p class="form-error" id="rsEditError">{{ editError }}</p>
        <p class="form-error" id="rsTestOut" style="color: rgb(var(--ok))">{{ testOut }}</p>
      </div>

      <div class="dialog-foot">
        <button type="button" class="btn danger" id="rsDelete" :hidden="!remote?.id" @click="doDelete">{{ t("common.delete") }}</button>
        <button type="button" class="btn" id="rsTestBtn" @click="doTest">{{ t("server.test") }}</button>
        <button type="button" class="btn" id="rsCancel" @click="emit('close')">{{ t("common.cancel") }}</button>
        <button type="button" class="btn primary" id="rsSave" @click="submit">{{ t("common.save") }}</button>
      </div>
    </div>
  </div>
</template>
