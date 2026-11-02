<script setup lang="ts">
const toast = useToastStore();
import { computed, onMounted, onUnmounted, ref, watch } from "vue";

import { useRemoteStore, type RemoteProfile } from "@/stores/remote";
import { useSettingsStore } from "@/stores/settings";
import ComputerRow from "@/components/hub/ComputerRow.vue";
import PcMenu from "@/components/hub/PcMenu.vue";
import SystemInfoDialog from "@/components/hub/SystemInfoDialog.vue";
import RemoteEditDialog from "@/components/hub/RemoteEditDialog.vue";
import RemoteListDialog from "@/components/hub/RemoteListDialog.vue";
import TunnelDialog from "@/components/hub/TunnelDialog.vue";
import { useToastStore } from "@/stores/toast";
import { useEscape } from "@/composables/useEscape";

const props = defineProps<{ addSeed?: number }>();

/**
 * 电脑列表（原 `ui/hub.html` 的 `#page-computers`）。
 *
 * ```
 * .cq-list-view
 *   .cq-head       标题 + 在线/离线/待生效统计
 *   .cq-toolbar    搜索 + 状态分段筛选 + 排序 + 刷新 + ⋮
 *   .cq-help       操作说明（可折叠）
 *   .cq-card
 *     .cq-grid
 *       **没有 .cq-thead** —— 旧版 CSS 里它是 `display:none`，
 *       2026-11-01 用户定案改成「移动端同款上下布局」的**卡片流**
 *       .cq-rows
 *         .cq-row   每台电脑一张卡（见 ComputerRow.vue）
 *   .cq-foot
 * ```
 *
 * ## 数据刷新（2026-11-02 定案，与「连不上时自动重载」是两件事）
 *
 * 定时刷新**常开**（间隔读 `dataRefreshSec`，默认 2 秒），只更新数据、
 * **不驱动刷新按钮转圈**；按钮只代表「用户手动点了刷新」。
 * 弱网容忍：连续失败达阈值才判离线（在 `stores/remote.ts` 里实现）。
 */

const remote = useRemoteStore();
const settings = useSettingsStore();

const query = ref("");
const filter = ref<"all" | "online" | "offline" | "pending">("all");
const helpOpen = ref(false);
// 旧版 key 就是 `anyboot.orderMode`（hub.html:7472），不能自造
const sortMode = ref<"custom" | "name" | "recent">(
  (() => {
    try {
      const saved = localStorage.getItem("anyboot.orderMode");
      if (saved === "custom" || saved === "name" || saved === "recent") return saved;
    } catch {
      /* 忽略 */
    }
    return "custom";
  })(),
);
/** 排序菜单开合（旧版 #cqSortMenu） */
const sortMenuOpen = ref(false);
const SORT_LABELS: Record<"custom" | "name" | "recent", string> = {
  custom: "自定义",
  name: "按名称",
  recent: "最近连接",
};
function pickSort(mode: "custom" | "name" | "recent") {
  sortMode.value = mode;
  sortMenuOpen.value = false;
  try {
    localStorage.setItem("anyboot.orderMode", mode);
  } catch {
    /* 忽略 */
  }
}
const refreshing = ref(false);
/** 系统信息弹层：非空时打开 */
const sysInfoId = ref<string | null>(null);

function openSysInfo(id: string) {
  if (id === "local") return;
  sysInfoId.value = id;
}



/** 本机行（固定第一条） */
const LOCAL_ROW: RemoteProfile = { id: "local", name: "本机", addresses: [] };

const allRows = computed<RemoteProfile[]>(() => [LOCAL_ROW, ...remote.servers]);

/** 系统信息弹层里显示的电脑名 */
const sysInfoTitle = computed(
  () => allRows.value.find((r) => r.id === sysInfoId.value)?.name ?? "",
);

/** 某台电脑的引导项信息 */
function infoOf(id: string) {
  return remote.bootInfo.get(id);
}

/** 是否待生效（GRUB 的 nextEntry 与 savedEntry 不同） */
function isPending(id: string): boolean {
  const info = infoOf(id);
  if (!info?.grub?.available) return false;
  const { next, saved } = info.grub;
  return Boolean(next) && Boolean(saved) && next !== saved;
}

