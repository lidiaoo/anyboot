<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 备份窗（原 `ui/hub-backup.html`，1057 行）—— backup 命令族 15 个。
 *
 * ## 1:1 结构（**独立窗自己的 `.panel` 系列**，不是主窗内嵌的 `.bk-*`）
 * ```
 * header > span.appicon + h1「备份与恢复」+ .tools（远程管理 / 主题）
 * main.scroll-thin
 *   section.panel#exportPanel
 *     .panel-head > .picon + h2 + .sub
 *     .cats#exportCats
 *     .info#exportKeyHint（私钥警告）
 *     .pw-zone#exportPwZone > .pw-grid > .field(.strength/.err)
 *     .row-actions > .btn.primary + .note
 *   section.panel#importPanel
 *   hr.divider
 *   section.panel#syncPanel
 *     .sync-head > .picon + h2 + .switch
 *     .form-grid > .form-row > .field × N
 *     .checkline / .cats / .pw-zone
 *     .sync-status
 * ```
 */

interface SyncState {
  enabled: boolean;
  server_url: string;
  username: string;
  directory: string;
  period_minutes: number;
  sync_on_change: boolean;
  categories: string[];
  encrypt: boolean;
  last_sync_ms?: number;
  last_error?: string;
  hasPassword?: boolean;
  hasEncryptPassword?: boolean;
  isConfigured?: boolean;
}

/** 分类（逐字照抄旧版 CATS，key 即备份文件载荷字段名） */
const CATEGORIES = [
  { id: "servers", name: "电脑列表", desc: "电脑档案与连接配置" },
  { id: "jumps", name: "SSH 服务器", desc: "跳板机 / 服务器档案（含密码明文）" },
  { id: "keys", name: "SSH 密钥", desc: "私钥与公钥（导出含私钥明文）" },
  { id: "terminal", name: "偏好设置", desc: "终端与传输偏好（跨端共同子集）" },
  { id: "history", name: "连接记录", desc: "会话历史与信任指纹" },
] as const;

const sync = ref<SyncState | null>(null);
const busy = ref("");
const message = ref("");
const errorText = ref("");

/** 是否勾了 keys（旧版 hasKeys） */
const exportIncludeKeys = computed(() => exportCats.value.includes("keys"));
const exportPw1 = ref("");
const exportPw2 = ref("");
const exportCats = ref<string[]>(["servers", "jumps", "keys", "terminal", "history"]);
// 旧版同步默认只勾 4 类（不含 history）
const syncCats = ref<string[]>(["servers", "jumps", "keys", "terminal"]);

/** 同步加密（旧版 #syncEncrypt / #syncEncryptPw / #syncEncryptErr） */
const syncEncrypt = ref(false);
const syncEncryptPw = ref("");
const syncEncryptErr = ref("");
/** 是否锁定（导入/导出进行中） */
const locked = computed(() => Boolean(busy.value));
/** 口令强度三档（旧版 refreshExportUi） */
const exportStrengthScore = computed(() => {
  const a = exportPw1.value;
  if (!a) return 0;
  return Math.min(3, (a.length >= 8 ? 1 : 0) + (/[0-9]/.test(a) && /[a-zA-Z]/.test(a) ? 1 : 0) + (/[^0-9a-zA-Z]/.test(a) ? 1 : 0));
});
const exportStrengthPct = computed(() => ((exportStrengthScore.value / 3) * 100).toFixed(0));
const exportStrengthColor = computed(() => {
  const s = exportStrengthScore.value;
  return s <= 1 ? "rgb(var(--bad))" : s === 2 ? "rgb(var(--warn))" : "rgb(var(--ok))";
});
const exportPwErr = computed(() => {
  const a = exportPw1.value, b = exportPw2.value;
  if (a && a.length < 4) return "口令至少 4 位";
  if (b && a !== b) return "两次输入不一致";
  return "";
});

const syncEnabled = ref(false);
const syncUrl = ref("");
const syncUser = ref("");
const syncPass = ref("");
const syncDir = ref("AnyBoot");
const syncPeriod = ref(15);
const syncOnChange = ref(false);

