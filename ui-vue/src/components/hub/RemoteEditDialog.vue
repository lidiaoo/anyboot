<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useServersStore } from "@/stores/servers";
import { useToastStore } from "@/stores/toast";
import type { RemoteProfile } from "@/stores/remote";
import { useEscape } from "@/composables/useEscape";

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
  (serversStore.servers ?? []).filter((s) => s.protocol === "ssh").map((s) => ({ id: s.id, name: (s.name || "(未命名)") + "（" + s.host + ":" + (s.port || 22) + "）" })),
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

/** 连接优先级六通道（旧版 ROUTE_IDS / ROUTE_LABELS） */
const PAIR_BLOCKS = [
  { id: "cloudflare", label: "Cloudflare" },
  { id: "ts", label: "Tailscale" },
  { id: "tsnet", label: "Tailscale（内建）" },
];
const PAIR_MODES = [
  { id: "parallel_first", long: "并发（首个）" },
  { id: "parallel_all", long: "并发（全部）" },
  { id: "serial", long: "串行" },
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
  lan: "直连", jump: "跳板机", tunnel: "Cloudflare 隧道",
  ts: "Tailscale 直连", funnel: "Tailscale Funnel", tsnet: "Tailscale Funnel（内建）",
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
const PAIR_BLOCKS_UI = [
  { id: "cloudflare", label: "Cloudflare" },
  { id: "ts", label: "Tailscale" },
  { id: "tsnet", label: "Tailscale（内建）" },
];
function pairsOf(kind: string) {
  return pairs.value.filter((p) => p.kind === kind);
}
function filledCount(kind: string) {
  return pairsOf(kind).filter((p) => !!p.addr).length;
}
function addPair(kind: string) {
  if (pairs.value.length >= 32) {
    toast.show("已达 32 条上限", "bad");
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
const hasToken = ref(false);
/** 折叠区状态 */
const foldConn = ref(false);
const foldPower = ref(false);
/** 连接策略 */
const routeProbe = ref<"first" | "all" | "serial">("first");
const probeMode = ref<"first" | "all" | "serial">("first");
const probeTimeout = ref(3);
const connCount = computed(() => rows.value.filter((r) => r.addr.trim()).length);
const powerRoutes = ref<string[]>([]);
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
    remoteHint.value = "已识别并加入地址列表";
  } else {
    remoteHint.value = "没识别出地址";
  }
}
function remoteGet() {
  remoteHint.value = "";
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
  testOut.value = "测试中…";
  try {
    const { call } = await import("@/api/tauri");
    await call("hub_remote_test", { remote: { id: props.remote?.id, name: name.value } });
    testOut.value = "连接正常";
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
      name.value = "";
      quick.value = "";
      hint.value = "";
      rows.value = [{ addr: "", macList: [], jump: "" }];
      return;
    }
    name.value = r.name;
    const ro = (r as Record<string, unknown>).routeOrder;
    if (Array.isArray(ro) && ro.length) {
      // 保留后端顺序，缺失的补到末尾
      const ordered = ro.filter((k) => (ROUTE_IDS as string[]).includes(String(k))).map(String);
      for (const k of ROUTE_IDS) if (!ordered.includes(k)) ordered.push(k);
      routeOrder.value = ordered;
    }
    const rd = (r as Record<string, unknown>).routeDisabled as Record<string, boolean> | undefined;
    if (rd && typeof rd === "object") routeDisabled.value = { ...rd };
    const bp = (r as Record<string, unknown>).blockProbe as Record<string, string> | undefined;
    if (bp && typeof bp === "object") blockProbe.value = { ...bp };
    const bt = (r as Record<string, unknown>).blockTimeouts as Record<string, string> | undefined;
    if (bt && typeof bt === "object") blockTimeouts.value = { ...bt };
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
    hint.value = "没识别出地址，请手动填写";
    return;
  }
  const addr = match[1]!;
  const exists = rows.value.some((r) => r.addr === addr);
  if (exists) {
    hint.value = "这条地址已经加过了";
    return;
  }
  const empty = rows.value.find((r) => !r.addr);
  if (empty) empty.addr = addr;
  else rows.value.push({ addr, macList: [], jump: "" });
  hint.value = "已加入地址列表";
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
    route_disabled: routeDisabled.value,
    route_timeout: routeTimeout.value,
    block_probe: blockProbe.value,
    block_timeouts: blockTimeouts.value,
  });
}
</script>

