<script setup lang="ts">
import WindowNavMenu from "@/components/hub/WindowNavMenu.vue";
import { t } from "@/i18n";
import { computed, onMounted, ref , onUnmounted } from "vue";
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
  { id: "servers", name: "bk.catServers", desc: "bk.descServers" },
  { id: "jumps", name: "bk.catJumps", desc: "bk.descJumps" },
  { id: "keys", name: "bk.catKeys", desc: "bk.descKeys" },
  { id: "terminal", name: "bk.catTerminal", desc: "bk.descTerminal" },
  { id: "history", name: "bk.catHistory", desc: "bk.descHistory" },
] as const;

const sync = ref<SyncState | null>(null);
const busy = ref("");
const message = ref("");
const errorText = ref("");

/** 是否勾了 keys（旧版 hasKeys） */
const exportIncludeKeys = computed(() => exportCats.value.includes("keys"));
const exportPw1 = ref("");
const exportPw2 = ref("");
const exportPasswordValid = () => exportPw1.value.length >= 4 && exportPw1.value === exportPw2.value;
const exportDisabled = computed(
  () => Boolean(busy.value) || (exportIncludeKeys.value && !exportPasswordValid()),
);
const exportNoteText = computed(() => {
  if (exportIncludeKeys.value) {
    return exportPasswordValid()
      ? t("bk.exportEncryptWithKeys")
      : t("bk.needPwForKeys");
  }
  return exportPw1.value ? t("bk.exportEncrypted") : t("bk.exportPlain");
});
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
  if (a && a.length < 4) return t("bk.pwMin");
  if (b && a !== b) return t("bk.pwMismatch");
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
  dark.value = localStorage.getItem("anyboot.theme-resolved") === "dark";
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


// 页面心跳（旧版）：后端据此区分「窗口对象还在」和「页面还活着」——
// 停跳 15s = 僵尸窗口 → 销毁重建。不发心跳活窗口会被反复重建！
void call("hub_page_ping", { ready: true }).catch(() => {});
const pagePingTimer = window.setInterval(() => {
  void call("hub_page_ping", { ready: true }).catch(() => {});
}, 3000);
onUnmounted(() => window.clearInterval(pagePingTimer));

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
  // 旧版校验：keys 或加密码 → 必须 ≥4 位且两次一致
  const usePw = Boolean(exportPw1.value);
  if ((exportIncludeKeys.value || usePw) && (exportPw1.value.length < 4 || exportPw1.value !== exportPw2.value)) {
    showToast(t("bk.pwNotSet"));
    return;
  }
  // 旧版流程：先挑保存路径，用户取消就不导出
  let path = "";
  try {
    path = ((await call("backup_pick_export_path")) as string) ?? "";
  } catch {
    path = "";
  }
  if (!path) return;
  await run(
    t("bk.exportRunLabel"),
    async () => {
      // password null = 不加密（旧版语义；空串会被当空口令加密）
      const msg = (await call<string>("backup_export", {
        path,
        categories: exportIncludeKeys.value
          ? [...exportCats.value, "keys"]
          : exportCats.value,
        password: usePw ? exportPw1.value : null,
      })) || "";
      showToast(t("bk.exportedMsg", { msg, path }));
      return msg;
    },
    t("bk.exportedTitle"),
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
  importPath.value = path;
  try {
    // 旧版完整流：先预览（加密备份会要求口令）
    const preview = (await call("backup_preview", { path, password: importPw.value || null })) as Record<string, unknown>;
    fillPreview(preview);
  } catch (error) {
    const msg = String(error);
    if (msg.includes("__NEED_PASSWORD__")) {
      importPwErr.value = "";
      pwOpen.value = true; // 弹口令弹层（旧版 openPwModal）
      return;
    }
    showToast(msg);
  }
}

