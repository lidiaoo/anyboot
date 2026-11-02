<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useToastStore } from "@/stores/toast";
import { useRemoteStore } from "@/stores/remote";
import { useServersStore } from "@/stores/servers";
import ComputerList from "./ComputerList.vue";
import ServersPage from "./ServersPage.vue";
import AboutPage from "./AboutPage.vue";
import HomePage from "./HomePage.vue";
import GatewayPage from "./GatewayPage.vue";
import BackupPage from "./BackupPage.vue";
import TermPage from "./TermPage.vue";
import SessionsPage from "./SessionsPage.vue";
import TransfersPage from "./TransfersPage.vue";
import FilesPage from "./FilesPage.vue";
import KeysPage from "./KeysPage.vue";
import TermsetPage from "./TermsetPage.vue";

/**
 * hub 主界面（原 `ui/hub.html`，14921 行）。
 *
 * ## 1:1 还原原则（用户明确要求）
 *
 * DOM 结构、类名**必须与旧版逐字一致**。原因：
 *   · `styles.css` 5013 行
 *   · `hub.html` 的 3 个内联 style 块 2342 行
 * 全是按这些类名写的。自己重新设计布局 = 7000+ 行样式全部失效。
 *
 * 旧版外壳（`hub.html:2197`）：
 *   .shell
 *     aside.side
 *       .side-head   .side-logo + .side-name + .side-toggle
 *       nav.side-nav 12 × button.snav(> svg + span.s-label)
 *       button.snav  主题切换
 *       .side-foot
 *     .side-split
 *     .content
 *       section.page
 */

const toast = useToastStore();
const ui = useUiStateStore();
const remote = useRemoteStore();
const servers = useServersStore();