/** 过滤 + 排序后的行 */
const rows = computed(() => {
  const keyword = query.value.trim().toLowerCase();
  let list = allRows.value.filter((row) => {
    const info = infoOf(row.id);
    const online = info?.online ?? false;
    if (filter.value === "online" && !online) return false;
    if (filter.value === "offline" && online) return false;
    if (filter.value === "pending" && !isPending(row.id)) return false;
    if (!keyword) return true;
    const haystack = [row.name, info?.addr ?? "", ...(row.addresses ?? [])].join(" ").toLowerCase();
    return haystack.includes(keyword);
  });
  if (sortMode.value === "name") {
    list = [...list].sort((a, b) => a.name.localeCompare(b.name, "zh-Hans-CN"));
  }
  return list;
});

const stats = computed(() => {
  let online = 0;
  let pending = 0;
  for (const row of allRows.value) {
    if (infoOf(row.id)?.online) online += 1;
    if (isPending(row.id)) pending += 1;
  }
  return {
    online,
    offline: allRows.value.length - online,
    pending,
    all: allRows.value.length,
  };
});

/** 手动刷新：**这是唯一会让按钮转圈的地方** */
async function manualRefresh() {
  if (refreshing.value) return;
  refreshing.value = true;
  toast.show("正在重新探测全部电脑…");
  try {
    await remote.refreshAllBoot();
    toast.show("全部电脑已重连重载");
  } catch (e) {
    toast.show("探测失败：" + e, "bad");
  } finally {
    refreshing.value = false;
  }
}

// ---- 定时数据刷新（常开；间隔读设置；**不驱动刷新按钮**）----
let timer: number | null = null;

function stopTimer() {
  if (timer !== null) {
    window.clearInterval(timer);
    timer = null;
  }
}

function startTimer() {
  stopTimer();
  const sec = settings.dataRefreshSec;
  if (sec <= 0) return; // 用户关了数据刷新
  timer = window.setInterval(() => {
    if (document.hidden) return;
    void remote.refreshAllBoot(); // 静默：不碰 refreshing
  }, sec * 1000);
}

onMounted(async () => {
  consumeAddSeed();
  startTimer();

  // 隧道停止事件（旧版 hub-tunnel-stopped）：刷新隧道列表
  const { listen } = await import("@tauri-apps/api/event");
  unlisteners.push(await listen("hub-tunnel-stopped", () => void listTunnels()));
  // 验收信息写进 trace（macOS 无法 attach devtools）
  window.setTimeout(() => {
    const line =
      `[vue-computerlist] rows=${rows.value.length} total=${allRows.value.length} ` +
      `online=${stats.value.online} offline=${stats.value.offline} pending=${stats.value.pending} ` +
      `bootInfoLoaded=${remote.bootInfo.size}`;
    void import("@/api/tauri")
      .then(({ isTauri, call }) =>
        isTauri()
          ? call<void>("hub_report_error", { source: "vue-computerlist", message: line, stack: "" })
          : undefined,
      )
      .catch(() => {});
  }, 3000);
});
onUnmounted(() => {
  stopTimer();
  for (const off of unlisteners) off();
});
// 设置里改了间隔要重建定时器（否则旧间隔跑到刷新页面为止）
watch(() => settings.dataRefreshSec, startTimer);
// 外部（电脑页「＋ 添加」）触发：打开服务端编辑弹层
/**
 * 「＋ 添加」跨页触发：
 * 因为 ComputerList 是 `v-if` 按需挂载的，点击时它**刚被创建**，
 * 此刻 watch 的 prev 已经等于新 seed → 不会触发。所以在 onMounted 里
 * 检查一次「挂载时 seed 已 > 0 且本组件正是因此被创建」。
 */
let addSeedConsumed = false;
function consumeAddSeed() {
  if (addSeedConsumed || !props.addSeed) return;
  addSeedConsumed = true;
  rsEditing.value = "";
  rsEditOpen.value = true;
}
watch(() => props.addSeed, consumeAddSeed);

/** 事件解绑句柄 */
const unlisteners: (() => void)[] = [];

/** 当前选中的电脑（旧版 activeRemote，默认 local） */
const activeId = ref("local");

/** 隧道编辑弹层（旧版 #tunnelOverlay）+ 隧道启停 */
const tunnelOpen = ref(false);
const tunnelEditing = ref<string>("");

