<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, isTauri } from "@/api/tauri";
import UpdateDialog from "@/components/update/UpdateDialog.vue";
const { t } = useI18n();

/**
 * 关于页（原 `ui/hub.html` 的 `#page-about`，4452 行 / 477 行 DOM / 209 id）。
 *
 * ## 1:1 结构（4 张 .about-card）
 * ```
 * .about-wrap
 *   .about-card.about-hero   .about-logo + .about-title + .about-ver + .about-desc
 *                            + .about-hero-actions > .about-pill
 *   .about-card.about-update #update-section（.update-head / -meta / -new / -opts）
 *   .about-card.about-repo   .about-repo-head + .about-stats + .about-tip
 *   .about-card.about-links  .about-link × 3（+ .about-divider）
 *   .about-sign
 * ```
 */

interface UpdateState {
  phase: string;
  currentVersion: string;
  latestVersion: string;
  notes: string;
  publishedAt: string;
  downloaded: number;
  total: number;
  error: string;
  source: string;
  /** 已下载字节人类可读文案（旧版 #update-bytes） */
  bytes?: string;
  /** 自动检查开关 */
  autoCheck?: boolean;
  /** 下载源 */
  mirror?: string;
  /** 自动检查间隔（小时） */
  intervalHours?: number;
  /** 用户点过「跳过这个版本」 */
  skipped?: boolean;
}

const inTauri = isTauri();
const version = ref("—");
const update = ref<UpdateState | null>(null);
const checking = ref(false);
const updateError = ref("");
const updateOpen = ref(false);
const stars = ref("…");
const forks = ref("—");
const issues = ref("—");
/** 旧版 about 页 Star/Fork/Issues：GitHub 匿名 API + localStorage 缓存，失败留「…」 */
async function loadGitHubStats(): Promise<void> {
  const KEY = "anyboot.github.stats";
  type Stats = { stargazers_count?: number; forks_count?: number; open_issues_count?: number };
  const paint = (data: Stats | null) => {
    const n = (k: keyof Stats) => (data && typeof data[k] === "number" ? String(data[k]) : "…");
    stars.value = n("stargazers_count") === "…" ? "…" : n("stargazers_count");
    forks.value = n("forks_count") === "…" ? "—" : n("forks_count") + " Fork";
    issues.value = n("open_issues_count") === "…" ? "—" : n("open_issues_count") + " Issues";
  };
  try {
    paint(JSON.parse(localStorage.getItem(KEY) || "null"));
  } catch {
    paint(null);
  }
  if (typeof fetch !== "function") return;
  try {
    const resp = await fetch("https://api.github.com/repos/lidiaoo/anyboot");
    if (!resp.ok) return;
    const json = (await resp.json()) as Stats;
    const data = {
      stargazers_count: json.stargazers_count,
      forks_count: json.forks_count,
      open_issues_count: json.open_issues_count,
    };
    localStorage.setItem(KEY, JSON.stringify(data));
    paint(data);
  } catch {
    /* 保留缓存值 */
  }
}