const NAV = [
  { id: "home", label: "电脑", svg: '<rect x="2" y="4" width="20" height="14" rx="2"/><path d="M8 21h8M12 18v3"/>' },
  { id: "computers", label: "电脑列表", svg: '<path d="M4 6h16M4 12h16M4 18h16"/><circle cx="8" cy="6" r="1.4" fill="currentColor" stroke="none"/><circle cx="8" cy="12" r="1.4" fill="currentColor" stroke="none"/><circle cx="8" cy="18" r="1.4" fill="currentColor" stroke="none"/>' },
  { id: "servers", label: "服务器", svg: '<rect x="2" y="3" width="20" height="7" rx="2"/><rect x="2" y="14" width="20" height="7" rx="2"/><path d="M6 6.5h.01M6 17.5h.01"/>' },
  { id: "gateway", label: "远程接入", svg: '<path d="M5 12a7 7 0 0 1 14 0"/><path d="M3.5 15.5a3.5 3.5 0 0 1 3.4-4.4 5 5 0 0 1 9.6-1.2A4 4 0 0 1 19 18H7a3.5 3.5 0 0 1-3.5-2.5z"/><path d="M12 18v3"/>' },
  { id: "term", label: "终端", svg: '<rect x="2" y="4" width="20" height="14" rx="2"/><path d="M6 9l3 3-3 3"/>' },
  { id: "sessions", label: "会话", svg: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>' },
  { id: "transfers", label: "传输", svg: '<path d="M7 4v12M4 13l3 3 3-3M17 20V8M14 11l3-3 3 3"/>' },
  { id: "files", label: "文件", svg: '<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>' },
  { id: "keys", label: "密钥", svg: '<circle cx="8" cy="15" r="4"/><path d="M10.8 12.2 20 3M17 6l2.5 2.5"/>' },
  { id: "backup", label: "备份", svg: '<path d="M21 8v13H3V8M1 3h22v5H1zM10 12h4"/>' },
  { id: "termset", label: "终端设置", svg: '<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6"/>' },
  { id: "about", label: "关于", svg: '<circle cx="12" cy="12" r="9"/><path d="M12 16v-4M12 8h.01"/>' },
] as const;

type PageId = (typeof NAV)[number]["id"];

const PAGE_KEY = "anyboot.hub.page";

const currentPage = ref<PageId>(
  (() => {
    try {
      const fromUrl = (new URLSearchParams(location.search).get("page") || "") as PageId;
      if (NAV.some((item) => item.id === fromUrl)) return fromUrl;
      const saved = localStorage.getItem(PAGE_KEY) as PageId | null;
      if (saved && NAV.some((item) => item.id === saved)) return saved;
    } catch {
      /* 忽略 */
    }
    return "computers" as PageId;
  })(),
);

watch(currentPage, (page) => {
  try {
    localStorage.setItem(PAGE_KEY, page);
  } catch {
    /* 存不了就算了 */
  }
});

const navCollapsed = ref(false);

/**
 * 亮暗主题。
 *
 * CSS 里**暗色是默认**（`:root { color-scheme: dark }`），
 * 亮色靠 `[data-theme="light"]` 覆盖。所以这里要显式写 light。
 * 旧版默认亮色（不跟随系统），只在用户点过「主题」后才变。
 */
const dark = ref(false);
try {
  const saved = localStorage.getItem("anyboot.theme");
  dark.value = saved === "dark";
} catch {
  /* 吃默认 */
}
applyThemeClass(dark.value);

/**
 * `--ui-alpha`：body 背景的不透明度乘数。
 *
 * `legacy.css:129` 是 `background: rgba(var(--bg-rgb), var(--ui-alpha, 1))`，
 * 而这个变量**CSS 里从未定义** —— 旧版由 `app.js:2062` 按设置里的
 * 「不透明度」百分比在运行时 `setProperty`。
 * 不补这一步，`var(--ui-alpha, 1)` 会落到兜底值，且主题切换时背景不跟随。
 * 这里读同名的 localStorage 镜像（`anyboot.ui-alpha`），没有就按 100%。
 */
function applyUiAlpha() {
  let alpha = 1;
  try {
    const raw = localStorage.getItem("anyboot.ui-alpha");
    if (raw !== null) {
      const value = Number.parseFloat(raw);
      if (Number.isFinite(value) && value >= 0 && value <= 1) alpha = value;
    }
  } catch {
    /* 吃默认 */
  }
  document.documentElement.style.setProperty("--ui-alpha", String(alpha));
}
applyUiAlpha();

/**
 * 主题落点：**两个都要设**。
 *
 * · `legacy.css`（原 styles.css）用 `[data-theme="light"]` 属性
 * · `hub-inline.css`（原 hub.html 内联样式）用 `html.light` / `html.dark` 类
 *
 * 只设一个就会出现「变量切了、背景没切」——这正是主窗黑屏之外我踩的第二个坑。
 */
function applyThemeClass(isDark: boolean) {
  const html = document.documentElement;
  html.setAttribute("data-theme", isDark ? "dark" : "light");
  html.classList.toggle("light", !isDark);
  html.classList.toggle("dark", isDark);
}

function toggleTheme() {
  dark.value = !dark.value;
  applyThemeClass(dark.value);
  try {
    localStorage.setItem("anyboot.theme", dark.value ? "dark" : "light");
  } catch {
    /* 忽略 */
  }
}

/* 侧栏拖拽调宽（旧版 `#sideSplit` 的行为） */
let dragging = false;
const sideWidth = ref(248);
try {
  const saved = Number.parseInt(localStorage.getItem("anyboot.hub.sideWidth") || "", 10);
  if (Number.isFinite(saved) && saved >= 160 && saved <= 420) sideWidth.value = saved;
} catch {
  /* 忽略 */
}

function startDrag(event: PointerEvent) {
  dragging = true;
  (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
}
function onDrag(event: PointerEvent) {
  if (!dragging || navCollapsed.value) return;
  sideWidth.value = Math.min(420, Math.max(160, event.clientX));
  const el = document.querySelector<HTMLElement>(".side");
  if (el) el.style.width = `${sideWidth.value}px`;
}
function endDrag() {
  if (!dragging) return;
  dragging = false;
  try {
    localStorage.setItem("anyboot.hub.sideWidth", String(sideWidth.value));
  } catch {
    /* 忽略 */
  }
}

onMounted(async () => {
  const el = document.querySelector<HTMLElement>(".side");
  if (el && !navCollapsed.value) el.style.width = `${sideWidth.value}px`;
  await ui.start();
  await Promise.all([remote.load(), servers.load()]);
  void remote.refreshAllBoot();
});

/**
 * 「＋ 添加」跨页入口（旧版 `pcAddOrEdit()` 是全局函数，任意页面可调）。
 * 这里切到「电脑列表」页 —— 服务端编辑弹层挂在那里。
 */
const remoteAddSeed = ref(0);
function onHomeMenuAction(act: string) {
  if (act === "add") { openRemoteAdd(); return; }
  // edit/list/reload 都切到电脑列表页处理（那里有完整弹层）
  currentPage.value = "computers";
  remoteAddSeed.value += 1;
}

function openRemoteAdd() {
  currentPage.value = "computers";
  remoteAddSeed.value += 1;
}

const contentLabel = computed(
  () => NAV.find((item) => item.id === currentPage.value)?.label ?? "",
);

/** 底部状态栏时钟（旧版 `#clock`，每 1s 更新 HH:MM:SS） */
const clock = ref("--:--:--");
let clockTimer: number | null = null;
function tickClock() {
  clock.value = new Date().toLocaleTimeString("zh-CN", { hour12: false });
}
let beatTimer: number | null = null;
/** 页面心跳（旧版 hub_page_ping，每 3 秒告诉后端 hub 页还活着） */
function beat() {
  void import("@/api/tauri").then(({ call }) => call("hub_page_ping", { ready: true }).catch(() => {}));
}

onMounted(() => {
  tickClock();
  clockTimer = window.setInterval(tickClock, 1000);
  beat();
  beatTimer = window.setInterval(beat, 3000);
});
onUnmounted(() => {
  if (clockTimer !== null) window.clearInterval(clockTimer);
  if (beatTimer !== null) window.clearInterval(beatTimer);
});

/** 状态栏文案（旧版 `#footText` / `#footCount`） */
const footText = computed(() =>
  servers.servers.length ? "终端 / 文件 / 密钥均在独立窗口" : "未连接任何服务器",
);
const footCount = computed(() => `${servers.servers.length} 台服务器`);
</script>

<template>
  <div class="shell" :class="{ 'nav-collapsed': navCollapsed }">
    <aside class="side" id="hubSide" aria-label="远程管理导航">
      <div class="side-head">
        <span class="side-logo" aria-hidden="true">A</span>
        <span class="side-name">
          <b>AnyBoot</b>
          <i class="mono" id="sideVersion">remote</i>
        </span>
        <button
          id="sideToggle"
          class="icon-btn side-toggle"
          :title="navCollapsed ? '展开侧栏' : '收起侧栏'"
          :aria-label="navCollapsed ? '展开侧栏' : '收起侧栏'"
          @click="navCollapsed = !navCollapsed"
        >
          <svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            :style="{ transform: navCollapsed ? 'rotate(180deg)' : 'none' }"
          >
            <path id="sideChevron" d="M11 17l-5-5 5-5M18 17l-5-5 5-5" />
          </svg>
        </button>
      </div>

      <nav class="side-nav" id="hubSideNav" aria-label="导航">
        <button
          v-for="item in NAV"
          :key="item.id"
          type="button"
          class="snav"
          :class="{ active: currentPage === item.id }"
          :title="item.label"
          :data-page="item.id"
          @click="currentPage = item.id"
        >
          <svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.8"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
            v-html="item.svg"
          />
          <span class="s-label">{{ item.label }}</span>
        </button>
      </nav>

      <button
        type="button"
        id="themeToggle"
        class="snav"
        title="切换亮暗主题"
        aria-label="切换亮暗主题"
        @click="toggleTheme"
      >
        <span id="themeIcon" aria-hidden="true" style="width: 18px; text-align: center; flex: none">{{
          dark ? "☀️" : "🌙"
        }}</span>
        <span class="s-label">主题</span>
      </button>

      <div class="side-foot mono">
        <span class="s-label">GitHub · lidiaoo/anyboot<br />MIT · 心跳 10s · 并发探测</span>
      </div>
    </aside>

    <div
      id="sideSplit"
      class="side-split"
      role="separator"
      aria-orientation="vertical"
      aria-label="拖动调整侧栏宽度"
      title="拖动调整侧栏宽度"
      @pointerdown="startDrag"
      @pointermove="onDrag"
      @pointerup="endDrag"
    />

    <div class="content">
      <!-- id 必须与旧版一致：hub-inline.css 里有 `#page-files` / `#page-gateway`
           这样的作用域选择器（如 `html.light #page-files { --fb-blue: ... }`），
           少了 id 那些变量就不生效（面包屑会变成黑块）。 -->
      <section class="page active" :id="`page-${currentPage}`">
        <ComputerList v-if="currentPage === 'computers'" :add-seed="remoteAddSeed" />
        <ServersPage v-else-if="currentPage === 'servers'" />
        <AboutPage v-else-if="currentPage === 'about'" />
        <HomePage v-else-if="currentPage === 'home'" @add-remote="openRemoteAdd" @menu-action="onHomeMenuAction" />
        <GatewayPage v-else-if="currentPage === 'gateway'" />
        <BackupPage v-else-if="currentPage === 'backup'" />
        <TermPage v-else-if="currentPage === 'term'" />
        <SessionsPage v-else-if="currentPage === 'sessions'" />
        <TransfersPage v-else-if="currentPage === 'transfers'" />
        <FilesPage v-else-if="currentPage === 'files'" />
        <KeysPage v-else-if="currentPage === 'keys'" />
        <TermsetPage v-else-if="currentPage === 'termset'" />
        <p v-else class="placeholder">{{ contentLabel }} 页面</p>
      </section>
    </div>
  </div>

  <!--
    底部状态栏（对齐旧版 hub.html:4593）。

    **必须挂到 body 直下**：旧版布局是
        body { display: flex; flex-direction: column }
        .shell { flex: 1 }
        footer { ... }
    footer 若是 #app 的子元素，它会落在 flex 列里被挤出视口。
  -->
  <Teleport to="body">
    <div v-if="toast.text" id="toast" :class="{ bad: toast.bad }" role="status">{{ toast.text }}</div>
  </Teleport>
  <Teleport to="body">
    <footer>
      <span class="dot" />
      <span id="footText">{{ footText }}</span>
      <span class="sep">·</span>
      <span id="footCount">{{ footCount }}</span>
      <span class="clock" id="clock">{{ clock }}</span>
    </footer>
  </Teleport>
</template>
