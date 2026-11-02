<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { RemoteProfile } from "@/stores/remote";

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

const props = defineProps<{
  open: boolean;
  remote?: RemoteProfile | null;
}>();

const emit = defineEmits<{
  (e: "save", payload: Record<string, unknown>): void;
  (e: "close"): void;
}>();

const name = ref("");
const quick = ref("");
const hint = ref("");
/** 地址行：[{ addr, macs }] */
const rows = ref<{ addr: string; macs: string }[]>([{ addr: "", macs: "" }]);
/** 新增地址行的待填值 */
const addrHost = ref("");
const addrMacs = ref("");
/** 远程配对区 */
const remoteText = ref("");
const remoteHint = ref("");
const pairs = ref<{ mid?: string; kind?: string }[]>([]);
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
  rows.value.push({ addr: addrHost.value.trim(), macs: addrMacs.value.trim() });
  addrHost.value = "";
  addrMacs.value = "";
}
function removeRow(i: number) {
  rows.value.splice(i, 1);
  if (!rows.value.length) rows.value = [{ addr: "", macs: "" }];
}
function remoteSplit() {
  const text = remoteText.value.trim();
  if (!text) return;
  const m = text.match(/https?:\/\/[^\s]+|anyboot:\/\/[^\s]+|\d+\.\d+\.\d+\.\d+(?::\d+)?/);
  if (m) {
    rows.value.push({ addr: m[0], macs: "" });
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
function removePair(i: number) {
  pairs.value.splice(i, 1);
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
      rows.value = [{ addr: "", macs: "" }];
      return;
    }
    name.value = r.name;
    // 把 addresses + macs 摊平成行
    const addrs = r.addresses ?? [];
    const macs = (r.macs ?? {}) as Record<string, string[]>;
    rows.value = addrs.length
      ? addrs.map((addr) => ({ addr, macs: (macs[addr] ?? []).join(", ") }))
      : [{ addr: "", macs: "" }];
  },
  { immediate: true },
);

/** 地址快速识别：粘贴一整条 host:port 或带 token 的 URL */
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
  else rows.value.push({ addr, macs: "" });
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
    const list = row.macs
      .split(/[,;\s]+/)
      .map((m) => m.trim())
      .filter(Boolean);
    if (list.length) macs[addr] = list;
  }
  emit("save", {
    id: props.remote?.id ?? "",
    name: name.value.trim(),
    addresses,
    macs,
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
          <textarea id="rsAddrQuick" v-model="quick" rows="2" class="mono" spellcheck="false" placeholder="整段粘进来就行，例如：&#10;192.168.0.120:12300 AA:BB:CC:DD:EE:FF 台式机有线&#10;192.168.0.121 11-22-33-44-55-66（5G WiFi）" />
          <div class="addr-quick-actions">
            <button type="button" class="btn small" id="rsAddrRecognize" @click="recognize">识别并加入</button>
            <span class="addr-quick-hint" id="rsAddrHint">换行 / 空格 / 逗号 / 分号都能分开；认不出的文字（备注、括号）自动忽略</span>
          </div>
        </div>

        <div class="field">
          <label>局域网地址（第一个用来连接；MAC 只用于局域网电源）</label>
          <div id="rsAddrList" class="addr-list">
            <div v-for="(row, i) in rows" :key="i" class="addr-row">
              <input v-model="row.addr" type="text" class="mono" placeholder="127.0.0.1:12300" spellcheck="false" />
              <input v-model="row.macs" type="text" class="mono" placeholder="MAC" spellcheck="false" />
              <button type="button" class="icon-btn" aria-label="删除这一行" @click="removeRow(i)">✕</button>
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
            <div v-for="(p, i) in pairs" :key="i" class="pair-block">
              <span class="mono">{{ p.kind || p.mid || "配对" }}</span>
              <button type="button" class="icon-btn" aria-label="删除" @click="removePair(i)">✕</button>
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
            <div class="block-policy" id="rsBlockPolicyBox" />
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