const LINK_ITEMS = [
  { id: "releases", label: "about.tabReleases", icon: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>' },
  { id: "issues", label: "about.tabIssues", icon: '<path d="M9 8l-4 4 4 4M15 8l4 4-4 4"/>' },
  { id: "license", label: "about.tabLicense", icon: '<path d="M6 2.5h7.5L20 9v11.5H6z"/><path d="M13.5 2.5V9H20"/><path d="M9 14h6"/>', suffix: "MIT" },
];

/** 自动检查开关（旧版 #update-auto-check） */
const autoCheck = ref(true);
/** 下载源：auto / direct / ghfast / ghproxy / custom（自定义 https 前缀） */
const KNOWN_MIRRORS = ["auto", "direct", "ghfast", "ghproxy"] as const;
const mirror = ref<string>("auto");
/** 自定义下载源前缀（https://…），与 PC 下拉框的「自定义…」配套 */
const customUrl = ref("");
/** 自动检查间隔（小时，0=关闭） */
const intervalHours = ref(12);
/** 初始化期间不把「读回来的值」再写回后端（否则一进页面就覆盖用户设置） */
const optionsReady = ref(false);
/** 「更新提示窗」：自动检查命中新版本时弹一次（手动检查不弹，就地展开就行） */
const promptOpen = ref(false);
const manualCheck = ref(false);

// 自动检查命中新版本 → 弹提示窗（用户点名要的「更新提示窗」）
watch(
  () => update.value?.phase,
  (phase) => {
    if (phase === "available" && !update.value?.skipped && !manualCheck.value) {
      promptOpen.value = true;
    }
  },
);

/** 开关 / 下载源 / 频率改动即落盘（以前只在本地 ref 里改，重开就复原 = 功能等于没有） */
watch(autoCheck, (value) => {
  if (!optionsReady.value) return;
  void call("set_update_auto_check", { enabled: value }).catch((e) => (updateError.value = String(e)));
});
watch(mirror, (value) => {
  if (!optionsReady.value) return;
  if (value === "custom") return; // 等用户填完自定义地址再落盘
  void call("set_update_mirror", { value }).catch((e) => (updateError.value = String(e)));
});

watch(intervalHours, (value) => {
  if (!optionsReady.value) return;
  void call("set_update_interval", { hours: value }).catch((e) => (updateError.value = String(e)));
});

/** 下载（旧版 #update-download） */
async function startDownload() {
  try {
    await call("update_download");
    update.value = await call<UpdateState>("update_state");
  } catch (error) {
    updateError.value = String(error);
  }
}

/** 跳过这个版本（旧版 #update-skip → update_dismiss） */
async function skipVersion() {
  const v = update.value?.latestVersion;
  if (!v) return;
  try {
    await call("update_dismiss", { version: v });
    update.value = await call<UpdateState>("update_state");
  } catch (error) {
    updateError.value = String(error);
  }
}

onMounted(async () => {
  void loadGitHubStats();
  // 更新状态热更新（旧版 hub 监听 update-state → render(payload)）
  try {
    const { listen } = await import("@tauri-apps/api/event");
    void listen<UpdateState>("update-state", (event) => {
      if (event.payload) update.value = event.payload;
    });
  } catch { /* 非 Tauri 环境 */ }

  if (!inTauri) return;
  try {
    const { getVersion } = await import("@tauri-apps/api/app");
    version.value = await getVersion();
  } catch {
    version.value = "";
  }
  try {
    update.value = await call<UpdateState>("update_state");
    autoCheck.value = update.value?.autoCheck ?? true;
    const loadedMirror = String(update.value?.mirror ?? "auto");
    if ((KNOWN_MIRRORS as readonly string[]).includes(loadedMirror)) {
      mirror.value = loadedMirror;
      customUrl.value = "";
    } else {
      mirror.value = "custom";
      customUrl.value = loadedMirror;
    }
    intervalHours.value = update.value?.intervalHours ?? 12;
    optionsReady.value = true;
    // 兜底：getVersion 拿不到时用快照里的版本（浏览器 / mock 环境）
    if (!version.value && update.value?.currentVersion) {
      version.value = update.value.currentVersion;
    }
  } catch (error) {
    updateError.value = String(error);
  }
  if (!version.value) version.value = "—";
});

async function checkUpdate() {
  if (checking.value) return;
  checking.value = true;
  manualCheck.value = true;
  updateError.value = "";
  updateOpen.value = true;
  // 立即进入「正在检查」态：meta 显示“正在检查新版本…”，按钮转圈/禁用——否则点了没反馈(用户:没滚轮)
  update.value = { ...(update.value ?? {}), phase: "checking" } as UpdateState;
  try {
    update.value = await call<UpdateState>("update_check");
  } catch (error) {
    updateError.value = String(error);
    update.value = { ...(update.value ?? {}), phase: "error" } as UpdateState;
  } finally {
    checking.value = false;
    manualCheck.value = false;
  }
}

function openPage(which: string) {
  const urls: Record<string, string> = {
    github: "https://github.com/lidiaoo/anyboot",
    releases: "https://github.com/lidiaoo/anyboot/releases",
    issues: "https://github.com/lidiaoo/anyboot/issues",
    license: "https://github.com/lidiaoo/anyboot/blob/main/LICENSE",
  };
  // 旧版 hub_about_open(page)：page ∈ repo/releases/issues/license，后端映射到 URL
  if (!(which in urls)) return;
  void call("hub_about_open", { page: which }).catch(() => {});
}
</script>

<template>
  <div class="hub-page">
    <div class="page-head"><h2>{{ t("about.title") }}</h2></div>
    <div class="hub-page-body">
      <div class="about-wrap">
        <!-- 版本卡 -->
        <div class="about-card about-hero">
          <span class="about-logo" aria-hidden="true">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
              <rect x="2.5" y="4" width="19" height="13.5" rx="2" />
              <path d="M8 21h8M12 17.5V21" />
            </svg>
          </span>
          <h2 class="about-title">AnyBoot</h2>
          <p class="about-ver mono" id="aboutVersion">{{ t("about.verLine", { ver: version }) }}</p>
          <p class="about-desc">{{ t("about.desc") }}</p>
          <div class="about-hero-actions">
            <button type="button" id="about-check-update" class="about-pill" :disabled="checking" @click="checkUpdate">
              <span v-if="checking" class="spin" aria-hidden="true" />
              {{ checking ? t("about.checkingNow") : t("about.checkUpdate") }}<span v-if="!checking" aria-hidden="true"> ›</span>
            </button>
          </div>
        </div>

        <!-- 更新卡：与「发现新版本」弹窗共用同一个 Vue 组件（UpdateDialog） -->
        <UpdateDialog v-if="inTauri && updateOpen" inline @close="updateOpen = false" />

        <!-- 仓库卡 -->
        <div class="about-card about-repo">
          <button type="button" class="about-repo-head" id="aboutGitHub" @click="openPage('github')">
            <svg class="about-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M9 8l-4 4 4 4M15 8l4 4-4 4" />
            </svg>
            <span class="about-repo-text">
              <b>GitHub · lidiaoo/anyboot</b>
              <i>{{ t("about.repoHome") }}</i>
            </span>
            <span class="about-star">
              <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                <path d="M12 3.6l2.6 5.3 5.9.9-4.3 4.1 1 5.8-5.2-2.7-5.2 2.7 1-5.8-4.3-4.1 5.9-.9z" />
              </svg>
              <b id="about-star-inline">{{ stars }}</b>
            </span>
            <span class="about-chev" aria-hidden="true">›</span>
          </button>
          <div class="about-stats">
            <span id="about-stat-star">{{ stars }} Star</span>
            <span id="about-stat-fork">{{ forks }}</span>
            <span id="about-stat-issue">{{ issues }}</span>
          </div>
          <p class="about-tip">{{ t("about.starTip") }}</p>
        </div>

        <!-- 链接卡 -->
        <div class="about-card about-links">
          <template v-for="(item, i) in LINK_ITEMS" :key="item.id">
            <div v-if="i > 0" class="about-divider" aria-hidden="true" />
            <button type="button" class="about-link" @click="openPage(item.id)">
              <svg class="about-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" v-html="item.icon" />
              <span>{{ t(item.label) }}</span>
              <span v-if="item.suffix" class="about-suffix mono">{{ item.suffix }}</span>
              <span class="about-chev" aria-hidden="true">›</span>
            </button>
          </template>
        </div>

        <p class="about-sign mono">Made with AnyBoot · 2026</p>
      </div>
    </div>

    <!-- 更新提示窗（自动检查命中新版本时弹一次；不打断，可稍后/跳过） -->
    <div
      v-if="promptOpen"
      style="position:fixed;inset:0;z-index:80;display:flex;align-items:center;justify-content:center;background:rgba(0,0,0,.45)"
      @click.self="promptOpen = false"
    >
      <div role="dialog" aria-modal="true" style="width:min(420px,92vw);background:var(--panel,#fff);border:1px solid var(--line,#e4e4e7);border-radius:14px;padding:18px 20px;box-shadow:0 18px 48px rgba(0,0,0,.28)">
        <h3 style="margin:0 0 6px;font-size:16px">{{ t("about.foundNew", { v: update?.latestVersion || "" }) }}</h3>
        <pre v-if="update?.notes" style="margin:0 0 12px;max-height:160px;overflow:auto;white-space:pre-wrap;font-size:12px;color:var(--dim,#52525b)">{{ update.notes }}</pre>
        <p v-else style="margin:0 0 12px;font-size:12px;color:var(--dim,#52525b)">{{ t("about.notChecked") }}</p>
        <div style="display:flex;gap:8px;justify-content:flex-end;flex-wrap:wrap">
          <button type="button" class="btn" @click="promptOpen = false">{{ t("common.cancel") }}</button>
          <button type="button" class="btn" @click="skipVersion(); promptOpen = false">{{ t("about.skip") }}</button>
          <button type="button" class="btn primary" @click="updateOpen = true; startDownload(); promptOpen = false">{{ t("about.download") }}</button>
        </div>
      </div>
    </div>
  </div>
</template>