function openTunnel(id: string) {
  void listTunnels();
  tunnelEditing.value = id;
  tunnelOpen.value = true;
}

function onSaveTunnel(payload: Record<string, unknown>) {
  tunnelOpen.value = false;
  void saveTunnel(payload);
}

/** 服务端编辑 / 列表弹层（旧版 #rsEditOverlay / #rsListOverlay） */
const rsEditOpen = ref(false);
const rsListOpen = ref(false);
const rsEditing = ref<string>("");

/** PcMenu 顶部操作（旧版 data-act） */
function onMenuAction(act: string) {
  if (act === "add") {
    rsEditing.value = "";
    rsEditOpen.value = true;
  } else if (act === "edit") {
    const id = activeId.value;
    if (!id || id === "local") {
      toast.show("本机是默认连接，不可编辑；选中其它电脑那一行后再编辑");
      return;
    }
    rsEditing.value = id;
    rsEditOpen.value = true;
  } else if (act === "list") {
    rsListOpen.value = true;
  } else if (act === "reloadCurrent") {
    if (!activeId.value || activeId.value === "local") {
      toast.show("先点一行电脑选中它，再重载", "bad");
      return;
    }
    toast.show("正在重载「" + (remote.servers.find((s) => s.id === activeId.value)?.name ?? "") + "」…");
    void import("@/api/tauri").then(({ call }) => call("remote_boot_info", { serverId: activeId.value }).catch((e) => toast.show("重载失败：" + e, "bad")));
  } else if (act === "reloadAll") {
    void import("@/api/tauri").then(({ call }) => call("remote_list").catch(() => {}));
  }
}

function openRemoteEdit(id: string) {
  rsEditing.value = id;
  rsEditOpen.value = true;
}

async function saveRemote(payload: Record<string, unknown>) {
  rsEditOpen.value = false;
  try {
    await import("@/api/tauri").then(({ call }) => call("remote_save", { server: payload }));
    await remote.load();
  } catch {
    /* 忽略 */
  }
}

/** 电脑页面弹层（旧版 #cqViewOverlay） */
const viewOpen = ref(false);
const viewUrl = ref("");
const viewTitle = ref("");
const viewSeed = ref(0);

function openView(rid: string) {
  const row = remote.servers.find((s) => s.id === rid);
  viewTitle.value = row?.name ?? "电脑页面";
  const info = infoOf(rid);
  viewUrl.value = info?.addr ? `http://${info.addr}/hub.html` : "";
  viewSeed.value += 1;
  viewOpen.value = true;
}

function move(payload: { id: string; delta: number }) {
  const list = [...remote.servers];
  const index = list.findIndex((s) => s.id === payload.id);
  const next = index + payload.delta;
  if (index < 0 || next < 0 || next >= list.length) return;
  [list[index], list[next]] = [list[next]!, list[index]!];
  void import("@/api/tauri").then(({ call }) =>
    call("remote_reorder", { ids: list.map((s) => s.id) }).then(() => remote.load()),
  );
}

/** 行内 ⋮ 菜单（对齐旧版 `cqRowMenu`，hub.html:8158） */
const rowMenuId = ref<string | null>(null);

function openMore(id: string) {
  activeId.value = id;
  rowMenuId.value = rowMenuId.value === id ? null : id;
  // 旧版打开 ⋮ 时会 touch 一下（记最近使用）
  void import("@/api/tauri").then(({ call }) => call("remote_touch", { id }));
}

/** 行菜单动作分派（对齐旧版 picked.act 分支） */
function rowMenuAction(action: string, id: string) {
  rowMenuId.value = null;
  const row = renmoteRow(id);
  switch (action) {
    case "view":
      openView(id);
      break;
    case "goto":
      window.location.href = `hub.html?page=home&rid=${encodeURIComponent(id)}`;
      break;
    case "browser":
      void openInBrowser(id);
      break;
    case "refresh":
      void import("@/api/tauri").then(({ call }) => call("remote_test", { id })).then(() => remote.load()).catch((e) => toast.show("测试失败：" + e, "bad"));
      break;
    case "sysinfo":
      sysInfoId.value = id;
      break;
    case "edit":
      openRemoteEdit(id);
      break;
    case "copyip": {
      const addr = infoOf(id)?.addr || (row?.addresses?.[0] ?? "");
      void navigator.clipboard?.writeText(addr).catch(() => {});
      break;
    }
    case "delete":
      void import("@/api/tauri").then(({ call }) => call("remote_delete", { id })).then(() => remote.load()).catch((e) => toast.show("删除失败：" + e, "bad"));
      break;
  }
}

