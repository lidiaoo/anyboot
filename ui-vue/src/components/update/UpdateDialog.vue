<script setup lang="ts">
/**
 * 版本更新弹窗 —— **PC 端唯一的更新弹窗组件**（弹窗窗口 update.html 与应用内「关于」页共用同一份）。
 * 样式/按钮顺序与移动端（Android AlertDialog / iOS alert）一致：
 *   标题 + 当前版本 + 更新日志 + 进度 + 下载源（下拉/自定义）+ 跳过 / 稍后 / 下载并安装。
 */
import { computed, onMounted, ref, watch } from "vue";

const props = defineProps<{ inline?: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

interface UpdateState {
  phase: string;
  currentVersion: string;
  latestVersion: string;
  notes: string;
  publishedAt: string;
  assetName: string;
  downloaded: number;
  total: number;
  error: string;
  source: string;
  skipped?: boolean;
  autoCheck?: boolean;
  mirror?: string;
  intervalHours?: number;
  skipUntilEpoch?: number;
}

const invoke = (window as any).__TAURI__?.core?.invoke as
  | ((cmd: string, args?: Record<string, unknown>) => Promise<any>)
  | undefined;
const listen = (window as any).__TAURI__?.event?.listen as
  | ((event: string, cb: (e: { payload: any }) => void) => Promise<any>)
  | undefined;

const state = ref<UpdateState | null>(null);
const errorText = ref("");
const KNOWN_MIRRORS = ["auto", "direct", "ghfast", "ghproxy"] as const;
const mirror = ref<string>("auto");
const customUrl = ref("");
const optionsReady = ref(false);
const busy = ref(false);

const phase = computed(() => state.value?.phase ?? "idle");
const percent = computed(() => {
  const total = state.value?.total ?? 0;
  if (!total) return 0;
  return Math.min(100, Math.round(((state.value?.downloaded ?? 0) / total) * 100));
});
const mb = (n: number) => (n / 1048576).toFixed(1);
const bytesText = computed(() => {
  const total = state.value?.total ?? 0;
  const downloaded = state.value?.downloaded ?? 0;
  if (!total) return downloaded ? `${mb(downloaded)} MB` : "";
  return `${mb(downloaded)}/${mb(total)} MB`;
});
const downloading = computed(() => phase.value === "downloading" || phase.value === "verifying");
const canInstall = computed(() => phase.value === "ready_to_install" || phase.value === "installing");
/** 有「新版本」语义的阶段（标题/日志/跳过/下载源只在这些阶段出现） */
const assetMB = computed(() => {
  const size = state.value?.total ?? 0;
  return size ? `${(size / 1048576).toFixed(1)} MB` : "";
});
const hasRelease = computed(() =>
  ["available", "downloading", "verifying", "ready_to_install", "installing"].includes(phase.value),
);
/** 是否处于检查中/未知（此时不该显示任何下载相关内容） */
const checkingLike = computed(() => phase.value === "checking" || phase.value === "idle");
const title = computed(() => {
  if (hasRelease.value) return state.value?.latestVersion ? `发现新版本 v${state.value.latestVersion}` : "发现新版本";
  if (phase.value === "up_to_date") return "已是最新版本";
  if (phase.value === "error") return "检查更新失败";
  if (checkingLike.value) return "正在检查更新";
  return "检查更新";
});
/** 主按钮永远可见：有新版本=下载；下载/校验/安装中=对应文案；已最新/检查中=置灰并说明 */
const primaryLabel = computed(() => {
  switch (phase.value) {
    case "available":
      return "下载";
    case "downloading":
      return `下载中 ${percent.value}%`;
    case "verifying":
      return "校验中…";
    case "ready_to_install":
      return "安装并重启";
    case "installing":
      return "安装中…";
    case "checking":
      return "检查中…";
    case "up_to_date":
      return "已是最新";
    case "error":
      return state.value?.assetName ? "重试下载" : "重新检查";
    default:
      return "检查更新";
  }
});
const primaryDisabled = computed(() =>
  busy.value || ["downloading", "verifying", "installing"].includes(phase.value),
);
function onPrimary(): void {
  switch (phase.value) {
    case "available":
    case "error":
      if (state.value?.assetName) void run("update_download");
      else void run("update_check");
      break;
    case "ready_to_install":
      void run("update_install");
      break;
    default:
      break;
  }
}

function applyState(next: UpdateState | null): void {
  if (!next) return;
  state.value = next;
  const loaded = String(next.mirror ?? "auto");
  if ((KNOWN_MIRRORS as readonly string[]).includes(loaded)) {
    mirror.value = loaded;
    customUrl.value = "";
  } else if (loaded) {
    mirror.value = "custom";
    customUrl.value = loaded;
  }
  optionsReady.value = true;
}

async function refresh(): Promise<void> {
  if (!invoke) return;
  try {
    applyState(await invoke("update_state"));
  } catch (e) {
    errorText.value = String(e);
  }
}

async function run(cmd: string, args?: Record<string, unknown>): Promise<void> {
  if (!invoke) return;
  busy.value = true;
  try {
    await invoke(cmd, args);
    await refresh();
  } catch (e) {
    errorText.value = String(e);
  } finally {
    busy.value = false;
  }
}

function applyCustomMirror(): void {
  const value = customUrl.value.trim();
  if (!value) return;
  void run("set_update_mirror", { value }).catch(() => {});
}

watch(mirror, (value) => {
  if (!optionsReady.value || value === "custom") return;
  void run("set_update_mirror", { value });
});

async function close(): Promise<void> {
  if (props.inline) {
    emit("close");
    return;
  }
  // 弹窗窗口里 `window.__TAURI__.window` 不一定有实例（之前点「稍后」没反应就是这里），
  // 统一交给后端命令关窗；命令失败再退回浏览器侧 API。
  try {
    if (invoke) {
      await invoke("update_close_window");
      return;
    }
  } catch {
    /* 落到下面的兜底 */
  }
  try {
    (window as any).__TAURI__.window.getCurrentWindow().close();
  } catch {
    emit("close");
  }
}

/** 阶梯忽略：0 = 永久，其余 = 忽略 N 天 */
async function skipForDays(days: number): Promise<void> {
  await run("update_dismiss", { version: state.value?.latestVersion || "", days });
  await close();
}

onMounted(async () => {
  listen?.("update-state", (event) => applyState(event.payload));
  await refresh();
  if (!state.value || state.value.phase === "idle") {
    void run("update_check");
  }
  // 兜底轮询（弹窗窗口可能收不到焦点，事件极端情况下会漏）
  setInterval(refresh, 2000);
});
</script>

<template>
  <div :class="['upd-card', { 'is-inline': props.inline }]" role="dialog" aria-modal="true">
    <header class="upd-head">
      <h3>{{ title }}</h3>
      <small v-if="state?.currentVersion">当前 v{{ state.currentVersion }}</small>
    </header>
    <p v-if="state?.publishedAt" class="upd-meta">发布于 {{ state.publishedAt }}</p>

    <template v-if="hasRelease">
      <pre v-if="state?.notes" class="upd-notes">{{ state.notes }}</pre>
      <p v-else class="upd-notes empty">（这个版本没有填写更新日志）</p>
    </template>

    <p v-if="phase === 'available' && assetMB" class="upd-size">安装包大小 <span class="mono">{{ assetMB }}</span></p>
    <div v-if="downloading || canInstall" class="upd-progress">
      <div class="bar"><span :style="{ width: percent + '%' }" /></div>
      <div class="nums">
        <span>{{ percent }}%</span>
        <span class="mono">{{ bytesText }}</span>
        <span v-if="state?.source">来源：{{ state.source }}</span>
      </div>
    </div>

    <p v-if="state?.error || errorText" class="upd-error">{{ state?.error || errorText }}</p>

    <!-- 下载源：下拉 + 自定义（与「关于」页同一套取值，落盘 setting）；只在有新版时显示 -->
    <div v-if="hasRelease" class="upd-source">
      <span class="k">下载源</span>
      <select v-model="mirror">
        <option value="auto">自动（直连 → 镜像）</option>
        <option value="direct">直连</option>
        <option value="ghfast">ghfast.top</option>
        <option value="ghproxy">gh-proxy.com</option>
        <option value="custom">自定义…</option>
      </select>
      <input
        v-if="mirror === 'custom'"
        v-model="customUrl"
        type="url"
        class="mono"
        placeholder="https://ghfast.top/"
        @change="applyCustomMirror"
        @keyup.enter="applyCustomMirror"
      />
    </div>

    <footer class="upd-actions">
      <!-- 有新版：跳过（阶梯忽略）/稍后/下载·安装 -->
      <template v-if="hasRelease">
        <select
          class="skip-select"
          :disabled="busy"
          title="跳过此版本（可选择忽略时长）"
          @change="(e) => { const v = Number((e.target as HTMLSelectElement).value); (e.target as HTMLSelectElement).value = ''; if (!Number.isNaN(v)) skipForDays(v); }"
        >
          <option value="">跳过此版本…</option>
          <option value="1">忽略 1 天</option>
          <option value="7">忽略 7 天</option>
          <option value="30">忽略 30 天</option>
          <option value="0">永久跳过此版本</option>
        </select>
        <button type="button" class="ghost" @click="run('update_remind_later', { hours: 24 }).then(close)">稍后提醒</button>
        <button type="button" class="primary" :disabled="primaryDisabled" @click="onPrimary">
          {{ primaryLabel }}
        </button>
      </template>

      <!-- 检查中：只给关闭（避免在"还在查"的界面放任何下载按钮） -->
      <template v-else-if="checkingLike">
        <button type="button" class="ghost" @click="close">关闭</button>
      </template>

      <!-- 已是最新：说明 + 重新检查（没有下载/跳过这些不适用的动作） -->
      <template v-else-if="phase === 'up_to_date'">
        <button type="button" class="ghost" @click="run('update_check')">重新检查</button>
        <button type="button" class="primary" @click="close">关闭</button>
      </template>

      <!-- 失败：重新检查 / 关闭 -->
      <template v-else>
        <button type="button" class="ghost" @click="close">关闭</button>
        <button type="button" class="primary" :disabled="busy" @click="run('update_check')">重新检查</button>
      </template>
    </footer>
    <p v-if="hasRelease" class="upd-hint">下载源与检查频率可在「远程管理 → 关于」里设置 · 关闭窗口 = 稍后再说</p>
  </div>
</template>

<style scoped>
.upd-card {
  box-sizing: border-box;
  width: min(560px, 96vw);
  margin: 18px auto;
  padding: 20px 22px 16px;
  border-radius: 18px;
  background: var(--panel, #ffffff);
  color: var(--fg, #1f2328);
  border: 1px solid var(--line, #e4e4e7);
  box-shadow: 0 18px 48px rgba(0, 0, 0, 0.25);
  font: 13px/1.55 system-ui, -apple-system, "Segoe UI", "Noto Sans CJK SC", sans-serif;
}
.upd-head { display: flex; align-items: baseline; gap: 10px; }
.upd-head h3 { margin: 0; font-size: 16px; font-weight: 650; }
.upd-head .v { color: var(--accent, #3b82f6); }
.upd-head small { color: var(--dim, #6b7280); font-family: ui-monospace, monospace; }
.upd-meta { margin: 4px 0 10px; color: var(--dim, #6b7280); font-size: 12px; }
.upd-notes {
  margin: 0 0 12px;
  max-height: 220px;
  overflow: auto;
  white-space: pre-wrap;
  font: 12px/1.6 ui-monospace, monospace;
  background: color-mix(in srgb, var(--fg, #1f2328) 7%, transparent);
  border-radius: 10px;
  padding: 10px 12px;
}
.upd-notes.empty { color: var(--dim, #6b7280); }
.upd-size { margin: 0 0 10px; color: var(--dim, #6b7280); font-size: 12px; }
.upd-progress { margin: 0 0 10px; }
.upd-progress .bar { height: 8px; border-radius: 4px; background: rgba(127, 127, 127, 0.25); overflow: hidden; }
.upd-progress .bar span { display: block; height: 100%; background: var(--accent, #3b82f6); transition: width 0.12s linear; }
.upd-progress .nums { display: flex; gap: 12px; margin-top: 5px; color: var(--dim, #6b7280); font-size: 11px; }
.upd-error { margin: 0 0 10px; color: #dc2626; white-space: pre-wrap; }
.upd-source { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin: 4px 0 14px; }
.upd-source .k { color: var(--dim, #6b7280); font-size: 12px; }
.upd-source select, .upd-source input {
  font: inherit;
  font-size: 12px;
  padding: 5px 8px;
  border-radius: 8px;
  border: 1px solid var(--line, #d4d4d8);
  background: transparent;
  color: inherit;
  min-width: 0;
}
.upd-source input { flex: 1 1 220px; }
.upd-actions { display: flex; gap: 8px; justify-content: flex-end; flex-wrap: wrap; }
.upd-actions select.skip-select {
  font: inherit;
  font-size: 12px;
  border-radius: 9px;
  padding: 7px 10px;
  border: 1px solid var(--line, #d4d4d8);
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.upd-actions button {
  font: inherit;
  border-radius: 9px;
  padding: 7px 14px;
  border: 1px solid var(--line, #d4d4d8);
  background: transparent;
  color: inherit;
  cursor: pointer;
}
.upd-actions button.primary { background: var(--accent, #3b82f6); border-color: var(--accent, #3b82f6); color: #fff; }
.upd-actions button:disabled { opacity: 0.55; cursor: not-allowed; }
.upd-hint { margin: 10px 0 0; color: var(--faint, #9ca3af); font-size: 11px; }
.upd-card.is-inline { width: 100%; margin: 0; box-shadow: none; }
</style>