const dark = ref(false);
try {
  dark.value = localStorage.getItem("anyboot.theme") === "dark";
} catch {
  /* 吃默认 */
}
// 主题跟随（旧版 backup 窗监听 theme-changed）—— onMounted 里注册
function applyTheme(isDark: boolean) {
  const html = document.documentElement;
  html.setAttribute("data-theme", isDark ? "dark" : "light");
  html.classList.toggle("light", !isDark);
  html.classList.toggle("dark", isDark);
}
applyTheme(dark.value);

function toggleTheme() {
  dark.value = !dark.value;
  applyTheme(dark.value);
  try {
    localStorage.setItem("anyboot.theme", dark.value ? "dark" : "light");
  } catch {
    /* 忽略 */
  }
}

function openHub() {
  void call("hub_open_window", {}).catch(() => {});
}

async function refreshSync() {
  try {
    sync.value = await call<SyncState>("backup_sync_get_state");
    // Rust SyncState 序列化为 snake_case（旧版同款读取）
    syncEnabled.value = Boolean(sync.value?.enabled);
    syncUrl.value = sync.value?.server_url ?? "";
    syncUser.value = sync.value?.username ?? "";
    syncDir.value = sync.value?.directory || "AnyBoot";
    syncPeriod.value = sync.value?.period_minutes ?? 30;
    syncOnChange.value = Boolean(sync.value?.sync_on_change);
  } catch (error) {
    errorText.value = String(error);
  }
}

onMounted(refreshSync);

// 主题跟随（旧版 backup 窗监听 theme-changed）
onMounted(async () => {
  try {
    const { listen } = await import("@tauri-apps/api/event");
    void listen<{ theme?: string }>("theme-changed", (event) => {
      const theme = event.payload?.theme;
      if (theme) applyTheme(theme !== "light");
    });
  } catch { /* 非 Tauri 环境 */ }
});

function toggleCat(which: "export" | "sync" | "import", id: string) {
  const source = which === "export" ? exportCats : which === "sync" ? syncCats : importCats;
  const next = source.value.includes(id)
    ? source.value.filter((x) => x !== id)
    : [...source.value, id];
  if (which === "export") exportCats.value = next;
  else if (which === "sync") syncCats.value = next;
  else importCats.value = next;
}

async function run(label: string, action: () => Promise<unknown>, done: string) {
  if (busy.value) return;
  busy.value = label;
  message.value = "";
  errorText.value = "";
  try {
    await action();
    message.value = done;
    await refreshSync();
  } catch (error) {
    errorText.value = String(error);
  } finally {
    busy.value = "";
  }
}

async function doExport() {
  if (exportPw1.value !== exportPw2.value) {
    errorText.value = "两次口令不一致";
    return;
  }
  await run(
    "导出",
    () =>
      call("backup_export", {
        categories: exportCats.value,
        includeKeys: exportIncludeKeys.value,
        password: exportPw1.value || "",
      }),
    "备份文件已导出",
  );
}

async function doImport() {
  let path = "";
  try {
    path = ((await call("backup_pick_import_file")) as string) ?? "";
  } catch {
    path = "";
  }
  if (!path) return;
  // 独立窗简化流程：直接按默认「跳过已存在」策略导入（完整预览在 hub 备份页）
  void run(
    "导入",
    () => call("backup_import", { path, password: "", categories: [], strategy: "skipExisting" }),
    "导入完成（设置已恢复）",
  );
}

