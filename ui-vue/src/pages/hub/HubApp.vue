<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useUiStateStore } from "@/stores/uiState";
import { useRemoteStore } from "@/stores/remote";
import ComputerList from "./ComputerList.vue";
import ServersPage from "./ServersPage.vue";
import AboutPage from "./AboutPage.vue";

/**
 * hub 主界面（原 `ui/hub.html`）。
 *
 * ## 页面结构（对齐旧版的 12 个 `.page` 区块）
 *
 * ```
 * .shell
 *   nav.side-nav       ← 侧栏导航（电脑 / 电脑列表 / 服务器 / 远程接入 /
 *                        终端 / 会话 / 传输 / 文件 / 密钥 / 备份 / 终端设置 / 关于）
 *   main.pages         ← 当前页内容
 *   footer.status      ← 状态栏
 * ```
 *
 * ## 迁移策略（阶段 3）
 *
 * hub 有 8,236 行核心逻辑 + 4,600 行 HTML，覆盖 109 个 IPC 命令、10 个功能域。
 * 按功能域拆组件，**先骨架后填充**：本文件负责导航与页面切换，
 * 各页内容逐步迁入 `pages/hub/*.vue`。
 */

const ui = useUiStateStore();
const remote = useRemoteStore();

/** 导航项（顺序与旧版一致） */
const NAV = [
  { id: "home", label: "电脑" },
  { id: "computers", label: "电脑列表" },
  { id: "servers", label: "服务器" },
  { id: "gateway", label: "远程接入" },
  { id: "term", label: "终端" },
  { id: "sessions", label: "会话" },
  { id: "transfers", label: "传输" },
  { id: "files", label: "文件" },
  { id: "keys", label: "密钥" },
  { id: "backup", label: "备份与恢复" },
  { id: "termset", label: "终端设置" },
  { id: "about", label: "关于" },
] as const;

type PageId = (typeof NAV)[number]["id"];

const PAGE_KEY = "anyboot.hub.page";

const currentPage = ref<PageId>(
  // 优先级：?page= 直达 > 上次访问的页 > 默认「电脑列表」
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

/** 记住上次访问的页（下次打开 hub 直接回去） */
watch(currentPage, (page) => {
  try {
    localStorage.setItem(PAGE_KEY, page);
  } catch {
    /* 存不了就算了 */
  }
});

/** 侧栏收起（窄视口自动收，用户也可手动） */
const navCollapsed = ref(false);

onMounted(async () => {
  await ui.start();
  await remote.load();
  void remote.refreshAllBoot();

  // 验收信息写进 trace（macOS 无法 attach devtools）
  window.setTimeout(() => {
    const line =
      `[vue-hub] ready=${ui.ready ? 1 : 0} page=${currentPage.value} ` +
      `servers=${remote.servers.length} bootInfo=${remote.bootInfo.size} ` +
      `online=${remote.totalOnline} transport=${ui.activeTransport}`;
    void import("@/api/tauri")
      .then(({ isTauri, call }) =>
        isTauri()
          ? call<void>("hub_report_error", { source: "vue-hub", message: line, stack: "" })
          : undefined,
      )
      .catch(() => {});
  }, 3000);
});

const currentLabel = computed(
  () => NAV.find((item) => item.id === currentPage.value)?.label ?? "",
);

function goto(id: PageId) {
  currentPage.value = id;
}
</script>

<template>
  <div class="shell" :class="{ 'nav-collapsed': navCollapsed }">
    <nav class="side-nav">
      <button
        v-for="item in NAV"
        :key="item.id"
        type="button"
        :class="['snav', { 'is-on': currentPage === item.id }]"
        :title="item.label"
        @click="goto(item.id)"
      >
        <span class="s-label">{{ item.label }}</span>
      </button>
    </nav>

    <main class="pages">
      <header v-if="currentPage !== 'computers' && currentPage !== 'servers'" class="page-head">
        <h1>{{ currentLabel }}</h1>
        <button
          type="button"
          class="btn icon ghost sm"
          :title="navCollapsed ? '展开侧栏' : '收起侧栏'"
          @click="navCollapsed = !navCollapsed"
        >
          {{ navCollapsed ? "»" : "«" }}
        </button>
      </header>

      <section class="page-body" :class="{ 'is-flush': currentPage === 'computers' || currentPage === 'servers' }">
        <ComputerList v-if="currentPage === 'computers'" />
        <ServersPage v-else-if="currentPage === 'servers'" />
        <AboutPage v-else-if="currentPage === 'about'" />
        <template v-else>
          <p class="placeholder">
            「{{ currentLabel }}」页面正在迁移中 —— 阶段 3 按功能域逐块迁入。
          </p>
          <dl class="summary">
            <dt>电脑档案</dt>
            <dd>{{ remote.servers.length }} 台</dd>
            <dt>已读取引导项</dt>
            <dd>{{ remote.bootInfo.size }} 台</dd>
            <dt>Revision</dt>
            <dd>{{ ui.revision }}</dd>
          </dl>
        </template>
      </section>
    </main>
  </div>
</template>

<style scoped>
.shell {
  display: flex;
  height: 100%;
  background: var(--bg, #fbfcfe);
}
.side-nav {
  width: 168px;
  flex: none;
  border-right: 1px solid var(--line, #e5e7eb);
  padding: 10px 8px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  overflow-y: auto;
  background: var(--panel, #fff);
}
.shell.nav-collapsed .side-nav {
  width: 52px;
}
.shell.nav-collapsed .s-label {
  display: none;
}
.snav {
  text-align: left;
  padding: 8px 10px;
  border-radius: 7px;
  border: 0;
  background: transparent;
  font-size: 13px;
  color: var(--fg-dim, #4b5563);
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.snav:hover {
  background: var(--soft, #f3f4f6);
}
.snav.is-on {
  background: var(--accent-soft, #eff6ff);
  color: var(--accent, #2563eb);
  font-weight: 600;
}
.pages {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.page-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 20px 10px;
  border-bottom: 1px solid var(--line, #e5e7eb);
}
.page-head h1 {
  font-size: 16px;
  margin: 0;
}
.page-body {
  flex: 1;
  overflow-y: auto;
  padding: 20px;
}
.page-body.is-flush {
  padding: 0;
}
.placeholder {
  font-size: 13px;
  color: var(--fg-faint, #9ca3af);
  margin: 0 0 16px;
}
.summary {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 6px 16px;
  font-size: 13px;
  max-width: 360px;
}
.summary dt {
  color: var(--fg-faint, #9ca3af);
}
.summary dd {
  margin: 0;
}
</style>
