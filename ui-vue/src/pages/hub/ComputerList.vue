<script setup lang="ts">
const toast = useToastStore();
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";

import { useRemoteStore, type RemoteProfile } from "@/stores/remote";
import { useSettingsStore } from "@/stores/settings";
import ComputerRow from "@/components/hub/ComputerRow.vue";
import PcMenu from "@/components/hub/PcMenu.vue";
import SystemInfoDialog from "@/components/hub/SystemInfoDialog.vue";
import RemoteEditDialog from "@/components/hub/RemoteEditDialog.vue";
import RemoteListDialog from "@/components/hub/RemoteListDialog.vue";
import { useToastStore } from "@/stores/toast";
import { t } from "@/i18n";
import { useEscape } from "@/composables/useEscape";

const props = defineProps<{ addSeed?: number; remoteAction?: { act: string; n: number } }>();

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
const sortBtnRef = ref<HTMLElement | null>(null);
// 排序菜单是 position:fixed，必须按锚点按钮算 top/left（旧版 7584 同款）：
// 之前只有 width 没 top/left → 飘到窗口左上/右上。打开时算一次，clamp 到窗口内。
watch(sortMenuOpen, async (open) => {
  if (!open) return;
  await nextTick();
  const btn = sortBtnRef.value;
  const menu = document.getElementById("cqSortMenu");
  if (!btn || !menu) return;
  const rect = btn.getBoundingClientRect();
  const width = menu.offsetWidth || 244;
  menu.style.top = rect.bottom + 6 + "px";
  menu.style.left = Math.max(8, Math.min(rect.left, window.innerWidth - width - 8)) + "px";
});
const SORT_LABELS: Record<"custom" | "name" | "recent", string> = {
  custom: "hub.list.sortCustom",
  name: "hub.list.sortName",
  recent: "hub.list.sortRecent",
};

/** 操作说明 9 条（key 列表，模板 v-for 渲染） */
const HELP_ITEMS = [
  "hub.list.helpBody1", "hub.list.helpBody2", "hub.list.helpBody3",
  "hub.list.helpBody4", "hub.list.helpBody5", "hub.list.helpBody6",
  "hub.list.helpBody7", "hub.list.helpBody8", "hub.list.helpBody9",
];
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
const LOCAL_ROW: RemoteProfile = { id: "local", name: "", addresses: [] };

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

/** 所有电脑里「检测到(available)」的通道 kind 并集（固定顺序 sdboot/opencore/clover），
 *  与旧版 cqBootColumns 一致：保证各行通道列对齐，某行没检测到就占一列显示「未检测到」。 */