/** 预览结果 → 弹层（旧版 openPreview） */
function fillPreview(preview: Record<string, unknown>) {
  previewRows.value = Object.entries(preview ?? {}).map(([key, value]) => {
    const v = (value ?? {}) as Record<string, number>;
    return {
      label: key,
      inBackup: Number(v.inBackup ?? v.count ?? 0),
      local: Number(v.local ?? 0),
      conflicts: Number(v.conflicts ?? v.sameId ?? 0),
    };
  });
  importCats.value = ["servers", "jumps", "keys", "terminal", "history"];
  // 旧版 ab-import-strategy：记住上次用的导入策略
  try { importStrategy.value = (localStorage.getItem("ab-import-strategy") || "skipExisting") as typeof importStrategy.value; } catch { /* 忽略 */ }
  previewOpen.value = true;
}

/** 规范化 WebDAV 地址（旧版 normalizeDavUrl） */
function normalizeDavUrl(raw: string): string {
  const url = String(raw || "").trim();
  if (!url) return "";
  if (/^https?:\/\//i.test(url)) return url;
  return "https://" + url.replace(/^\/\//, "");
}

async function saveSync(silent = false) {
  // 旧版 collectAndSaveConfig：分类空校验 / 加密口令校验 / 条件字段
  if (!syncCats.value.length && !silent) {
    showToast(t("bk.selectSyncCat"));
    throw new Error("no-categories");
  }
  const cfg: Record<string, unknown> = {
    serverUrl: normalizeDavUrl(syncUrl.value),
    username: syncUser.value.trim(),
    directory: syncDir.value.trim() || "AnyBoot",
    periodMinutes: syncPeriod.value,
    syncOnChange: syncOnChange.value,
    categories: syncCats.value,
    encrypt: syncEncrypt.value,
    enabled: syncEnabled.value,
  };
  if (syncPass.value) cfg.password = syncPass.value; // 留空 = 保持已存
  if (syncEncryptPw.value) cfg.encryptPassword = syncEncryptPw.value;
  if (syncEncrypt.value && !syncEncryptPw.value && !sync.value?.hasEncryptPassword) {
    syncEncryptErr.value = t("bk.syncNeedPw");
    throw new Error("need-encrypt-password");
  }
  await call("backup_sync_save_config", { config: cfg });
  syncPass.value = "";
  syncEncryptPw.value = "";
  await refreshSync();
  if (!silent) showToast(t("bk.configSaved"));
}

function toggleSync() {
  const on = !syncEnabled.value;
  syncEnabled.value = on;
  void (async () => {
    try {
      if (on) await saveSync(true); // 开同步 = 顺手把表单存了（旧版语义）
      await call("backup_sync_set_enabled", { enabled: on });
      showToast(on ? t("bk.syncOn") : t("bk.syncOff"));
      await refreshSync();
    } catch (error) {
      syncEnabled.value = !on;
      showToast(String(error));
    }
  })();
}

function testWebdav() {
  if (!syncUrl.value.trim()) {
    showToast(t("bk.fillServerAddr"));
    return;
  }
  void (async () => {
    try {
      await saveSync(true); // 先存再测（旧版语义）
      const result = await call<string>("backup_webdav_test", { config: {} });
      if (result === "ok") {
        showToast(t("bk.connOk"));
      } else if (result === "notFound") {
        // 远端目录不存在 → 询问是否创建（旧版 openMkdirConfirm）
        confirmBody.value = t("bk.mkdirAsk", { dir: syncDir.value.trim() || "AnyBoot" });
        confirmOpen.value = true;
        pendingMkdir.value = true;
      }
    } catch (error) {
      showToast(t("bk.testFail", { msg: String(error) }));
    }
  })();
}

/** 创建远端目录（旧版 cfmOk 的 pendingMkdir 分支） */
async function createWebdavDir() {
  confirmOpen.value = false;
  if (!pendingMkdir.value) return; // 正常覆盖导入路径
  pendingMkdir.value = false;
  try {
    await call("backup_webdav_mkdir", { config: {} });
    showToast(t("bk.dirCreated"));
  } catch (error) {
    showToast(t("bk.mkdirFail", { msg: String(error) }));
  }
}

function syncNow() {
  if (!sync.value?.isConfigured) return;
  void (async () => {
    try {
      // 旧版：toast 显示后端返回的同步结果消息
      const msg = (await call<string>("backup_sync_now")) || t("bk.syncDone");
      showToast(msg);
    } catch (error) {
      showToast(t("bk.syncFail", { msg: String(error) }));
    }
    await refreshSync(); // 成败都刷状态（last_sync_ms / last_error）
  })();
}

/** 导入三段式与弹层状态 */
const pwOpen = ref(false);
const importPw = ref("");
const importPwErr = ref("");
const pwAttempts = ref(0);
const previewOpen = ref(false);
const previewRows = ref<{ label: string; inBackup: number; local: number; conflicts: number }[]>([]);
const previewWarn = ref("");
const previewSrc = ref("");
const importPath = ref("");
const importCats = ref<string[]>(["servers", "jumps", "keys", "terminal", "history"]);
const importStrategy = ref<"skipExisting" | "overwrite">("skipExisting");
const confirmOpen = ref(false);
const pendingMkdir = ref(false);
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
async function submitPw() {
  if (!importPw.value.trim()) {
    importPwErr.value = t("bk.needPw");
    return;
  }
  try {
    const preview = (await call("backup_preview", {
      path: importPath.value,
      password: importPw.value,
    })) as Record<string, unknown>;
    fillPreview(preview);
    pwOpen.value = false;
  } catch (error) {
    pwAttempts.value += 1;
    const msg = String(error);
    importPwErr.value = msg.includes("口令错误")
      ? t("bk.pwWrong", { n: pwAttempts.value })
      : msg;
  }
}
function startImport() {
  previewOpen.value = false;
  const total = previewRows.value.reduce((s, r) => s + r.conflicts, 0);
  if (importStrategy.value === "overwrite" && total > 0) {
    confirmBody.value = t("bk.overwriteBodySimple", { n: total });
    confirmOpen.value = true;
    return;
  }
  void executeImport();
}

/** 确认弹层 OK：pendingMkdir → 创建远端目录；否则覆盖导入（旧版 cfmOk 双用途） */
function confirmOk() {
  if (pendingMkdir.value) { void createWebdavDir(); return; }
  executeImport();
}

async function executeImport() {
  confirmOpen.value = false;
  pendingMkdir.value = false;
  if (busy.value) return;
  busy.value = t("bk.importBusy");
  try {
    try { localStorage.setItem("ab-import-strategy", importStrategy.value); } catch { /* 忽略 */ }
    const res = (await call("backup_import", {
      path: importPath.value,
      password: importPw.value,
      categories: importCats.value,
      strategy: importStrategy.value,
    })) as Record<string, unknown>;
    resultRows.value = Object.entries(res ?? {}).map(([k, v]) => ({ k, v: String(v) }));
    resultOpen.value = true;
    showToast(t("bk.importDoneSimple"));
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
    <h1>{{ $t("bk.titleFull") }}</h1>
    <div class="tools">
      <WindowNavMenu class="win-nav-menu" />
      <button type="button" class="toolbtn" id="themeBtn" :aria-label="$t('keysWin.themeAria')" @click="toggleTheme">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 3a9 9 0 0 1 0 18z" fill="currentColor" stroke="none" />
        </svg>
        {{ $t("keysWin.theme") }}
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
        <h2>{{ $t("bk.exportTitle") }}</h2>
        <span class="sub">{{ $t("bk.exportSub") }}</span>
      </div>

      <div class="cats" id="exportCats">
        <label v-for="cat in CATEGORIES" :key="cat.id" class="cat-row">
          <input
            type="checkbox"
            :checked="exportCats.includes(cat.id)"
            @change="toggleCat('export', cat.id)"
          />
          <span class="cname">{{ $t(cat.name) }}</span>
          <span class="cdesc">{{ $t(cat.desc) }}</span>
        </label>
      </div>

      <div v-if="exportIncludeKeys" id="exportKeyHint" class="info">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 8v5M12 16h.01" />
        </svg>
        <span>{{ $t("bk.exportKeyWarn") }}</span>
      </div>

      <div v-if="exportIncludeKeys" id="exportPwZone" class="pw-zone">
        <div class="pw-grid">
          <div class="field">
            <label for="expPw1">{{ $t("bk.exportPw") }}</label>
            <input id="expPw1" v-model="exportPw1" type="password" :placeholder="$t('bk.pwPhMin')" autocomplete="new-password" />
            <div class="strength"><i id="expStrength" :style="{ width: exportStrengthPct + '%', background: exportStrengthColor }" /></div>
          </div>
          <div class="field">
            <label for="expPw2">{{ $t("bk.retype") }}</label>
            <input id="expPw2" v-model="exportPw2" type="password" :placeholder="$t('bk.pwPhMatch')" autocomplete="new-password" />
            <div class="err" id="expPwErr">{{ exportPwErr }}</div>
          </div>
        </div>
      </div>

      <div class="row-actions">
        <button type="button" class="btn primary" id="exportBtn" :disabled="exportDisabled" @click="doExport">
          <template v-if="busy"><span class="spin"></span>{{ $t("bk.exportingPkg") }}</template>
          <template v-else>{{ $t("bk.exportBtn") }}</template>
        </button>
        <span class="note" id="exportNote">{{ exportNoteText }}</span>
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
        <h2>{{ $t("bk.importTitle") }}</h2>
        <span class="sub">{{ $t("bk.importSub") }}</span>
      </div>
      <div class="row-actions">
        <button type="button" class="btn primary" id="importBtn" :disabled="!!busy" @click="doImport">
          <template v-if="busy"><span class="spin"></span>{{ busy }}</template>
          <template v-else>{{ $t("bk.pickFile") }}</template>
        </button>
        <span class="note">{{ $t("bk.importNote") }}</span>
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
        <h2>{{ $t("bk.syncTitle") }}</h2>
        <label class="switch" style="margin-left: auto" :title="$t('bk.syncToggleTitle')">
          <input type="checkbox" id="syncEnabled" :checked="syncEnabled" @change="toggleSync" />
          <i />
        </label>
      </div>

      <div class="form-grid" id="syncForm">
        <div class="form-row">
          <div class="field" style="flex: 2 1 280px">
            <label for="syncUrl">{{ $t("bk.serverUrl") }}</label>
            <input id="syncUrl" v-model="syncUrl" placeholder="https://dav.jianguoyun.com/dav/" autocomplete="off" />
          </div>
          <div class="field">
            <label for="syncDir">{{ $t("bk.dir") }}</label>
            <input id="syncDir" v-model="syncDir" placeholder="AnyBoot" autocomplete="off" />
          </div>
        </div>

        <div class="form-row">
          <div class="field">
            <label for="syncUser">{{ $t("bk.account") }}</label>
            <input id="syncUser" v-model="syncUser" :placeholder="$t('bk.accountPh')" autocomplete="off" />
          </div>
          <div class="field">
            <label for="syncPass">{{ $t("bk.pwdLabel") }}</label>
            <input
              id="syncPass"
              v-model="syncPass"
              type="password"
              :placeholder="sync?.hasPassword ? $t('bk.pwdPhSaved') : $t('bk.pwdPh')"
              autocomplete="new-password"
            />
          </div>
        </div>

        <div class="form-row">
          <div class="field">
            <label>{{ $t("bk.autoFreq") }}</label>
            <div class="seg" id="syncPeriod">
              <button
                v-for="opt in [
                  { v: 15, label: 'bk.min15' },
                  { v: 30, label: 'bk.min30' },
                  { v: 60, label: 'bk.min60' },
                  { v: 0, label: 'bk.manual' },
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
          {{ $t("bk.autoSyncHint") }}
        </label>

        <div class="cats" id="syncCats">
          <label v-for="cat in CATEGORIES" :key="cat.id" class="cat-row">
            <input
              type="checkbox"
              :checked="syncCats.includes(cat.id)"
              @change="toggleCat('sync', cat.id)"
            />
            <span class="cname">{{ $t(cat.name) }}</span>
            <span class="cdesc">{{ $t(cat.desc) }}</span>
          </label>
        </div>

        <!-- 云端备份加密（旧版 #syncEncryptZone） -->
        <div class="pw-zone" id="syncEncryptZone">
          <label class="checkline">
            <input id="syncEncrypt" v-model="syncEncrypt" type="checkbox" />
            {{ $t("bk.encryptCloud") }}
          </label>
          <div class="field" id="syncEncryptPwField" :hidden="!syncEncrypt" style="max-width: 320px">
            <label for="syncEncryptPw">{{ $t("bk.syncPw") }}</label>
            <input id="syncEncryptPw" v-model="syncEncryptPw" type="password" :placeholder="sync?.hasPassword ? $t('bk.pwdPhSaved') : $t('bk.syncPw')" autocomplete="new-password" />
            <span class="err" id="syncEncryptErr">{{ syncEncryptErr }}</span>
          </div>
        </div>
      </div>

      <div class="sync-status" id="syncStatus">
        <span v-if="sync?.last_error" class="err">{{ sync.last_error }}</span>
        <span v-else-if="sync?.last_sync_ms">{{ $t("bk.lastSync", { t: new Date(sync.last_sync_ms).toLocaleString() }) }}</span>
        <span v-else>{{ $t("bk.neverSynced") }}</span>
      </div>

      <div v-if="locked" class="info" id="syncLockInfo">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 8v5M12 16h.01" />
        </svg>
        <span>{{ $t("bk.lockedDuringTransfer") }}</span>
      </div>

      <div class="row-actions">
        <button type="button" class="btn" id="syncTestBtn" :disabled="!!busy" @click="testWebdav">{{ $t("bk.testConn") }}</button>
        <button type="button" class="btn" id="syncSaveBtn" :disabled="!!busy" @click="() => saveSync()">{{ $t("bk.saveCfg") }}</button>
        <button type="button" class="btn primary" id="syncNowBtn" :disabled="!syncEnabled || !!busy" @click="syncNow">
          {{ $t("bk.syncNow") }}
        </button>
      </div>
    </section>

    <p v-if="message" class="note" style="color: rgb(var(--ok))">{{ message }}</p>
    <p v-if="errorText" class="err">{{ errorText }}</p>

  <!-- 口令弹层（旧版 #pwModal） -->
  <div v-if="pwOpen" id="pwModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="pwTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="pwTitle">{{ $t("bk.pwTitle") }}</h3>
        <button type="button" id="pwClose" :aria-label="$t('common.close')" @click="pwOpen = false">✕</button>
      </div>
      <form id="pwForm" class="modal-body" @submit.prevent="submitPw">
        <div class="field">
          <label for="importPw">{{ $t("bk.pwLabel2") }}</label>
          <input id="importPw" v-model="importPw" type="password" autocomplete="off" />
          <div class="err" id="importPwErr">{{ importPwErr }}</div>
          <p class="modal-note">{{ $t("bk.pwNote") }}</p>
        </div>
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="pwCancel" @click="pwOpen = false">{{ $t("common.cancel") }}</button>
          <button type="submit" class="btn primary">{{ $t("bk.unlockPreview") }}</button>
        </div>
      </form>
    </div>
  </div>

  <!-- 导入预览（旧版 #previewModal） -->
  <div v-if="previewOpen" id="previewModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="prevTitle">
    <div class="modal-card wide">
      <div class="modal-head">
        <h3 id="prevTitle">{{ $t("bk.prevTitle") }}</h3>
        <button type="button" id="prevClose" :aria-label="$t('common.close')" @click="previewOpen = false">✕</button>
      </div>
      <div class="modal-body">
        <div class="src-line" id="prevSrc">{{ previewSrc }}</div>
        <table class="prev">
          <thead><tr><th>{{ $t("bk.thCat") }}</th><th style="text-align: right">{{ $t("bk.thInBackup") }}</th><th style="text-align: right">{{ $t("bk.thLocal") }}</th><th>{{ $t("bk.thConflict") }}</th></tr></thead>
          <tbody id="prevRows">
            <tr v-for="row in previewRows" :key="row.label">
              <td>{{ row.label }}</td>
              <td style="text-align: right">{{ row.inBackup }}</td>
              <td style="text-align: right">{{ row.local }}</td>
              <td>
                <span v-if="row.conflicts" class="chip conflict">{{ $t("bk.sameIdCount", { n: row.conflicts }) }}</span>
                <span v-else>—</span>
              </td>
            </tr>
            <tr v-if="!previewRows.length"><td colspan="4">{{ $t("bk.noRecognizable") }}</td></tr>
          </tbody>
        </table>
        <div v-if="previewWarn" id="prevWarn" class="info warn">
          <span id="prevWarnText">{{ previewWarn }}</span>
        </div>
        <div class="cats" id="importCats">
          <label v-for="cat in CATEGORIES" :key="cat.id" class="cat-row">
            <input type="checkbox" :checked="importCats.includes(cat.id)" @change="toggleCat('import', cat.id)" />
            <span class="cname">{{ $t(cat.name) }}</span>
            <span class="cdesc">{{ $t(cat.desc) }}</span>
          </label>
        </div>
        <div class="strategy" id="strategyBox">
          <label class="radio-line">
            <input v-model="importStrategy" type="radio" name="strategy" value="skipExisting" />
            <span>{{ $t("bk.skipExisting") }}<span class="rdesc">{{ $t("bk.skipExistingDesc") }}</span></span>
          </label>
          <label class="radio-line">
            <input v-model="importStrategy" type="radio" name="strategy" value="overwrite" />
            <span>{{ $t("bk.overwriteExisting") }}<span class="rdesc">{{ $t("bk.overwriteExistingDesc") }}</span></span>
          </label>
        </div>
      </div>
      <div class="modal-foot">
        <button type="button" class="btn plain" id="prevCancel" @click="previewOpen = false">{{ $t("common.cancel") }}</button>
        <button type="button" class="btn primary" id="prevGo" @click="startImport">{{ $t("bk.startImport") }}</button>
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
        <h3 id="cfmTitle">{{ $t("bk.confirmOverwrite") }}</h3>
      </div>
      <div class="modal-body">
        <p class="del-body" id="cfmBody">{{ confirmBody }}</p>
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="cfmCancel" @click="confirmOpen = false">{{ $t("common.cancel") }}</button>
          <button type="button" class="btn danger" id="cfmOk" @click="confirmOk">{{ $t("bk.doOverwrite") }}</button>
        </div>
      </div>
    </div>
  </div>

  <!-- 导入结果（旧版 #resultModal） -->
  <div v-if="resultOpen" id="resultModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="resTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="resTitle">{{ $t("bk.importDone") }}</h3>
        <button type="button" id="resClose" :aria-label="$t('common.close')" @click="resultOpen = false">✕</button>
      </div>
      <div class="modal-body" id="resBody">
        <p v-for="row in resultRows" :key="row.k">{{ row.k }}: {{ row.v }}</p>
        <p v-if="!resultRows.length">{{ $t("bk.importFinished") }}</p>
      </div>
      <div class="modal-foot">
        <button type="button" class="btn primary" id="resOk" @click="resultOpen = false">{{ $t("bk.done") }}</button>
      </div>
    </div>
  </div>

  <!-- toast（旧版 #toast） -->
  <div v-if="toastText" id="toast">{{ toastText }}</div>
  </main>
</template>