function renmoteRow(id: string) {
  return remote.servers.find((s) => s.id === id);
}

/** 用系统浏览器打开某台电脑的页面（旧版 remote_open_page） */
async function openInBrowser(id: string) {
  const info = infoOf(id);
  const url = info?.addr ? `http://${info.addr}/hub.html` : "";
  if (!url) {
    toast.show("这台电脑还没有可直连打开的地址", "bad");
    return;
  }
  const { call } = await import("@/api/tauri");
  await call("remote_open_page", { url })
    .then(() => toast.show("已在系统浏览器打开直连地址"))
    .catch((e) => toast.show("打开浏览器失败：" + e, "bad"));
}

/** 页面探活（旧版 hub_page_ping，给电脑页标签用） */
async function pingPage() {
  const { call } = await import("@/api/tauri");
  await call("hub_page_ping", { ready: true }).catch(() => {});
}

/** 让主窗聚焦（旧版 hub_focus_window） */
async function focusWindow() {
  const { call } = await import("@/api/tauri");
  await call("hub_focus_window", {}).catch(() => {});
}

/** 隧道列表 / 启停（旧版 hub_list_tunnels / hub_tunnel_start / hub_tunnel_stop） */
/** 隧道列表（旧版 hub_list_tunnels）；挂到 ⋮ 菜单前先刷新一遍 */
const tunnelRows = ref<unknown[]>([]);

async function listTunnels() {
  const { call } = await import("@/api/tauri");
  tunnelRows.value = ((await call("hub_list_tunnels", {}).catch(() => [])) as unknown[]) ?? [];
  return tunnelRows.value;
}

async function startTunnel(id: string) {
  const { call } = await import("@/api/tauri");
  await call("hub_tunnel_start", { tunnelId: id }).catch((e) => toast.show("启动隧道失败：" + e, "bad"));
}

async function stopTunnel(id: string) {
  const { call } = await import("@/api/tauri");
  await call("hub_tunnel_stop", { tunnelId: id }).catch((e) => toast.show("停止隧道失败：" + e, "bad"));
}

async function saveTunnel(payload: Record<string, unknown>) {
  const { call } = await import("@/api/tauri");
  await call("hub_save_tunnel", { tunnel: payload }).catch(() => {});
}

async function deleteTunnel(id: string) {
  const { call } = await import("@/api/tauri");
  await call("hub_delete_tunnel", { id }).catch((e) => toast.show("删除隧道失败：" + e, "bad"));
}

/** 开启 / 关闭「连接全部电脑」（旧版 remote_set_connect_all） */
async function setConnectAll(on: boolean) {
  const { call } = await import("@/api/tauri");
  await call("remote_set_connect_all", { on }).catch((e) => toast.show("设置失败：" + e, "bad"));
}

/** 行级请求锁（旧版 cqBusy：同一行同一动作在途时不重复发） */
const cqBusy = new Set<string>();

function switchBoot(payload: Record<string, unknown>) {
  const key = `${payload.serverId}:boot:${payload.kind}`;
  if (cqBusy.has(key)) return;
  cqBusy.add(key);
  void remote
    .switchBoot(payload)
    .then(() => toast.show("已切换下次启动项"))
    .catch((e) => toast.show("切换失败：" + e, "bad"))
    .finally(() => cqBusy.delete(key));
}

function power(payload: Record<string, unknown>) {
  const key = `${payload.serverId}:act:${payload.action}`;
  if (cqBusy.has(key)) return;
  cqBusy.add(key);
  void remote
    .power(payload)
    .then(() => toast.show("已发送电源指令"))
    .catch((e) => toast.show("电源操作失败：" + e, "bad"))
    .finally(() => cqBusy.delete(key));
}
useEscape(viewOpen, () => { viewOpen.value = false });
</script>