const CHANNEL_ORDER = ["channel:sdboot", "channel:opencore", "channel:clover"];
const channelKinds = computed<string[]>(() => {
  const seen = new Set<string>();
  for (const row of allRows.value) {
    const info = infoOf(row.id);
    for (const ch of info?.channels ?? []) {
      if (ch.available) seen.add(ch.kind);
    }
  }
  // 按固定顺序排序，其余未知通道追加在后
  const ordered = CHANNEL_ORDER.filter((k) => seen.has(k));
  for (const k of seen) if (!ordered.includes(k)) ordered.push(k);
  return ordered;
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
  toast.show(t("hub.list.probingAll"));
  try {
    await remote.refreshAllBoot(true);
    toast.show(t("hub.list.reloadedAll"));
  } catch (e) {
    toast.show(t("hub.list.probeFailed", { msg: String(e) }), "bad");
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
  // 兜底：若全局（HubApp）尚未加载服务器列表，本页自行拉一次，避免空数据
  if (!remote.loaded) {
    try { await remote.load(); } catch { /* 忽略 */ }
  }
  startTimer();
  // 【修复选中前后布局跳动】进列表就预拉所有电脑的引导项/在线状态（对齐旧版
  // cqRefresh「重新探测全部电脑」）。之前只有选中某行或定时器 tick 才 load，
  // 导致「读取中(矮) → 加载完(高)」行高跳动，看着像"选中就变形"。预拉后数据齐、行高稳定，
  // 选中只加高亮不改尺寸。
  void Promise.all(remote.servers.map((s) => remote.refreshBoot(s.id).catch(() => {})));

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
// 【修复】组件是 v-if 按需挂载的，每次重进「电脑列表」页都会重建 —— 组件内普通变量
// 会随 setup 重置。用 sessionStorage 记住「已消费到哪个 seed」跨重建保留：只有
// addSeed 比上次消费值更大（真正新点了一次「添加」）才弹，否则不误弹空表单。
const ADD_SEED_KEY = "anyboot.hub.addSeedConsumed";
function consumeAddSeed() {
  if (!props.addSeed) return;
  let last = 0;
  try { last = Number(sessionStorage.getItem(ADD_SEED_KEY) ?? 0) || 0; } catch { last = 0; }
  if (props.addSeed <= last) return;
  try { sessionStorage.setItem(ADD_SEED_KEY, String(props.addSeed)); } catch { /* ignore */ }
  rsEditing.value = "";
  rsEditOpen.value = true;
}
watch(() => props.addSeed, consumeAddSeed);

// 顶栏 ⋮ 的 edit/list/reloadCurrent/reloadAll → 执行真实动作（不再是空选项）
watch(() => props.remoteAction?.n, () => {
  const act = props.remoteAction?.act ?? "";
  if (!act) return;
  if (act === "edit") {
    if (!activeId.value || activeId.value === "local") {
      toast.show(t("hub.list.editLocalOnly"));
      return;
    }
    openRemoteEdit(activeId.value);
  } else if (act === "list") {
    rsListOpen.value = true;
  } else if (act === "reloadCurrent") {
    if (!activeId.value || activeId.value === "local") {
      toast.show(t("hub.list.selectRowFirst"), "bad");
      return;
    }
    toast.show(t("hub.list.reloading", { name: remote.servers.find((s) => s.id === activeId.value)?.name ?? "" }));
    void remote.refreshBoot(activeId.value, true).catch((e) => toast.show("重载失败：" + e, "bad"));
  } else if (act === "reloadAll") {
    void manualRefresh();
  }
});

/** 事件解绑句柄 */
const unlisteners: (() => void)[] = [];

/** 当前选中的电脑（旧版 activeRemote，默认 local） */
const activeId = ref("local");

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
      toast.show(t("hub.list.editLocalOnly"));
      return;
    }
    rsEditing.value = id;
    rsEditOpen.value = true;
  } else if (act === "list") {
    rsListOpen.value = true;
  } else if (act === "reloadCurrent") {
    if (!activeId.value || activeId.value === "local") {
      toast.show(t("hub.list.selectRowFirst"), "bad");
      return;
    }
    toast.show(t("hub.list.reloading", { name: remote.servers.find((s) => s.id === activeId.value)?.name ?? "" }));
    // 必须走 store.refreshBoot：它把 remote_boot_info 结果存进 store 并刷新行；直接 call 只拿数据不更新 UI
    void remote.refreshBoot(activeId.value).catch((e) => toast.show("重载失败：" + e, "bad"));
  } else if (act === "reloadAll") {
    void manualRefresh();
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
  // 旧版 frame.title：本机页面 / 服务端页面
  viewTitle.value = rid === "local" ? t("hub.list.viewTitleLocal") : t("hub.list.viewTitleRemote");
  const info = infoOf(rid);
  // 统一化（用户 2026-11-15）：电脑页访问网址优先用**嗅探可用**的地址
  // （probe_results 里 ok=true 的第一个），都不通/还没探测才回退 info.addr
  // （后端当前连接地址）—— 否则点卡片进电脑页会加载一个探测不通的地址白屏。
  const okAddr = (info?.probe_results ?? [])
    .find((r) => r?.ok && r.address)?.address ?? "";
  if (!okAddr && !info?.addr) {
    viewUrl.value = "";
  } else {
    // 旧版 pcConnUrl（lan 赢家）：origin + /?token=...
    const token = encodeURIComponent(row?.token ?? "");
    let url = `http://${okAddr || info!.addr}/?token=${token}`;
    // withGuestTheme：当前页亮暗 + 引导页主题偏好（本机查看方偏好）
    const dark = document.documentElement.classList.contains("dark") ? "dark" : "light";
    let theme = "";
    try { theme = localStorage.getItem("anyboot.hub.boot-theme") || ""; } catch { /* 忽略 */ }
    url += `&abUi=${dark}` + (theme ? `&abTheme=${encodeURIComponent(theme)}` : "");
    viewUrl.value = url;
  }
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
    case "view": {
      // 旧版 cqOpenPage：openMode=goto 时跳「电脑」页并选中，否则内嵌查看
      let mode = "inline";
      try { mode = localStorage.getItem("cqOpenMode") === "goto" ? "goto" : "inline"; } catch { /* 默认 */ }
      if (mode === "goto") {
        window.dispatchEvent(new CustomEvent("cq-goto-page", { detail: id }));
      } else {
        openView(id);
      }
      break;
    }
    case "goto": {
      // 旧版 cqGotoPage：SPA 内切「电脑」页并选中该 chip（不重载）
      window.dispatchEvent(new CustomEvent("cq-goto-page", { detail: id }));
      break;
    }
    case "browser":
      void openInBrowser(id);
      break;
    case "refresh": {
      const nm = remote.servers.find((s) => s.id === id)?.name ?? "";
      toast.show(t("hub.list.probingOne", { name: nm }));
      void remote.refreshBoot(id, true)
        .then(() => toast.show(t("hub.list.reprobed", { name: nm })))
        .catch((e) => toast.show("重新探测失败：" + e, "bad"));
      break;
    }
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
// 浏览器打开用的连接候选（用户要求：多连接时让用户选择走哪个地址；移动端/web 同理）
const browserPickOpen = ref(false);
const browserPickId = ref("");
const browserPickList = ref<{ label: string; address: string; ok: boolean | null }[]>([]);

const ROUTE_NAME: Record<string, string> = {
  lan: "hub.route.lan", jump: "hub.route.jump", tunnel: "Cloudflare", ts: "Tailscale", funnel: "Funnel", tsnet: "hub.route.builtin",
};

/** 收集这台电脑所有可用于浏览器打开的地址（探测出的各通道地址 + 配置的远程配对地址），去重。 */
function browserCandidates(id: string): { label: string; address: string; ok: boolean | null }[] {
  const row = remote.servers.find((s) => s.id === id) as Record<string, unknown> | undefined;
  const info = infoOf(id);
  const out: { label: string; address: string; ok: boolean | null }[] = [];
  const seen = new Set<string>();
  const push = (label: string, address: string, ok: boolean | null) => {
    const a = String(address || "").trim();
    if (!a || seen.has(a)) return;
    seen.add(a);
    out.push({ label, address: a, ok });
  };
  // 1) 探测出结果的各通道地址（带通/不通）
  for (const r of (info?.probe_results ?? [])) {
    if (!r?.address) continue;
    const nm = ROUTE_NAME[r.kind ?? "lan"] ? t(ROUTE_NAME[r.kind ?? "lan"]) : (r.remote ? t("hub.route.remote") : (r.kind || t("hub.route.lan")));
    push(nm, r.address, !!r.ok);
  }
  // 2) 配置的局域网地址（探测里没有时兜底带出）
  for (const a of ((row?.addresses as string[] | undefined) ?? [])) push(t("hub.route.lan"), String(a), null);
  // 3) 远程配对地址
  for (const p of ((row?.pairs as { kind?: string; lane?: string; addr?: string }[] | undefined) ?? [])) {
    if (!p?.addr) continue;
    const rk = p.kind === "cloudflare" ? "tunnel" : p.kind === "tsnet" ? "tsnet" : p.kind === "ts" ? (p.lane === "funnel" ? "funnel" : "ts") : (p.kind || "tunnel");
    push(ROUTE_NAME[rk] ? t(ROUTE_NAME[rk]) : (p.kind || t("hub.route.remote")), String(p.addr), null);
  }
  return out;
}

async function openInBrowser(id: string) {
  const row = remote.servers.find((s) => s.id === id);
  const token = encodeURIComponent(String(row?.token ?? ""));
  const cands = browserCandidates(id);
  if (!cands.length) {
    toast.show(t("hub.list.noOpenAddr"), "bad");
    return;
  }
  if (cands.length === 1) {
    await doOpenBrowser(cands[0]!.address, token);
    return;
  }
  // 多连接 → 弹层让用户选
  browserPickId.value = id;
  browserPickList.value = cands;
  browserPickOpen.value = true;
}

async function doOpenBrowser(address: string, token: string) {
  const url = `http://${address}/?token=${token}`;
  const { isTauri } = await import("@/api/tauri");
  if (!isTauri()) {
    window.open(url, "_blank"); // 网页端/移动端：直接开新标签
    toast.show(t("hub.list.opened", { addr: address }));
    return;
  }
  const { call } = await import("@/api/tauri");
  await call("remote_open_page", { url })
    .then(() => toast.show(t("hub.list.openedSystem", { addr: address })))
    .catch((e) => toast.show("打开浏览器失败：" + e, "bad"));
}

async function pickBrowserAddr(c: { address: string }) {
  const row = remote.servers.find((s) => s.id === browserPickId.value);
  const token = encodeURIComponent(String(row?.token ?? ""));
  browserPickOpen.value = false;
  await doOpenBrowser(c.address, token);
}

/** 页面探活（旧版 hub_page_ping，给电脑页标签用） */

/** 让主窗聚焦（旧版 hub_focus_window） */

/** 隧道列表 / 启停（旧版 hub_list_tunnels / hub_tunnel_start / hub_tunnel_stop） */
/** 隧道列表（旧版 hub_list_tunnels）；挂到 ⋮ 菜单前先刷新一遍 */






/** 开启 / 关闭「连接全部电脑」（旧版 remote_set_connect_all） */

/** 行级请求锁（旧版 cqBusy：同一行同一动作在途时不重复发） */
const cqBusy = new Set<string>();

function switchBoot(payload: Record<string, unknown>) {
  const key = `${payload.serverId}:boot:${payload.kind}`;
  if (cqBusy.has(key)) return;
  cqBusy.add(key);
  void remote
    .switchBoot(payload)
    .catch((e) => toast.show("切换失败：" + e, "bad"))
    .finally(() => cqBusy.delete(key));
}

function power(payload: Record<string, unknown>) {
  const key = `${payload.serverId}:act:${payload.action}`;
  if (cqBusy.has(key)) return;
  cqBusy.add(key);
  void remote
    .power(payload)
    .then(() => toast.show(t("hub.list.powerSent")))
    .catch((e) => toast.show("电源操作失败：" + e, "bad"))
    .finally(() => cqBusy.delete(key));
}
useEscape(viewOpen, () => { viewOpen.value = false });
</script>

<template>
  <div class="cq-list-view">
    <div class="cq-head">
      <div class="cq-head-txt">
        <h1>{{ $t("hub.list.title") }}</h1>
      </div>
      <div class="cq-stats">
        <button type="button" class="cq-stat cq-stat--all" :class="{ 'is-active': filter === 'all' }" @click="filter = 'all'">
          <i>{{ $t("hub.list.all") }}</i><b id="cqStAll">{{ stats.all }}</b>
        </button>
        <button type="button" class="cq-stat cq-stat--on" :class="{ 'is-active': filter === 'online' }" @click="filter = 'online'">
          <i>{{ $t("hub.list.online") }}</i><b id="cqStOnline">{{ stats.online }}</b>
        </button>
        <button type="button" class="cq-stat cq-stat--off" :class="{ 'is-active': filter === 'offline' }" @click="filter = 'offline'">
          <i>{{ $t("hub.list.offline") }}</i><b id="cqStOffline">{{ stats.offline }}</b>
        </button>
        <button type="button" class="cq-stat cq-stat--pend" :class="{ 'is-active': filter === 'pending' }" @click="filter = 'pending'">
          <i>{{ $t("hub.list.pending") }}</i><b id="cqStPending">{{ stats.pending }}</b>
        </button>
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
          :placeholder="$t('hub.list.search')"
          :aria-label="$t('hub.list.searchAria')"
          spellcheck="false"
          autocomplete="off"
        />
      </div>

      <span class="cq-spacer" />
      <button type="button" id="cqHelpToggle" class="link-btn cq-help-btn" :aria-expanded="helpOpen" @click="helpOpen = !helpOpen">
        {{ $t("hub.list.help") }}
      </button>
      <button
        type="button"
        id="cqSort"
        ref="sortBtnRef"
        class="link-btn menu-anchor"
        :title="$t('hub.list.sortTitle')"
        :aria-expanded="sortMenuOpen"
        @click.stop="sortMenuOpen = !sortMenuOpen"
      >
        {{ $t("hub.list.sort") }} <span id="cqSortText">{{ $t(SORT_LABELS[sortMode]) }}</span>
      </button>
      <button type="button" id="cqRefresh" class="link-btn" :disabled="refreshing" :title="$t('hub.list.refreshTitle')" @click="manualRefresh">
        <span id="cqRefreshText">{{ refreshing ? $t("hub.list.probing") : $t("common.refresh") }}</span>
      </button>
      <PcMenu id="cqMore" @menu-action="onMenuAction" />
    </div>

    <div v-show="helpOpen" class="cq-help" id="cqHelp">
      <h4>{{ $t("hub.list.help") }}</h4>
      <ul>
        <li v-for="(k, i) in HELP_ITEMS" :key="i">{{ $t(k) }}</li>
      </ul>
    </div>

    <div class="cq-card">
      <div class="cq-grid" role="table" :aria-label="$t('hub.list.title')">
        <!-- 表头（旧版静态 HTML 里就有，CSS 里 `display:none` —— 卡片流时代不用它，
             但 DOM 保留：一是与旧版一致，二是它给 .cq-col 提供字号/颜色继承基线） -->
        <div class="cq-thead" role="row" aria-hidden="true">
          <div class="cq-col cq-col-dev" role="columnheader">{{ $t("nav.computer") }}</div>
          <div class="cq-col cq-col-st" role="columnheader">{{ $t("hub.list.colStatus") }}</div>
          <div class="cq-col cq-col-boot" role="columnheader">{{ $t("hub.boot.col", { name: "rEFInd" }) }}</div>
          <div class="cq-col cq-col-boot" role="columnheader">{{ $t("hub.boot.col", { name: "RefindPlus" }) }}</div>
          <div class="cq-col cq-col-boot" role="columnheader">{{ $t("hub.boot.col", { name: "GRUB" }) }}</div>
          <div class="cq-col cq-col-pwr" role="columnheader">{{ $t("hub.list.colPower") }}</div>
          <div class="cq-col cq-col-more" role="columnheader" />
        </div>
        <div class="cq-rows" id="cqTbody">
          <ComputerRow
            v-for="row in rows"
            :key="row.id"
            :row="row"
            :info="infoOf(row.id)"
            :channel-kinds="channelKinds"
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
          />
          </div>
      </div>
      <div v-if="!rows.length" class="cq-empty" id="cqEmpty">
        <div>{{ remote.lastError ? $t("hub.list.loadFailed") : $t("hub.list.empty") }}</div>
        <p id="cqEmptyHint">{{ remote.lastError || $t("hub.list.emptyHint") }}</p>
        <button v-if="remote.lastError" type="button" class="link-btn" @click="remote.load()">{{ $t("common.retry") }}</button>
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
            <button type="button" class="btn small" id="cqViewReload" @click="viewSeed += 1">{{ $t("hub.list.reloadBtn") }}</button>
            <button type="button" class="icon-btn" id="cqViewClose" :aria-label="$t('common.close')" @click="viewOpen = false">
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
              <p><b>{{ $t("hub.list.viewLoading") }}</b></p>
              <p>{{ $t("hub.list.viewHint") }}</p>
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
      :aria-label="$t('hub.list.sortMenuAria')"
      @click.stop
    >
      <div class="menu-caption">{{ $t("hub.list.sortMenuAria") }}</div>
      <button role="menuitem" data-order="custom" :class="{ on: sortMode === 'custom' }" @click="pickSort('custom')">{{ $t("hub.list.sortCustomDesc") }}</button>
      <button role="menuitem" data-order="name" :class="{ on: sortMode === 'name' }" @click="pickSort('name')">{{ $t("hub.list.sortNameAZ") }}</button>
      <button role="menuitem" data-order="recent" :class="{ on: sortMode === 'recent' }" @click="pickSort('recent')">{{ $t("hub.list.sortRecent") }}</button>
      <div class="menu-divider" role="separator" />
      <div class="menu-caption" id="cqSortHint">{{ $t("hub.list.sortHint") }}</div>
    </div>

    <!-- 行内 ⋮ 菜单（旧版 cqRowMenu，hub.html:8158） -->
    <div v-if="rowMenuId" class="overlay-transparent" @click="rowMenuId = null">
      <div class="pc-menu cq-row-menu" role="menu" @click.stop>
        <div class="menu-caption">{{ renmoteRow(rowMenuId)?.name }}</div>
        <button type="button" role="menuitem" @click="rowMenuAction('view', rowMenuId!)">{{ $t("rowMenu.viewInline") }}</button>
        <button type="button" role="menuitem" @click="rowMenuAction('goto', rowMenuId!)">{{ $t("rowMenu.gotoTab") }}</button>
        <button type="button" role="menuitem" @click="rowMenuAction('browser', rowMenuId!)">{{ $t("rowMenu.openBrowser") }}</button>
        <div class="menu-divider" />
        <button type="button" role="menuitem" @click="rowMenuAction('refresh', rowMenuId!)">{{ $t("rowMenu.reprobe") }}</button>
        <button type="button" role="menuitem" @click="rowMenuAction('sysinfo', rowMenuId!)">{{ $t("rowMenu.sysinfo") }}</button>
        <template v-if="rowMenuId !== 'local'">
          <button type="button" role="menuitem" @click="rowMenuAction('edit', rowMenuId!)">{{ $t("rowMenu.edit") }}</button>
          <button type="button" role="menuitem" @click="rowMenuAction('copyip', rowMenuId!)">{{ $t("rowMenu.copyAddr") }}</button>
          <div class="menu-divider" />
          <button type="button" role="menuitem" class="danger" @click="rowMenuAction('delete', rowMenuId!)">{{ $t("rowMenu.delete") }}</button>
        </template>
      </div>
    </div>

    <!-- 浏览器打开：多连接让用户选择走哪个地址 -->
    <div v-if="browserPickOpen" class="overlay" @click.self="browserPickOpen = false">
      <div class="dialog" role="dialog" aria-modal="true" style="max-width: 460px">
        <div class="dialog-head">
          <h3>{{ $t("cl.pickConn") }}</h3>
          <button type="button" class="icon-btn" :aria-label="$t('common.close')" @click="browserPickOpen = false">✕</button>
        </div>
        <div class="dialog-body" style="display:flex;flex-direction:column;gap:8px">
          <button
            v-for="(c, i) in browserPickList"
            :key="i"
            type="button"
            class="btn"
            style="justify-content:flex-start;text-align:left;gap:8px"
            @click="pickBrowserAddr(c)"
          >
            <span class="cq-remote-kind" :class="{ 'ok': c.ok === true, 'bad': c.ok === false }">{{ c.label }}</span>
            <span class="mono" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{{ c.address }}</span>
            <span v-if="c.ok === true" style="color:rgb(var(--ok));margin-left:auto;flex:none">{{ $t("hub.conn.okShort") }}</span>
            <span v-else-if="c.ok === false" style="color:rgb(var(--bad));margin-left:auto;flex:none">{{ $t("hub.conn.fail") }}</span>
          </button>
        </div>
        <div class="dialog-foot">
          <button type="button" class="btn" @click="browserPickOpen = false">{{ $t("common.cancel") }}</button>
        </div>
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
