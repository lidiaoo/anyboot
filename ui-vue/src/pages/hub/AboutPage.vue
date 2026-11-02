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
}

const inTauri = isTauri();
const version = ref("—");
const update = ref<UpdateState | null>(null);
const checking = ref(false);
const updateError = ref("");
const updateOpen = ref(false);
const stars = ref("…");

const LINK_ITEMS = [
  { id: "releases", label: "更新日志", icon: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>' },
  { id: "issues", label: "问题反馈（Issues）", icon: '<path d="M9 8l-4 4 4 4M15 8l4 4-4 4"/>' },
  { id: "license", label: "开源许可", icon: '<path d="M6 2.5h7.5L20 9v11.5H6z"/><path d="M13.5 2.5V9H20"/><path d="M9 14h6"/>', suffix: "MIT" },
];

const phaseText = computed(() => {
  switch (update.value?.phase) {
    case "checking":
      return "检查中…";
    case "up_to_date":
      return "已是最新版本";
    case "available":
      return `发现新版本 ${update.value.latestVersion}`;
    case "downloading":
      return "下载中…";
    case "verifying":
      return "校验中…";
    case "ready_to_install":
      return "可以安装";
    case "installing":
      return "安装中…";
    case "error":
      return `出错：${update.value.error}`;
    default:
      return "还没检查过";
  }
});

const progress = computed(() => {
  const total = update.value?.total ?? 0;
  if (!total) return 0;
  return Math.min(100, Math.round(((update.value?.downloaded ?? 0) / total) * 100));
});

onMounted(async () => {
  if (!inTauri) return;
  try {
    const { getVersion } = await import("@tauri-apps/api/app");
    version.value = await getVersion();
  } catch {
    version.value = "";
  }
  try {
    update.value = await call<UpdateState>("update_state");
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
  try {
    update.value = await call<UpdateState>("update_check");
  } catch (error) {
    updateError.value = String(error);
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
  const url = urls[which];
  if (!url) return;
  // 旧版用 hub_about_open（打开关于页里的外链），失败再退回通用打开命令
  void call("hub_about_open", { url })
    .catch(() => call("open_web_page", { url }))
    .catch(() => {});
}
</script>

<template>
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
        <button type="button" id="about-check-update" class="about-pill" @click="checkUpdate">
          检查更新<span aria-hidden="true"> ›</span>
        </button>
      </div>
    </div>

    <!-- 更新卡（只在 Tauri 里出现） -->
    <div v-if="inTauri && updateOpen" class="about-card about-update">
      <div class="update-head" id="update-head">
        <div class="update-version">
          <span class="update-k">当前版本</span>
          <b class="update-v mono" id="update-current">{{ update?.currentVersion || version }}</b>
        </div>
        <div class="update-actions" id="update-actions">
          <button type="button" id="update-check" class="btn primary" :disabled="checking" @click="checkUpdate">
            检查更新
          </button>
        </div>
      </div>
      <div class="update-meta" id="update-meta">{{ phaseText }}</div>

      <div v-if="update?.phase === 'available' || update?.phase === 'downloading'" class="update-new-head">
        <strong>发现新版本 {{ update?.latestVersion }}</strong>
        <span class="update-k">{{ update?.publishedAt }}</span>
      </div>
      <pre v-if="update?.notes" class="update-opt">{{ update.notes }}</pre>

      <div v-if="update?.total" class="update-progress-meta" id="update-progress-meta">
        <div class="update-track"><div class="update-fill" id="update-fill" :style="{ width: progress + '%' }" /></div>
        <span class="update-pct" id="update-pct">{{ progress }}%</span>
        <span class="mono">{{ update?.source }}</span>
      </div>

      <div v-if="update?.phase === 'available'" class="update-buttons" id="update-buttons">
        <button type="button" class="btn primary">下载</button>
        <button type="button" class="btn">跳过这个版本</button>
      </div>

      <div v-if="updateError" class="update-error">{{ updateError }}</div>

      <div class="update-opts" id="update-opts">
        <label class="update-opt">
          <input type="checkbox" id="update-auto-check" checked disabled />
          <span>自动检查（启动 30 秒后 + 每 12 小时）</span>
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
        <span id="about-stat-fork">— Fork</span>
        <span id="about-stat-issue">— Issues</span>
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
</template>
