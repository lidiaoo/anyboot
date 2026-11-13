<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { call, isTauri } from "@/api/tauri";

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
  { id: "releases", label: "更新日志", icon: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>' },
  { id: "issues", label: "问题反馈（Issues）", icon: '<path d="M9 8l-4 4 4 4M15 8l4 4-4 4"/>' },
  { id: "license", label: "开源许可", icon: '<path d="M6 2.5h7.5L20 9v11.5H6z"/><path d="M13.5 2.5V9H20"/><path d="M9 14h6"/>', suffix: "MIT" },
];

const phaseText = computed(() => {
  switch (update.value?.phase) {
    case "checking":
      return "正在检查新版本…";
    case "up_to_date":
      return "已是最新版本";
    case "available":
      return `发现新版本 ${update.value.latestVersion}`;
    case "downloading":
      return `正在下载 v${update.value?.latestVersion ?? ""}…`;
    case "verifying":
      return `正在校验 v${update.value?.latestVersion ?? ""}…`;
    case "ready_to_install":
      return "可以安装";
    case "installing":
      return `正在安装 v${update.value?.latestVersion ?? ""}，应用即将退出并重启…`;
    case "error":
      return `出错：${update.value.error}`;
    default:
      return "还没检查过";
  }
});

/** 是否有新版本可下载（旧版 #update-new 的显隐条件） */
const hasNew = computed(() =>
  update.value?.phase === "available" ||
  update.value?.phase === "downloading" ||
  update.value?.phase === "verifying" ||
  update.value?.phase === "ready_to_install",
);
/** 是否正在下载 */
const downloading = computed(() =>
  update.value?.phase === "downloading" || update.value?.phase === "verifying",
);

/** 自动检查开关（旧版 #update-auto-check） */
const autoCheck = ref(true);
/** 下载源（旧版 #update-mirror） */
const mirror = ref<"auto" | "direct" | "ghfast" | "ghproxy">("auto");

/** 下载（旧版 #update-download） */
async function startDownload() {
  try {
    await call("update_download");
    update.value = await call<UpdateState>("update_state");
  } catch (error) {
    updateError.value = String(error);
  }
}

/** 安装并重启（旧版 #update-install） */
async function installUpdate() {
  try {
    await call("update_install");
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

const progress = computed(() => {
  const total = update.value?.total ?? 0;
  if (!total) return 0;
  return Math.min(100, Math.round(((update.value?.downloaded ?? 0) / total) * 100));
});

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
    mirror.value = (update.value?.mirror as typeof mirror.value) ?? "auto";
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
    <div class="page-head"><h2>关于</h2></div>
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
          <p class="about-ver mono" id="aboutVersion">v{{ version }} · 多系统启动管理器</p>
          <p class="about-desc">多系统一键切换 · GRUB 管理 · 远程电源控制 · SSH 跳板机隧道 · SFTP 文件传输</p>
          <div class="about-hero-actions">
            <button type="button" id="about-check-update" class="about-pill" :disabled="checking" @click="checkUpdate">
              <span v-if="checking" class="spin" aria-hidden="true" />
              {{ checking ? "检查中…" : "检查更新" }}<span v-if="!checking" aria-hidden="true"> ›</span>
            </button>
          </div>
        </div>

        <!-- 更新卡（只在 Tauri 里出现） -->
        <!-- ── 更新详情（旧版 #update-section，hub.html:4469）── -->
        <div v-if="inTauri && updateOpen" id="update-section" class="about-card about-update">
          <div class="update-head" id="update-head">
            <div class="update-version">
              <span class="update-k">当前版本</span>
              <b class="update-v mono" id="update-current">{{ update?.currentVersion || version }}</b>
            </div>
            <div class="update-actions" id="update-actions">
              <button type="button" id="update-check" class="btn primary" :disabled="checking" @click="checkUpdate">
                <span v-if="checking" class="spin" aria-hidden="true" />
                {{ checking ? "检查中…" : "检查更新" }}
              </button>
              <button v-if="updateError" type="button" id="update-retry" class="btn" @click="checkUpdate">重试</button>
            </div>
          </div>
          <div class="update-meta" id="update-meta">{{ phaseText }}</div>

          <!-- 发现新版本（旧版 #update-new） -->
          <div v-if="hasNew" id="update-new" class="update-new">
            <div class="update-new-head">
              <strong id="update-new-title">发现新版本 {{ update?.latestVersion }}</strong>
              <span class="update-k" id="update-new-date">{{ update?.publishedAt }}</span>
            </div>
            <pre v-if="update?.notes" id="update-notes" class="update-notes">{{ update.notes }}</pre>
            <div v-if="update?.total" id="update-progress" class="update-progress">
              <div class="update-track"><div id="update-fill" class="update-fill" :style="{ width: progress + '%' }" /></div>
              <div class="update-progress-meta">
                <span id="update-pct" class="update-pct">{{ progress }}%</span>
                <span id="update-bytes" class="mono">{{ update?.bytes || "" }}</span>
                <span id="update-source">{{ update?.source || "" }}</span>
              </div>
            </div>
            <div class="update-buttons">
              <button
                v-if="!downloading"
                type="button"
                id="update-download"
                class="btn primary"
                @click="startDownload"
              >
                下载
              </button>
              <button
                v-if="update?.downloaded"
                type="button"
                id="update-install"
                class="btn primary"
                @click="installUpdate"
              >
                安装并重启
              </button>
              <button type="button" id="update-skip" class="btn" @click="skipVersion">跳过这个版本</button>
            </div>
          </div>

          <div v-if="updateError" id="update-error" class="update-error">{{ updateError }}</div>

          <div class="update-opts" id="update-opts">
            <label class="update-opt">
              <input id="update-auto-check" v-model="autoCheck" type="checkbox" />
              <span>自动检查（启动 30 秒后 + 每 12 小时）</span>
            </label>
            <label class="update-opt">
              <span class="update-k">下载源</span>
              <select id="update-mirror" v-model="mirror">
                <option value="auto">自动（直连 → 镜像）</option>
                <option value="direct">只直连 GitHub</option>
                <option value="ghfast">镜像 ghfast.top</option>
                <option value="ghproxy">镜像 gh-proxy.com</option>
              </select>
            </label>
          </div>
        </div>

        <!-- 仓库卡 -->
        <div class="about-card about-repo">
          <button type="button" class="about-repo-head" id="aboutGitHub" @click="openPage('github')">
            <svg class="about-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <path d="M9 8l-4 4 4 4M15 8l4 4-4 4" />
            </svg>
            <span class="about-repo-text">
              <b>GitHub · lidiaoo/anyboot</b>
              <i>开源项目主页</i>
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
          <p class="about-tip">点 Star 会打开 GitHub 网页，请在网页上点一下 —— 这是给开源作者最好的鼓励。</p>
        </div>

        <!-- 链接卡 -->
        <div class="about-card about-links">
          <template v-for="(item, i) in LINK_ITEMS" :key="item.id">
            <div v-if="i > 0" class="about-divider" aria-hidden="true" />
            <button type="button" class="about-link" @click="openPage(item.id)">
              <svg class="about-ico" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" v-html="item.icon" />
              <span>{{ item.label }}</span>
              <span v-if="item.suffix" class="about-suffix mono">{{ item.suffix }}</span>
              <span class="about-chev" aria-hidden="true">›</span>
            </button>
          </template>
        </div>

        <p class="about-sign mono">Made with AnyBoot · 2026</p>
      </div>
    </div>
  </div>
</template>