<template>
  <div v-if="open" id="rsEditOverlay" class="overlay">
    <div class="dialog rs-edit" role="dialog" aria-modal="true" aria-labelledby="rsEditTitle">
      <div class="dialog-head">
        <h3 id="rsEditTitle">{{ remote?.id ? "编辑服务端" : "连接 AnyBoot 服务端" }}</h3>
        <button type="button" class="icon-btn" id="rsEditClose" aria-label="关闭" @click="emit('close')">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>

      <div class="dialog-body">
        <div class="field-row">
          <div class="field">
            <label for="rsName">电脑名称</label>
            <input id="rsName" v-model="name" type="text" placeholder="例如：书房台式机（留空用主机名兜底）" />
          </div>
        </div>

        <div class="field">
          <label for="rsAddrQuick">快速识别（粘贴任意写法，自动拆出地址 / 端口 / MAC 加进下面）</label>
          <textarea
            id="rsAddrQuick"
            v-model="quick"
            rows="2"
            class="mono"
            spellcheck="false"
            placeholder="整段粘进来就行，例如：&#10;192.168.0.120:12300 AA:BB:CC:DD:EE:FF 台式机有线&#10;192.168.0.121 11-22-33-44-55-66（5G WiFi）"
            @keydown.ctrl.enter.prevent="recognize"
            @keydown.meta.enter.prevent="recognize"
            @paste="onQuickPaste"
          />
          <div class="addr-quick-actions">
            <button type="button" class="btn small" id="rsAddrRecognize" @click="recognize">识别并加入</button>
            <span class="addr-quick-hint" id="rsAddrHint">换行 / 空格 / 逗号 / 分号都能分开；认不出的文字（备注、括号）自动忽略</span>
          </div>
        </div>

        <div class="field">
          <label>局域网地址（第一个用来连接；MAC 只用于局域网电源）</label>
          <div id="rsAddrList" class="addr-list">
            <div v-for="(row, i) in rows" :key="i" class="addr-row">
              <div class="addr-main">
                <span class="addr-host">{{ row.addr || "（未填地址）" }}</span>
                <span v-if="i === 0" class="addr-badge">连接用</span>
                <button type="button" class="addr-del" aria-label="删掉这个地址" title="删掉这个地址" @click="removeRow(i)">✕</button>
              </div>
              <div class="addr-macs">
                <span class="route-macs-label">MAC</span>
                <template v-if="row.macList.length">
                  <button
                    v-for="mac in row.macList"
                    :key="mac"
                    type="button"
                    class="mac-tag"
                    title="点一下删掉这块网卡"
                    @click="dropMac(i, mac)"
                  >{{ mac }} <i>✕</i></button>
                </template>
                <span v-else class="route-macs-label">没配 MAC —— 局域网电源发不出去</span>
              </div>
              <div class="addr-jump">
                <span class="route-macs-label">跳板机</span>
                <select v-model="row.jump" class="addr-jump-select" title="这个地址经由哪台跳板机中转（空 = 这个地址直连）">
                  <option value="">直连</option>
                  <option v-for="opt in jumpOptions" :key="opt.id" :value="opt.id">{{ opt.name }}</option>
                </select>
              </div>
            </div>
          </div>
          <div class="addr-add">
            <input id="rsAddrHost" v-model="addrHost" type="text" spellcheck="false" placeholder="192.168.0.120 或 192.168.0.120:12300" />
            <input id="rsAddrMacs" v-model="addrMacs" type="text" spellcheck="false" placeholder="MAC（可多个，空格隔开）" />
            <button type="button" class="btn small" id="rsAddrAdd" @click="addAddr">添加</button>
          </div>
          <p class="field-hint">没写端口按 12300 补。地址已存在时只把 MAC 追加进去；点 MAC 标签可以单独删掉那块网卡。</p>
        </div>

        <div class="field">
          <label for="rsRemote">远程接入（粘贴配对码 / 地址，按类型自动归入下面的区块）</label>
          <textarea id="rsRemote" v-model="remoteText" rows="2" class="mono" spellcheck="false" placeholder="anyboot://pair?v=2&amp;mid=…&amp;type=cloudflare&amp;tunnel=…&amp;token=…&#10;https://xxx.trycloudflare.com / http://100.x.y.z:12300 / https://xxx.ts.net" />
          <div class="addr-quick-actions">
            <button type="button" class="btn small" id="rsRemoteSplit" @click="remoteSplit">识别并加入</button>
            <button type="button" class="btn small" id="rsRemoteGet" @click="remoteGet">从对方获取</button>
            <button type="button" class="btn small" id="rsRemoteClear" @click="remoteClear">全部清空</button>
            <span class="addr-quick-hint" id="rsRemoteHint">{{ remoteHint }}</span>
          </div>
          <p class="field-hint">三个区块各自可挂多条，全部保留、各自探测、各自超时 —— 一台电脑装了几个系统就配几条。同一系统同一区块再配一次 = 更新那一行（按 mid + 区块判定），不会多出一条。局域网地址优先使用，全连不上才走这里。</p>
          <p class="field-hint">「从对方获取」：对方要在同一局域网且其本机服务在线，它屏幕上会弹确认框。</p>
          <div id="rsPairsBox" class="pair-blocks">
            <div v-for="block in PAIR_BLOCKS_UI" :key="block.id" class="pb-block">
              <div class="pb-head">
                <span class="pb-title">{{ block.label }}</span>
                <span class="pb-count">{{ filledCount(block.id) ? filledCount(block.id) + " 条" : "未配置" }}</span>
                <button type="button" class="pb-add" @click="addPair(block.id)">+ 添加</button>
              </div>
              <div v-if="!pairsOf(block.id).length" class="pb-empty">这个区块还没有配对 —— 在上面粘配对码，或点「+ 添加」手填地址</div>
              <div v-for="(pair, pi) in pairsOf(block.id)" :key="pi" :class="['pb-row', { 'pb-off': pair.enabled === false }]">
                <input
                  class="pb-addr mono"
                  type="text"
                  spellcheck="false"
                  :placeholder="block.id === 'ts' ? 'http://100.x.y.z:12300 或 https://xxx.ts.net' : 'https://xxx.trycloudflare.com'"
                  :value="pair.addr"
                  @change="pair.addr = ($event.target as HTMLInputElement).value.trim()"
                />
                <span v-if="pair.kind === 'ts'" class="pb-lane">{{ (pair.lane || 'direct') === 'funnel' ? 'funnel' : '直连' }}</span>
                <input class="pb-note" type="text" placeholder="备注" :value="pair.note" @change="pair.note = ($event.target as HTMLInputElement).value.trim()" />
                <input class="pb-note mono" type="password" spellcheck="false" placeholder="令牌" :value="''" :title="pair.token ? '已存令牌（留空保持不变）' : '配对时对方签发的令牌'" @change="pair.token = ($event.target as HTMLInputElement).value.trim()" />
                <input
                  class="rp-timeout mono"
                  type="number"
                  step="0.5"
                  min="0.2"
                  max="30"
                  placeholder="默认"
                  title="这一条的探测超时（秒）；留空 = 用区块默认值，再留空 = 全局默认值"
                  :value="pair.probe_timeout_ms ? String(pair.probe_timeout_ms / 1000) : ''"
                  @change="pair.probe_timeout_ms = Math.round(Math.min(30, Math.max(0.2, parseFloat(($event.target as HTMLInputElement).value) || 0)) * 1000)"
                />
                <button type="button" class="rp-btn" title="上移" @click="movePair(pair, -1)">↑</button>
                <button type="button" class="rp-btn" title="下移" @click="movePair(pair, 1)">↓</button>
                <button type="button" class="rp-btn" title="删除这条配对" @click="removePairItem(pair)">✕</button>
                <label class="switch" title="停用后这条配对不参与探测与连接">
                  <input type="checkbox" :checked="pair.enabled !== false" @change="pair.enabled = !(pair.enabled !== false)" />
                  <span class="trk" aria-hidden="true" />
                </label>
                <span class="pb-swtag"><span class="pb-swtag-l">启用</span></span>
                <label class="switch" title="是否展示：默认开；关掉后电脑列表整行不展示这条通道">
                  <input type="checkbox" :checked="pair.show_probe !== false" @change="pair.show_probe = !(pair.show_probe !== false)" />
                  <span class="trk" aria-hidden="true" />
                </label>
              </div>
            </div>
          </div>
        </div>

        <div class="field">
          <div id="rsTokenRow" class="token-row" :hidden="!hasToken">
            <span class="slot-chip">已存令牌</span>
            <span class="mono masked" id="rsTokenSavedText">{{ hasToken ? "••••••" : "" }}</span>
            <button type="button" class="token-eye" id="rsTokenCopy" aria-label="复制令牌" title="复制令牌" :hidden="!hasToken" @click="copyToken">⧉</button>
            <button type="button" class="token-eye" id="rsTokenEye" aria-label="显示令牌" title="显示 / 隐藏令牌" @click="tokenVisible = !tokenVisible">👁</button>
          </div>
          <label for="rsToken">访问令牌 token（服务端设了才要填）</label>
          <input id="rsToken" v-model="token" class="mono" placeholder="留空保持不变；要替换就填新令牌" style="margin-top: 6px" />
        </div>

        <div class="fold-head" id="rsFoldConnHead" @click="foldConn = !foldConn">
          <span class="chev">{{ foldConn ? "▼" : "▶" }}</span>连接策略
          <span class="fold-count" id="rsFoldConnCount">{{ connCount }}</span>
        </div>
        <div class="fold-body" id="rsFoldConn" :hidden="!foldConn">
          <div class="field">
            <label><b>连接优先级</b>（拖动排序；每行中间那格 = 这个通道自己的探测超时/秒，留空用下面的默认值）</label>
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
                <span class="rp-grip" title="拖动排序">☰</span>
                <span class="rp-name">{{ ROUTE_LABELS[kind] }}</span>
                <input
                  class="rp-timeout mono"
                  type="number"
                  step="0.5"
                  min="0.2"
                  max="30"
                  :value="routeTimeout[kind] ?? ''"
                  :aria-label="ROUTE_LABELS[kind] + ' 通道的探测超时（秒）'"
                  @change="routeTimeout[kind] = Number(($event.target as HTMLInputElement).value) || undefined"
                />
                <button type="button" class="rp-btn" title="上移" @click="moveRoute(index, -1)">↑</button>
                <button type="button" class="rp-btn" title="下移" @click="moveRoute(index, 1)">↓</button>
                <label class="switch" :title="ROUTE_LABELS[kind] + ' 通道是否启用'">
                  <input type="checkbox" :checked="!routeDisabled[kind]" @change="toggleRouteEnabled(kind)" />
                  <span class="trk" aria-hidden="true" />
                </label>
              </div>
              <p class="field-hint">按住 ☰ 上下拖动调整顺序（或点 ↑↓）；探活与接口请求按此顺序逐通道试，第一个通的就走谁。每个通道可以单独设探测超时：局域网一般 1~2 秒够，走公网中继的 Funnel / 内置 Funnel 首次握手慢，可以单独给 5~8 秒，互不牵连。留空的那一格就用下面的默认值。</p>
            </div>
          </div>
          <div class="field">
            <label><b>连接优先级的探测方式</b>（六个通道之间怎么试，对上面每一个通道都生效）</label>
            <div class="seg-row" role="group" aria-label="连接优先级的探测方式">
              <button type="button" class="btn small" :class="{ 'seg-on': routeProbe === 'first' }" id="rsRouteProbeFirst" @click="routeProbe = 'first'">并发（首个）</button>
              <button type="button" class="btn small" :class="{ 'seg-on': routeProbe === 'all' }" id="rsRouteProbeAll" @click="routeProbe = 'all'">并发（全部）</button>
              <button type="button" class="btn small" :class="{ 'seg-on': routeProbe === 'serial' }" id="rsRouteProbeSerial" @click="routeProbe = 'serial'">串行</button>
            </div>
            <p class="field-hint">电脑页与电脑列表的探活都按这里来：并发（首个）把全部通道同时发出，谁先成功就用谁；并发（全部）等全部返回，逐行报每个通道的成败与耗时；串行按上面的优先级逐个通道试，第一个通就停。与下面那个「多网卡探测」是两个粒度：这个管通道之间，那个管同一通道里的多个地址。探测跑在独立的探测线程池上，调大超时或选并发全部也不会卡住界面。</p>
          </div>
          <div class="field" id="rsBlockPolicy">
            <label><b>区块内部策略</b>（同一个区块里多条配对之间怎么探）+ <b>区块默认超时</b></label>
            <div class="block-policy" id="rsBlockPolicyBox">
              <div v-for="block in PAIR_BLOCKS" :key="block.id" class="bp-row">
                <span class="bp-name">{{ block.label }}</span>
                <span class="bp-seg">
                  <button
                    v-for="mode in PAIR_MODES"
                    :key="mode.id"
                    type="button"
                    :class="['btn', 'small', { primary: (blockProbe[block.id] || 'parallel_first') === mode.id }]"
                    style="flex: 1"
                    @click="setBlockProbe(block.id, mode.id)"
                  >{{ mode.long }}</button>
                </span>
                <input
                  class="bp-timeout mono"
                  type="number"
                  step="0.5"
                  min="0.2"
                  max="30"
                  placeholder="默认"
                  title="这个区块的默认探测超时（秒）；留空 = 全局默认值"
                  :value="blockTimeouts[block.id] || ''"
                  @change="setBlockTimeout(block.id, ($event.target as HTMLInputElement).value)"
                />
              </div>
            </div>
            <p class="field-hint">每个区块单独选：并发（首个）命中即收工、其余标「未探测」（快）；并发（全部）逐条报状态（体检用）；串行按行序逐个试、第一个通就停（严格尊重优先级）。并发（首个）下顺序只影响发起先后，先排的不一定赢 —— 要严格按顺序必须选串行。区块默认超时 = 该区块内没单独设超时的行用它，再留空则用下面的全局默认值。</p>
          </div>
          <div class="field">
            <label><b>多网卡探测</b>（同一通道里的多个地址之间怎么探）</label>
            <div class="seg-row" role="group" aria-label="多网卡探测">
              <button type="button" class="btn small" :class="{ 'seg-on': probeMode === 'first' }" id="rsProbeFirst" @click="probeMode = 'first'">并发（首个）</button>
              <button type="button" class="btn small" :class="{ 'seg-on': probeMode === 'all' }" id="rsProbeAll" @click="probeMode = 'all'">并发（全部）</button>
              <button type="button" class="btn small" :class="{ 'seg-on': probeMode === 'serial' }" id="rsProbeSerial" @click="probeMode = 'serial'">串行</button>
            </div>
            <p class="field-hint">一台电脑配了多个局域网地址（多块网卡 / DHCP 换过 IP）时生效：并发（首个）谁先通用谁，���发（全部）等全部返回，串行按地址顺序试、第一个通就停。</p>
          </div>
          <div class="field">
            <label for="rsProbeTimeout">默认探测超时（秒）—— 通道那格留空时用它</label>
            <input id="rsProbeTimeout" v-model.number="probeTimeout" class="mono num-narrow" type="number" step="0.5" min="0.2" max="30" />
            <p class="field-hint">默认 2 秒（范围 0.2 ~ 30 秒）。想让某个通道等更久 / 更短，就在上面那一行各自填 —— 填了的那一格以它自己为准，没填的才用这个默认值。</p>
          </div>
          <div class="field">
            <label>经跳板机（按地址分别选）</label>
            <p class="field-hint">跳板机挂在地址上 —— 上面「局域网地址」每一行都有自己的跳板机下拉，空 = 那个地址直连。一台电脑有三块网卡时，可以分别经家里 / 公司两台跳板机中转，各建各的转发。</p>
          </div>
        </div>

        <div class="fold-head" id="rsFoldPowerHead" @click="foldPower = !foldPower">
          <span class="chev">{{ foldPower ? "▼" : "▶" }}</span>局域网电源路由
          <span class="fold-count" id="rsFoldPowerCount">{{ powerRoutes.length }}</span>
        </div>
        <div class="fold-body" id="rsFoldPower" :hidden="!foldPower">
          <div id="rsPowerRoutes" class="route-list">
            <div v-for="(r, i) in powerRoutes" :key="i" class="route-row">
              <span class="mono">{{ r }}</span>
              <button type="button" class="icon-btn" aria-label="删除" @click="powerRoutes.splice(i, 1)">✕</button>
            </div>
          </div>
          <p class="field-hint">配了路由，电源按钮就往局域网发 UDP 包，由这台电脑上的 sol 按端口执行对应动作 —— 机器睡着、连接断着也能用；没配则唤醒走标准 WOL。广播地址留空 = 自动（全局广播 + 各地址的 /24 广播）。</p>
          <button type="button" class="btn small" id="rsPowerAdd" style="margin-top: 8px" @click="powerRoutes.push('')">+ 添加一条路由</button>
        </div>

        <div class="field" id="rsRouteList">
          <label><b>连接优先级</b></label>
        </div>

        <p class="form-error" id="rsEditError">{{ editError }}</p>
        <p class="form-error" id="rsTestOut" style="color: rgb(var(--ok))">{{ testOut }}</p>
      </div>

      <div class="dialog-foot">
        <button type="button" class="btn danger" id="rsDelete" :hidden="!remote?.id" @click="doDelete">删除</button>
        <button type="button" class="btn" id="rsTestBtn" @click="doTest">测试连接</button>
        <button type="button" class="btn" id="rsCancel" @click="emit('close')">取消</button>
        <button type="button" class="btn primary" id="rsSave" @click="submit">保存</button>
      </div>
    </div>
  </div>
</template>