/** 规范化 WebDAV 地址（旧版 normalizeDavUrl） */
function normalizeDavUrl(raw: string): string {
  const url = String(raw || "").trim();
  if (!url) return "";
  if (/^https?:\/\//i.test(url)) return url;
  return "https://" + url.replace(/^\/\//, "");
}

function saveSync() {
  void run(
    "保存同步配置",
    () =>
      call("backup_sync_save_config", {
        config: {
          serverUrl: normalizeDavUrl(syncUrl.value),
          username: syncUser.value.trim(),
          password: syncPass.value,
          directory: syncDir.value.trim() || "AnyBoot",
          periodMinutes: syncPeriod.value,
          syncOnChange: syncOnChange.value,
          categories: syncCats.value,
          encrypt: syncEncrypt.value,
          enabled: syncEnabled.value,
        },
      }),
    "同步配置已保存",
  );
}

function toggleSync() {
  const next = !syncEnabled.value;
  syncEnabled.value = next;
  void run(
    next ? "启用同步" : "关闭同步",
    () => call("backup_sync_set_enabled", { enabled: next }),
    next ? "已启用 WebDAV 同步" : "已关闭 WebDAV 同步",
  );
}

function testWebdav() {
  void run("测试 WebDAV", () => (async () => { await saveSync(); return call("backup_webdav_test", { config: {} }); })(), "WebDAV 连接正常");
}

function syncNow() {
  void run("立即同步", () => call("backup_sync_now"), "同步完成");
}

/** 导入三段式与弹层状态 */
const pwOpen = ref(false);
const importPw = ref("");
const importPwErr = ref("");
const previewOpen = ref(false);
const previewRows = ref<{ label: string; inBackup: number; local: number; conflicts: number }[]>([]);
const previewWarn = ref("");
const previewSrc = ref("");
const importPath = ref("");
const importCats = ref<string[]>(["servers", "jumps", "keys", "terminal", "history"]);
const importStrategy = ref<"skipExisting" | "overwrite">("skipExisting");
const confirmOpen = ref(false);
const confirmBody = ref("");
const resultOpen = ref(false);
const resultRows = ref<{ k: string; v: string }[]>([]);
const toastText = ref("");

function showToast(text: string) {
  toastText.value = text;
  window.setTimeout(() => {
    if (toastText.value === text) toastText.value = "";
  }, 2600);
}
function submitPw() {
  if (!importPw.value.trim()) {
    importPwErr.value = "请输入口令";
    return;
  }
  pwOpen.value = false;
}
function startImport() {
  previewOpen.value = false;
  const total = previewRows.value.reduce((s, r) => s + r.conflicts, 0);
  if (importStrategy.value === "overwrite" && total > 0) {
    confirmBody.value = `备份中有 ${total} 条与本机同 ID 的条目，将以备份内容整条覆盖。`;
    confirmOpen.value = true;
    return;
  }
  void executeImport();
}
async function executeImport() {
  confirmOpen.value = false;
  if (busy.value) return;
  busy.value = "导入";
  try {
    const res = (await call("backup_import", {
      path: importPath.value,
      password: importPw.value,
      categories: importCats.value,
      strategy: importStrategy.value,
    })) as Record<string, unknown>;
    resultRows.value = Object.entries(res ?? {}).map(([k, v]) => ({ k, v: String(v) }));
    resultOpen.value = true;
    showToast("导入完成");
  } catch (error) {
    errorText.value = String(error);
  } finally {
    busy.value = "";
  }
}
</script>

<template>
  <header>
    <span class="appicon" aria-hidden="true">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M21 8v13H3V8M1 3h22v5H1zM10 12h4" />
      </svg>
    </span>
    <h1>备份与恢复</h1>
    <div class="tools">
      <button type="button" class="toolbtn" id="hubBtn" aria-label="打开远程管理" title="打开远程管理" @click="openHub">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect x="2" y="3" width="20" height="7" rx="2" />
          <rect x="2" y="14" width="20" height="7" rx="2" />
          <path d="M6 6.5h.01M6 17.5h.01" />
        </svg>
        远程管理
      </button>
      <button type="button" class="toolbtn" id="themeBtn" aria-label="切换主题" @click="toggleTheme">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 3a9 9 0 0 1 0 18z" fill="currentColor" stroke="none" />
        </svg>
        主题
      </button>
    </div>
  </header>

  <main class="scroll-thin">
    <!-- 导出 -->
    <!-- 导出（旧版 #exportPanel） -->
    <section class="panel" id="exportPanel">
      <div class="panel-head">
        <span class="picon" aria-hidden="true">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 15V3M7 8l5-5 5 5M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
          </svg>
        </span>
        <h2>导出备份</h2>
        <span class="sub">导出为 .json 备份文件，可在手机 / 电脑间互导</span>
      </div>

      <div class="cats" id="exportCats">
        <label v-for="cat in CATEGORIES" :key="cat.id" class="radio-line">
          <input
            type="checkbox"
            :checked="exportCats.includes(cat.id)"
            @change="toggleCat('export', cat.id)"
          />
          {{ cat.name }}
        </label>
      </div>

      <div v-if="exportIncludeKeys" id="exportKeyHint" class="info">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 8v5M12 16h.01" />
        </svg>
        <span>导出包含 <b>SSH 私钥明文</b>，请设置口令加密，并妥善保管备份文件。</span>
      </div>

      <div v-if="exportIncludeKeys" id="exportPwZone" class="pw-zone">
        <div class="pw-grid">
          <div class="field">
            <label for="expPw1">备份口令</label>
            <input id="expPw1" v-model="exportPw1" type="password" placeholder="至少 4 位" autocomplete="new-password" />
            <div class="strength"><i id="expStrength" :style="{ width: exportStrengthPct + '%', background: exportStrengthColor }" /></div>
          </div>
          <div class="field">
            <label for="expPw2">再输一次</label>
            <input id="expPw2" v-model="exportPw2" type="password" placeholder="两次需一致" autocomplete="new-password" />
            <div class="err" id="expPwErr">{{ exportPwErr }}</div>
          </div>
        </div>
      </div>

      <div class="row-actions">
        <button type="button" class="btn primary" id="exportBtn" :disabled="!!busy" @click="doExport">
          <template v-if="busy"><span class="spin"></span>正在组包…</template>
          <template v-else>导出备份文件</template>
        </button>
        <span class="note" id="exportNote">不加密也能导出，但密钥明文会直接写进文件</span>
      </div>
    </section>

    <!-- 导入（旧版 #importPanel） -->
    <section class="panel" id="importPanel">
      <div class="panel-head">
        <span class="picon" aria-hidden="true">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 3v12M8 7l4-4 4 4M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
          </svg>
        </span>
        <h2>导入备份</h2>
        <span class="sub">从 .json 备份文件恢复（手机导出的备份同样可用）</span>
      </div>
      <div class="row-actions">
        <button type="button" class="btn primary" id="importBtn" :disabled="!!busy" @click="doImport">
          <template v-if="busy"><span class="spin"></span>{{ busy }}</template>
          <template v-else>选择备份文件…</template>
        </button>
        <span class="note">导入前会先预览差异，再让你确认</span>
      </div>
    </section>

    <hr class="divider" />

    <!-- WebDAV 同步（旧版 #syncPanel） -->
    <section class="panel" id="syncPanel">
      <div class="sync-head">
        <span class="picon" aria-hidden="true">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 7h-9l-2-2H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2z" />
          </svg>
        </span>
        <h2>WebDAV 云同步</h2>
        <label class="switch" style="margin-left: auto" title="开启 / 关闭同步">
          <input type="checkbox" id="syncEnabled" :checked="syncEnabled" @change="toggleSync" />
          <i />
        </label>
      </div>

      <div class="form-grid" id="syncForm">
        <div class="form-row">
          <div class="field" style="flex: 2 1 280px">
            <label for="syncUrl">服务器地址</label>
            <input id="syncUrl" v-model="syncUrl" placeholder="https://dav.jianguoyun.com/dav/" autocomplete="off" />
          </div>
          <div class="field">
            <label for="syncDir">目录</label>
            <input id="syncDir" v-model="syncDir" placeholder="AnyBoot" autocomplete="off" />
          </div>
        </div>

        <div class="form-row">
          <div class="field">
            <label for="syncUser">账号</label>
            <input id="syncUser" v-model="syncUser" placeholder="WebDAV 账号" autocomplete="off" />
          </div>
          <div class="field">
            <label for="syncPass">密码 / 应用密码</label>
            <input
              id="syncPass"
              v-model="syncPass"
              type="password"
              :placeholder="sync?.hasPassword ? '已保存，留空则不修改' : 'WebDAV 密码'"
              autocomplete="new-password"
            />
          </div>
        </div>

        <div class="form-row">
          <div class="field">
            <label>自动同步频率</label>
            <div class="seg" id="syncPeriod">
              <button
                v-for="opt in [
                  { v: 15, label: '15 分钟' },
                  { v: 30, label: '30 分钟' },
                  { v: 60, label: '60 分钟' },
                  { v: 0, label: '手动' },
                ]"
                :key="opt.v"
                type="button"
                :aria-selected="syncPeriod === opt.v"
                :class="{ 'is-on': syncPeriod === opt.v }"
                @click="syncPeriod = opt.v"
              >
                {{ opt.label }}
              </button>
            </div>
          </div>
        </div>

        <label class="checkline">
          <input id="syncOnChange" v-model="syncOnChange" type="checkbox" />
          有变化时自动同步（改动后约 30 秒内推送）
        </label>

        <div class="cats" id="syncCats">
          <label v-for="cat in CATEGORIES" :key="cat.id" class="radio-line">
            <input
              type="checkbox"
              :checked="syncCats.includes(cat.id)"
              @change="toggleCat('sync', cat.id)"
            />
            {{ cat.name }}
          </label>
        </div>

        <!-- 云端备份加密（旧版 #syncEncryptZone） -->
        <div class="pw-zone" id="syncEncryptZone">
          <label class="checkline">
            <input id="syncEncrypt" v-model="syncEncrypt" type="checkbox" />
            云端备份加密（推荐 —— 同步文件含密钥明文）
          </label>
          <div class="field" id="syncEncryptPwField" :hidden="!syncEncrypt" style="max-width: 320px">
            <label for="syncEncryptPw">同步口令</label>
            <input id="syncEncryptPw" v-model="syncEncryptPw" type="password" :placeholder="sync?.hasPassword ? '已保存，留空则不修改' : '同步口令'" autocomplete="new-password" />
            <span class="err" id="syncEncryptErr">{{ syncEncryptErr }}</span>
          </div>
        </div>
      </div>

      <div class="sync-status" id="syncStatus">
        <span v-if="sync?.last_error" class="err">{{ sync.last_error }}</span>
        <span v-else-if="sync?.last_sync_ms">上次同步：{{ new Date(sync.last_sync_ms).toLocaleString() }}</span>
        <span v-else>还没有同步过</span>
      </div>

      <div v-if="locked" class="info" id="syncLockInfo">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 8v5M12 16h.01" />
        </svg>
        <span>导入 / 导出进行中，同步暂时锁定。</span>
      </div>

      <div class="row-actions">
        <button type="button" class="btn" id="syncTestBtn" :disabled="!!busy" @click="testWebdav">测试连接</button>
        <button type="button" class="btn" id="syncSaveBtn" :disabled="!!busy" @click="saveSync">保存配置</button>
        <button type="button" class="btn primary" id="syncNowBtn" :disabled="!syncEnabled || !!busy" @click="syncNow">
          立即同步
        </button>
      </div>
    </section>

    <p v-if="message" class="note" style="color: rgb(var(--ok))">{{ message }}</p>
    <p v-if="errorText" class="err">{{ errorText }}</p>

  <!-- 口令弹层（旧版 #pwModal） -->
  <div v-if="pwOpen" id="pwModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="pwTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="pwTitle">此备份已加密</h3>
        <button type="button" id="pwClose" aria-label="关闭" @click="pwOpen = false">✕</button>
      </div>
      <form id="pwForm" class="modal-body" @submit.prevent="submitPw">
        <div class="field">
          <label for="importPw">备份口令</label>
          <input id="importPw" v-model="importPw" type="password" autocomplete="off" />
          <div class="err" id="importPwErr">{{ importPwErr }}</div>
          <p class="modal-note">口令错误 3 次后提示：口令由导出方设置，本机无法找回。</p>
        </div>
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="pwCancel" @click="pwOpen = false">取消</button>
          <button type="submit" class="btn primary">解锁预览</button>
        </div>
      </form>
    </div>
  </div>

  <!-- 导入预览（旧版 #previewModal） -->
  <div v-if="previewOpen" id="previewModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="prevTitle">
    <div class="modal-card wide">
      <div class="modal-head">
        <h3 id="prevTitle">导入预览</h3>
        <button type="button" id="prevClose" aria-label="关闭" @click="previewOpen = false">✕</button>
      </div>
      <div class="modal-body">
        <div class="src-line" id="prevSrc">{{ previewSrc }}</div>
        <table class="prev">
          <thead><tr><th>分类</th><th style="text-align: right">备份内</th><th style="text-align: right">本机现有</th><th>同 ID 冲突</th></tr></thead>
          <tbody id="prevRows">
            <tr v-for="row in previewRows" :key="row.label">
              <td>{{ row.label }}</td>
              <td style="text-align: right">{{ row.inBackup }}</td>
              <td style="text-align: right">{{ row.local }}</td>
              <td>
                <span v-if="row.conflicts" class="chip conflict">{{ row.conflicts }} 个同 ID</span>
                <span v-else>—</span>
              </td>
            </tr>
            <tr v-if="!previewRows.length"><td colspan="4">（备份文件里没有可识别的内容）</td></tr>
          </tbody>
        </table>
        <div v-if="previewWarn" id="prevWarn" class="info warn">
          <span id="prevWarnText">{{ previewWarn }}</span>
        </div>
        <div class="cats" id="importCats">
          <label v-for="cat in CATEGORIES" :key="cat.id" class="radio-line">
            <input type="checkbox" :checked="importCats.includes(cat.id)" @change="toggleCat('import', cat.id)" />
            {{ cat.name }}
          </label>
        </div>
        <div class="strategy" id="strategyBox">
          <label class="radio-line">
            <input v-model="importStrategy" type="radio" name="strategy" value="skipExisting" />
            <span>跳过已存在的条目<span class="rdesc">只添加本机没有的；现有配置一律不动（安全）</span></span>
          </label>
          <label class="radio-line">
            <input v-model="importStrategy" type="radio" name="strategy" value="overwrite" />
            <span>用备份覆盖同 ID 条目<span class="rdesc">同 ID 的本机档案会被备份内容整条替换；本机独有的条目保留</span></span>
          </label>
        </div>
      </div>
      <div class="modal-foot">
        <button type="button" class="btn plain" id="prevCancel" @click="previewOpen = false">取消</button>
        <button type="button" class="btn primary" id="prevGo" @click="startImport">开始导入</button>
      </div>
    </div>
  </div>

  <!-- 覆盖确认（旧版 #confirmModal） -->
  <div v-if="confirmOpen" id="confirmModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="cfmTitle">
    <div class="modal-card">
      <div class="del-hero">
        <span class="icon" aria-hidden="true">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 9v4M12 17h.01M10.3 3.9L1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" /></svg>
        </span>
        <h3 id="cfmTitle">确认覆盖导入？</h3>
      </div>
      <div class="modal-body">
        <p class="del-body" id="cfmBody">{{ confirmBody }}</p>
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="cfmCancel" @click="confirmOpen = false">取消</button>
          <button type="button" class="btn danger" id="cfmOk" @click="executeImport">覆盖导入</button>
        </div>
      </div>
    </div>
  </div>

  <!-- 导入结果（旧版 #resultModal） -->
  <div v-if="resultOpen" id="resultModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="resTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="resTitle">导入完成</h3>
        <button type="button" id="resClose" aria-label="关闭" @click="resultOpen = false">✕</button>
      </div>
      <div class="modal-body" id="resBody">
        <p v-for="row in resultRows" :key="row.k">{{ row.k }}：{{ row.v }}</p>
        <p v-if="!resultRows.length">导入已完成</p>
      </div>
      <div class="modal-foot">
        <button type="button" class="btn primary" id="resOk" @click="resultOpen = false">完成</button>
      </div>
    </div>
  </div>

  <!-- toast（旧版 #toast） -->
  <div v-if="toastText" id="toast">{{ toastText }}</div>
  </main>
</template>