<template>
  <div class="cq-list-view">
    <div class="cq-head">
      <div class="cq-head-txt">
        <h1>电脑列表</h1>
        <p class="cq-sub">
          远程管理<span class="slash">-</span>电脑 · 选中一行即可发送电源指令或切换引导项
        </p>
      </div>
      <div class="cq-stats">
        <div class="cq-stat cq-stat--on"><i>在线</i><b id="cqStOnline">{{ stats.online }}</b></div>
        <div class="cq-stat cq-stat--off"><i>离线</i><b id="cqStOffline">{{ stats.offline }}</b></div>
        <div class="cq-stat cq-stat--pend"><i>待生效</i><b id="cqStPending">{{ stats.pending }}</b></div>
      </div>
    </div>

    <div class="cq-toolbar">
      <div class="cq-search">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
             stroke-linecap="round" aria-hidden="true">
          <circle cx="11" cy="11" r="7" />
          <path d="M20 20l-3.5-3.5" />
        </svg>
        <input
        id="cqQ"
        v-model="query"
          type="search"
          placeholder="搜索电脑名称、IP 或 MAC"
          aria-label="搜索电脑"
          spellcheck="false"
          autocomplete="off"
        />
      </div>
      <div class="cq-seg" id="cqSeg" role="tablist" aria-label="状态筛选">
        <button
          v-for="f in [
            { id: 'all', label: '全部', n: stats.all },
            { id: 'online', label: '在线', n: stats.online },
            { id: 'offline', label: '离线', n: stats.offline },
            { id: 'pending', label: '待生效', n: stats.pending },
          ]"
          :key="f.id"
          role="tab"
          :aria-selected="filter === f.id"
          @click="filter = f.id as typeof filter"
        >
          {{ f.label }} <span class="n" :id="`cqN${f.id.charAt(0).toUpperCase()}${f.id.slice(1)}`">{{ f.n }}</span>
        </button>
      </div>
      <span class="cq-spacer" />
      <button type="button" id="cqHelpToggle" class="link-btn cq-help-btn" :aria-expanded="helpOpen" @click="helpOpen = !helpOpen">
        操作说明
      </button>
      <button
        type="button"
        id="cqSort"
        class="link-btn menu-anchor"
        title="选择电脑列表的排序方式（电脑页标签栏跟着同一套顺序）"
        :aria-expanded="sortMenuOpen"
        @click.stop="sortMenuOpen = !sortMenuOpen"
      >
        排序 <span id="cqSortText">{{ SORT_LABELS[sortMode] }}</span>
      </button>
      <button type="button" id="cqRefresh" class="link-btn" :disabled="refreshing" title="重新探测全部电脑的在线状态与引导项" @click="manualRefresh">
        <span id="cqRefreshText">{{ refreshing ? "探测中" : "刷新" }}</span>
      </button>
      <button type="button" class="link-btn" title="探测当前电脑的页面是否可访问" @click="pingPage">探活</button>
      <button type="button" class="link-btn" title="让主窗跳到前台" @click="focusWindow">聚焦主窗</button>
      <button type="button" class="link-btn" title="管理已添加的 AnyBoot 服务端（编辑 / 删除 / 新增）" @click="rsListOpen = true">
        服务端
      </button>
      <button type="button" class="link-btn" title="添加一台 AnyBoot 服务端" @click="rsEditing = ''; rsEditOpen = true">
        ＋服务端
      </button>
      <button type="button" class="link-btn" title="给当前电脑新建一条 SSH 隧道" @click="openTunnel(activeId)">
        隧道
      </button>
      <button
        type="button"
        class="link-btn"
        title="删除当前电脑的隧道配置"
        @click="deleteTunnel((remote.servers.find((s) => s.id === activeId) as any)?.tunnelId ?? '')"
      >
        删隧道
      </button>
      <label class="link-btn" title="是否同时探测全部电脑（关掉只探当前选中的）">
        <input
          type="checkbox"
          :checked="Boolean(remote.connectAll)"
          @change="setConnectAll(!remote.connectAll)"
        />
        连接全部
      </label>
      <PcMenu id="cqMore" @menu-action="onMenuAction" />
    </div>

    <div v-show="helpOpen" class="cq-help" id="cqHelp">
      <h4>操作说明</h4>
      <ul>
        <li>电源操作<b>点了立即执行，不再弹确认框</b>（与「电脑」页电源悬浮球同一套说明）。</li>
        <li>唤醒：发魔术包让电脑开机 —— 需要配好 MAC 并在主板开启网络唤醒。</li>
        <li>睡眠：电脑挂起到内存，网页服务立刻停掉 —— 会显示离线，要叫醒请点「唤醒」（需配好 MAC 并开启网络唤醒）。</li>
        <li>关机：电脑立刻关机，网页服务随之中断 —— 关机后只能用「唤醒」开机（需配好 MAC），或到机器旁按电源键。</li>
        <li>重启：电脑立刻重启，网页会话断开 —— 重启完成后会自动恢复在线。</li>
        <li>没配电源路由时走这台电脑自己的 /api/power（需要能连上它）；配了路由则发局域网 sol 包。</li>
        <li>rEFInd / GRUB / systemd-boot / OpenCore / Clover 的选择写入该机引导配置，<b>下次重启时生效</b>，可点「撤销」恢复原值。</li>
        <li>四个电源键任何时候都能按、不会置灰（与移动端一致）；设备离线时引导项不可改。</li>
        <li>显示「未检测到」= 该机没部署对应引导管理器。</li>
      </ul>
    </div>

    <div class="cq-card">
      <div class="cq-grid" role="table" aria-label="电脑列表">
        <!-- 表头（旧版静态 HTML 里就有，CSS 里 `display:none` —— 卡片流时代不用它，
             但 DOM 保留：一是与旧版一致，二是它给 .cq-col 提供字号/颜色继承基线） -->
        <div class="cq-thead" role="row" aria-hidden="true">
          <div class="cq-col cq-col-dev" role="columnheader">电脑</div>
          <div class="cq-col cq-col-st" role="columnheader">状态</div>
          <div class="cq-col cq-col-boot" role="columnheader">rEFInd 启动项</div>
          <div class="cq-col cq-col-boot" role="columnheader">GRUB 启动项</div>
          <div class="cq-col cq-col-pwr" role="columnheader">电源</div>
          <div class="cq-col cq-col-more" role="columnheader" />
        </div>
        <div class="cq-rows" id="cqTbody">
          <ComputerRow
            v-for="row in rows"
            :key="row.id"
            :row="row"
            :info="infoOf(row.id)"
            :pending="isPending(row.id)"
            :selected="activeId === row.id"
            :sort-mode="sortMode"
            :order-index="remote.servers.findIndex((s) => s.id === row.id)"
            :order-total="remote.servers.length"
            @select="activeId = $event"
            @switch-boot="switchBoot"
            @power="power"
            @sys-info="openSysInfo"
            @move="move"
            @more="openMore"
            @view="openView"
            @open-browser="openInBrowser"
            @start-tunnel="startTunnel"
            @stop-tunnel="stopTunnel"
          />
          </div>
      </div>
      <div v-if="!rows.length" class="cq-empty" id="cqEmpty">
        <div>没有匹配的电脑</div>
        <p id="cqEmptyHint">换个关键词，或把筛选切回「全部」</p>
      </div>
    </div>

    <!-- 电脑页面弹层（旧版 #cqViewOverlay）：宽高用 min() 撑满，iframe 撑余下空间 -->
    <div v-if="viewOpen" id="cqViewOverlay" class="overlay" @click.self="viewOpen = false">
      <div
        class="dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="cqViewTitle"
        style="width: min(1180px, 96vw); height: min(820px, 92vh); max-height: 92vh; display: flex; flex-direction: column; overflow: hidden; padding: 0"
      >
        <div class="dialog-head" style="flex: none">
          <h3 id="cqViewTitle">{{ viewTitle }}</h3>
          <div style="display: flex; align-items: center; gap: 6px">
            <button type="button" class="btn small" id="cqViewReload" @click="viewSeed += 1">重载</button>
            <button type="button" class="icon-btn" id="cqViewClose" aria-label="关闭" @click="viewOpen = false">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
                <path d="M6 6l12 12M18 6L6 18" />
              </svg>
            </button>
          </div>
        </div>
        <div class="pc-frame-host" id="pcFrameHost" style="flex: 1; min-height: 0; border: 0; border-radius: 0">
          <iframe
            v-if="viewUrl"
            :key="viewSeed"
            :src="viewUrl"
            :title="viewTitle"
            style="width: 100%; height: 100%; border: 0; background: #fff"
          />
          <div v-else id="pcEmpty" class="pc-empty">
            <div>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" aria-hidden="true">
                <rect x="3" y="4" width="18" height="16" rx="2" />
                <path d="M3 9h18M8 4v5" />
              </svg>
              <p><b>页面加载中…</b></p>
              <p>这台电脑的 AnyBoot 页面会显示在这里；连不上时会给出排查提示。</p>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 排序方式菜单（旧版 #cqSortMenu，hub.html:2500） -->
    <div
      v-if="sortMenuOpen"
      class="pc-menu cq-sort-menu"
      id="cqSortMenu"
      role="menu"
      aria-label="排序方式"
      @click.stop
    >
      <div class="menu-caption">排序方式</div>
      <button role="menuitem" data-order="custom" :class="{ on: sortMode === 'custom' }" @click="pickSort('custom')">自定义（我用 ↑ ↓ 排的）</button>
      <button role="menuitem" data-order="name" :class="{ on: sortMode === 'name' }" @click="pickSort('name')">名称 A → Z</button>
      <button role="menuitem" data-order="recent" :class="{ on: sortMode === 'recent' }" @click="pickSort('recent')">最近连接</button>
      <div class="menu-divider" role="separator" />
      <div class="menu-caption" id="cqSortHint">自定义：每行左侧 ↑ ↓ 调整顺序</div>
    </div>

    <!-- 行内 ⋮ 菜单（旧版 cqRowMenu，hub.html:8158） -->
    <div v-if="rowMenuId" class="overlay-transparent" @click="rowMenuId = null">
      <div class="menu cq-row-menu" role="menu" @click.stop>
        <div class="caption">{{ renmoteRow(rowMenuId)?.name }}</div>
        <button type="button" role="menuitem" @click="rowMenuAction('view', rowMenuId!)">打开页面（内嵌查看）</button>
        <button type="button" role="menuitem" @click="rowMenuAction('goto', rowMenuId!)">在电脑页打开（选中标签）</button>
        <button type="button" role="menuitem" @click="rowMenuAction('browser', rowMenuId!)">在系统浏览器打开</button>
        <div class="sep" />
        <button type="button" role="menuitem" @click="rowMenuAction('refresh', rowMenuId!)">重新探测这台电脑</button>
        <button type="button" role="menuitem" @click="rowMenuAction('sysinfo', rowMenuId!)">系统信息（系统 / CPU / 内存）</button>
        <template v-if="rowMenuId !== 'local'">
          <button type="button" role="menuitem" @click="rowMenuAction('edit', rowMenuId!)">编辑这台电脑</button>
          <button type="button" role="menuitem" @click="rowMenuAction('copyip', rowMenuId!)">复制地址</button>
          <div class="sep" />
          <button type="button" role="menuitem" class="danger" @click="rowMenuAction('delete', rowMenuId!)">删除这台电脑</button>
        </template>
      </div>
    </div>

    <!-- 服务端编辑 / 列表（旧版 #rsEditOverlay / #rsListOverlay） -->
    <RemoteEditDialog
      :open="rsEditOpen"
      :remote="remote.servers.find((s) => s.id === rsEditing) ?? null"
      @save="saveRemote"
      @close="rsEditOpen = false"
    />
    <!-- 隧道编辑（旧版 #tunnelOverlay） -->
    <TunnelDialog
      :open="tunnelOpen"
      :server="remote.servers.find((s) => s.id === tunnelEditing) ?? null"
      @save="onSaveTunnel"
      @close="tunnelOpen = false"
    />

    <RemoteListDialog
      :open="rsListOpen"
      :servers="remote.servers"
      @edit="openRemoteEdit"
      @remove="remote.remove($event)"
      @add="rsEditing = ''; rsEditOpen = true"
      @close="rsListOpen = false"
    />

    <SystemInfoDialog
      :server-id="sysInfoId"
      :title="sysInfoTitle"
      @close="sysInfoId = null"
    />

    <div class="cq-foot">
      电源操作立即执行（不再确认）· 唤醒离线也能发（魔术包）· 引导项变更在下次重启时生效
    </div>
  </div>
</template>

<style scoped>
.cq-help ul {
  margin: 0;
  padding-left: 18px;
}
.cq-help li {
  font-size: 12.5px;
  line-height: 1.7;
  color: var(--fg-dim, #4b5563);
}
.cq-empty {
  padding: 32px 16px;
  text-align: center;
  color: var(--fg-faint, #9ca3af);
  font-size: 13px;
}
</style>
