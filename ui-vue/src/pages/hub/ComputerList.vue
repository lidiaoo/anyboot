<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRemoteStore, type RemoteProfile } from "@/stores/remote";
import { useSettingsStore } from "@/stores/settings";
import ComputerRow from "@/components/hub/ComputerRow.vue";
import PcMenu from "@/components/hub/PcMenu.vue";
import SystemInfoDialog from "@/components/hub/SystemInfoDialog.vue";

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
const sortMode = ref<"custom" | "name" | "recent">("custom");
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
  try {
    await remote.refreshAllBoot();
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

onMounted(() => {
  startTimer();
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
onUnmounted(stopTimer);
// 设置里改了间隔要重建定时器（否则旧间隔跑到刷新页面为止）
watch(() => settings.dataRefreshSec, startTimer);

/** 当前选中的电脑（旧版 activeRemote，默认 local） */
const activeId = ref("local");

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

function openMore(id: string) {
  activeId.value = id;
}

function switchBoot(payload: Record<string, unknown>) {
  void remote.switchBoot(payload);
}

function power(payload: Record<string, unknown>) {
  void remote.power(payload);
}
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
        <div class="cq-stat cq-stat--on"><i>在线</i><b>{{ stats.online }}</b></div>
        <div class="cq-stat cq-stat--off"><i>离线</i><b>{{ stats.offline }}</b></div>
        <div class="cq-stat cq-stat--pend"><i>待生效</i><b>{{ stats.pending }}</b></div>
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
          v-model="query"
          type="search"
          placeholder="搜索电脑名称、IP 或 MAC"
          aria-label="搜索电脑"
          spellcheck="false"
          autocomplete="off"
        />
      </div>
      <div class="cq-seg" role="tablist" aria-label="状态筛选">
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
          {{ f.label }} <span class="n">{{ f.n }}</span>
        </button>
      </div>
      <span class="cq-spacer" />
      <button type="button" class="link-btn cq-help-btn" :aria-expanded="helpOpen" @click="helpOpen = !helpOpen">
        操作说明
      </button>
      <button
        type="button"
        class="link-btn"
        title="选择电脑列表的排序方式（电脑页标签栏跟着同一套顺序）"
        @click="sortMode = sortMode === 'custom' ? 'name' : sortMode === 'name' ? 'recent' : 'custom'"
      >
        排序 <span>{{ sortMode === "custom" ? "自定义" : sortMode === "name" ? "按名称" : "最近连接" }}</span>
      </button>
      <button type="button" class="link-btn" :disabled="refreshing" @click="manualRefresh">
        {{ refreshing ? "探测中" : "刷新" }}
      </button>
      <PcMenu />
    </div>

    <div v-show="helpOpen" class="cq-help">
      <h4>操作说明</h4>
      <ul>
        <li>电源操作<b>点了立即执行，不再弹确认框</b>。</li>
        <li>唤醒：发魔术包让电脑开机 —— 需要配好 MAC 并在主板开启网络唤醒。</li>
        <li>睡眠 / 关机：网页服务立刻停掉，会显示离线；要叫醒请点「唤醒」。</li>
        <li>重启：重启完成后会自动恢复在线。</li>
        <li>引导项变更<b>下次重启时生效</b>，可点「撤销」恢复原值。</li>
        <li>四个电源键任何时候都能按、不会置灰；设备离线时引导项不可改。</li>
        <li>显示「未检测到」= 该机没部署对应引导管理器。</li>
      </ul>
    </div>

    <div class="cq-card">
      <div class="cq-grid" role="table" aria-label="电脑列表">
        <div class="cq-rows">
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
          />
          </div>
      </div>
      <div v-if="!rows.length" class="cq-empty">
        <div>没有匹配的电脑</div>
        <p>换个关键词，或把筛选切回「全部」</p>
      </div>
    </div>

    <!-- 电脑页面弹层（旧版 #cqViewOverlay）：宽高用 min() 撑满，iframe 撑余下空间 -->
    <div v-if="viewOpen" class="overlay" @click.self="viewOpen = false">
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
            <button type="button" class="btn small" @click="viewSeed += 1">重载</button>
            <button type="button" class="icon-btn" aria-label="关闭" @click="viewOpen = false">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
                <path d="M6 6l12 12M18 6L6 18" />
              </svg>
            </button>
          </div>
        </div>
        <div class="pc-frame-host" style="flex: 1; min-height: 0; border: 0; border-radius: 0">
          <iframe
            v-if="viewUrl"
            :key="viewSeed"
            :src="viewUrl"
            :title="viewTitle"
            style="width: 100%; height: 100%; border: 0; background: #fff"
          />
          <div v-else class="pc-empty">
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
