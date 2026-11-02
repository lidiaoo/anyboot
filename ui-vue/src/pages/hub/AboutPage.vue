<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { call, isTauri } from "@/api/tauri";

/**
 * 关于页（原 `ui/hub.html` 的 `#page-about`）。
 *
 * 三块：版本卡 / 更新（检查、下载、安装）/ 仓库与外链。
 *
 * **只在应用窗口里出现更新功能**：远程网页没有进度事件通道，
 * 也不该给远程开「装本机应用」的口子。
 */

interface UpdateState {
  phase: string;
  currentVersion: string;
  latestVersion: string;
  notes: string;
  publishedAt: string;
  assetName: string;
  assetSize: number;
  downloaded: number;
  total: number;
  error: string;
  checkedAt: string;
  source: string;
}

const inTauri = isTauri();
const version = ref("—");
const update = ref<UpdateState | null>(null);
const checking = ref(false);
const updateError = ref("");

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
    version.value = "—";
  }
  try {
    update.value = await call<UpdateState>("update_state");
  } catch (error) {
    updateError.value = String(error);
  }
});

async function checkUpdate() {
  if (checking.value) return;
  checking.value = true;
  updateError.value = "";
  try {
    update.value = await call<UpdateState>("update_check");
  } catch (error) {
    updateError.value = String(error);
  } finally {
    checking.value = false;
  }
}

function openRepo() {
  void call("open_web_page", { url: "https://github.com/anyboot/anyboot" }).catch(() => {});
}
</script>

<template>
  <div class="about-wrap">
    <div class="about-card about-hero">
      <span class="about-logo" aria-hidden="true">🖥</span>
      <h2 class="about-title">AnyBoot</h2>
      <p class="about-ver mono">v{{ version }} · 多系统启动管理器</p>
      <p class="about-desc">
        多系统一键切换 · GRUB 管理 · 远程电源控制 · SSH 跳板机隧道 · SFTP 文件传输
      </p>
      <div class="about-hero-actions">
        <button v-if="inTauri" type="button" class="about-pill" :disabled="checking" @click="checkUpdate">
          {{ checking ? "检查中…" : "检查更新 ›" }}
        </button>
        <button type="button" class="about-pill" @click="openRepo">项目主页 ›</button>
      </div>
    </div>

    <div v-if="inTauri" class="about-card">
      <div class="update-head">
        <div class="update-version">
          <span class="update-k">当前版本</span>
          <b class="mono">{{ update?.currentVersion || version }}</b>
        </div>
      </div>
      <div class="update-meta">{{ phaseText }}</div>

      <div v-if="update?.phase === 'available' || update?.phase === 'downloading'" class="update-new">
        <div class="update-new-head">
          <strong>发现新版本 {{ update.latestVersion }}</strong>
          <span class="update-k">{{ update.publishedAt }}</span>
        </div>
        <pre v-if="update.notes" class="update-notes">{{ update.notes }}</pre>
        <div v-if="update.total" class="update-progress">
          <div class="update-track"><div class="update-fill" :style="{ width: progress + '%' }" /></div>
          <div class="update-progress-meta">
            <span>{{ progress }}%</span>
            <span class="mono">{{ update.downloaded }} / {{ update.total }}</span>
            <span>{{ update.source }}</span>
          </div>
        </div>
      </div>

      <div v-if="updateError" class="update-error">{{ updateError }}</div>
    </div>

    <div class="about-card">
      <h3>快捷键</h3>
      <ul class="keys">
        <li><kbd>⌘</kbd><kbd>,</kbd> 打开本窗口</li>
        <li><kbd>⌘</kbd><kbd>K</kbd> 命令面板（引导页 G 主题）</li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.about-wrap {
  max-width: 680px;
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.about-card {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 12px;
  background: var(--panel, #fff);
  padding: 18px 20px;
}
.about-hero {
  text-align: center;
}
.about-logo {
  font-size: 30px;
  display: block;
  margin-bottom: 6px;
}
.about-title {
  font-size: 20px;
  margin: 0 0 4px;
}
.about-ver {
  color: var(--fg-faint, #9ca3af);
  font-size: 12px;
  margin: 0 0 10px;
}
.about-desc {
  font-size: 12.5px;
  color: var(--fg-dim, #6b7280);
  margin: 0 0 14px;
  line-height: 1.7;
}
.about-hero-actions {
  display: flex;
  gap: 8px;
  justify-content: center;
  flex-wrap: wrap;
}
.about-pill {
  border: 1px solid var(--line, #e5e7eb);
  background: var(--soft, #f9fafb);
  border-radius: 999px;
  padding: 6px 16px;
  font-size: 12.5px;
  cursor: pointer;
  color: var(--fg-dim, #4b5563);
}
.about-pill:hover {
  background: var(--soft, #f3f4f6);
}
.update-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.update-version {
  display: flex;
  align-items: baseline;
  gap: 8px;
}
.update-k {
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
}
.update-meta {
  font-size: 12.5px;
  color: var(--fg-dim, #6b7280);
  margin-top: 8px;
}
.update-new {
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px solid var(--line, #f1f5f9);
}
.update-new-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  margin-bottom: 8px;
}
.update-notes {
  font-size: 12px;
  color: var(--fg-dim, #4b5563);
  background: var(--soft, #f9fafb);
  border-radius: 8px;
  padding: 10px 12px;
  white-space: pre-wrap;
  max-height: 200px;
  overflow-y: auto;
  margin: 0 0 10px;
}
.update-track {
  height: 6px;
  border-radius: 3px;
  background: var(--soft, #f1f5f9);
  overflow: hidden;
}
.update-fill {
  height: 100%;
  background: var(--accent, #2563eb);
  transition: width 0.2s;
}
.update-progress-meta {
  display: flex;
  gap: 12px;
  font-size: 11px;
  color: var(--fg-faint, #9ca3af);
  margin-top: 5px;
}
.update-error {
  margin-top: 10px;
  font-size: 12px;
  color: #dc2626;
}
h3 {
  font-size: 13px;
  margin: 0 0 10px;
  color: var(--fg-dim, #6b7280);
}
.keys {
  list-style: none;
  padding: 0;
  margin: 0;
  font-size: 12.5px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  color: var(--fg-dim, #4b5563);
}
kbd {
  font-family: ui-monospace, Menlo, monospace;
  background: var(--soft, #f3f4f6);
  border-radius: 4px;
  padding: 1px 5px;
  margin-right: 3px;
  font-size: 11px;
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
}
</style>
