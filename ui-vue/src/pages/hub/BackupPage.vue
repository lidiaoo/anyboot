<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { call } from "@/api/tauri";
import { useEscape } from "@/composables/useEscape";
const { t } = useI18n();

/**
 * 备份与恢复（原 `ui/hub.html` 的 `#page-backup`，3504 行 / 948 行 DOM）。
 *
 * ## 1:1 结构
 * ```
 * section#exportPanel.bk-panel  .bk-panel-head(.bk-picon + h2 + .bk-sub)
 *                                + .bk-cats + .bk-info + .bk-pw-zone
 *                                + .bk-row-actions(.bk-btn.bk-primary + .bk-note)
 * section#importPanel.bk-panel  同上（只有 .bk-row-actions）
 * hr.bk-divider
 * section#syncPanel.bk-panel    .bk-sync-head(含 .bk-switch) + .bk-form-grid
 *                                （.bk-form-row > .bk-field × N）
 *                                + .bk-checkline + .bk-cats + .bk-pw-zone
 * .bk-modal#pwModal            弹层
 * ```
 *
 * 命令族：`backup_*` 11 个。
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

/** 备份分类（旧版 .bk-cats 的复选项） */
/**
 * 备份分类（逐字照抄旧版 `CATS`，hub.html:3922）：
 * key / name / desc 三项都必须与旧版一致 —— 导出/导入的**载荷字段名**就是 key，
 * 用错 key 会让导出的备份文件与手机端 / 旧版本互不兼容。
 */
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

/** 导出：是否含私钥 → 需要口令 */
/** 是否勾了「SSH 密钥」（旧版 `hasKeys = exportChecked.has('keys')`） */
const exportIncludeKeys = computed(() => exportCats.value.includes("keys"));
const exportPw1 = ref("");
const exportPw2 = ref("");
/** 旧版 exportPasswordValid()：≥4 位且两次一致 */
const exportPasswordValid = () => exportPw1.value.length >= 4 && exportPw1.value === exportPw2.value;
/** 旧版 exportBtn.disabled = busy || (hasKeys && !valid) */
const exportDisabled = computed(() => Boolean(busy.value) || (exportIncludeKeys.value && !exportPasswordValid()));
/** 旧版 exportNote 文案 */
const exportNoteText = computed(() => {
  if (exportIncludeKeys.value) {
    return exportPasswordValid() ? t("bk.exportEncryptWithKeys") : t("bk.needPwForKeys");
  }
  return exportPw1.value ? t("bk.exportEncrypted") : t("bk.exportPlain");
});
// 旧版默认：五类全选（exportChecked = Set(["servers","jumps","keys","terminal","history"])）
const exportCats = ref<string[]>(["servers", "jumps", "keys", "terminal", "history"]);
// 旧版同步默认只勾 4 类（不含 history）：Set(["servers","jumps","keys","terminal"])
const syncCats = ref<string[]>(["servers", "jumps", "keys", "terminal"]);

const syncEnabled = ref(false);
const syncUrl = ref("");
const syncUser = ref("");
const syncPass = ref("");
const syncDir = ref("AnyBoot");
const syncPeriod = ref(15);
const syncOnChange = ref(false);
/** 云端备份加密（旧版 #syncEncrypt / #syncEncryptPw / #syncEncryptErr） */
const syncEncrypt = ref(false);
const syncEncryptPw = ref("");
const syncEncryptErr = ref("");
/** 同步是否被锁定（旧版 #syncLockInfo：导入/导出进行中） */
const locked = computed(() => Boolean(busy.value));
/** 预览来源行（旧版 #prevSrc） */
const previewSrc = computed(() => (importPath.value ? t("bk.srcPreview", { path: importPath.value }) : ""));
/** 加密备份口令弹层（旧版 #pwModal） */
const pwOpen = ref(false);
const importPw = ref("");
const importPwErr = ref("");
const pwAttempts = ref(0);

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

/** 预览结果 → 弹层（preview 与 submitPw 共用） */
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
  const totalConflicts = previewRows.value.reduce((sum, row) => sum + row.conflicts, 0);
  previewWarn.value =
    totalConflicts > 0
      ? t("bk.conflictHint", { n: totalConflicts })
      : "";
  importCats.value = ["servers", "jumps", "keys", "terminal", "history"];
  // 旧版 ab-import-strategy：记住上次用的导入策略
  try { importStrategy.value = (localStorage.getItem("ab-import-strategy") || "skipExisting") as typeof importStrategy.value; } catch { /* 忽略 */ }
  previewOpen.value = true;
}

/** 全局 toast（旧版 #bkToast） */
const bkToast = ref("");
function showToast(text: string) {
  bkToast.value = text;
  window.setTimeout(() => {
    if (bkToast.value === text) bkToast.value = "";
  }, 2600);
}

/** 导入流程（旧版三段：预览 → 覆盖确认 → 结果） */
const previewOpen = ref(false);
const previewRows = ref<{ label: string; inBackup: number; local: number; conflicts: number }[]>([]);
const previewWarn = ref("");
const importPath = ref("");
const importCats = ref<string[]>(["servers", "jumps", "keys", "terminal", "history"]);
/** 同 ID 处理策略：skipExisting（默认，安全）/ overwrite */
const importStrategy = ref<"skipExisting" | "overwrite">("skipExisting");
const confirmOpen = ref(false);
const pendingMkdir = ref(false);
const confirmBody = ref("");
const resultOpen = ref(false);
const resultRows = ref<{ k: string; v: string }[]>([]);

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
    const msg = String(error);
    if (msg.includes("__NEED_PASSWORD__")) {
      // 加密备份 → 弹口令弹层（旧版 openPwModal）
      importPwErr.value = "";
      pwOpen.value = true;
      return;
    }
    errorText.value = msg;
  }
}

onMounted(refreshSync);

/** 切换某个分类的勾选（`which` 区分导出 / 同步两套） */
/**
 * 口令强度（对齐旧版 `refreshExportUi` 的三档评分）：
 *   长度≥8 + 有数字且有字母 + 有符号 → 每满足一项加 1，满分 3
 */
const pwStrengthScore = computed(() => {
  const a = exportPw1.value;
  if (!a) return 0;
  return Math.min(
    3,
    (a.length >= 8 ? 1 : 0) +
      (/[0-9]/.test(a) && /[a-zA-Z]/.test(a) ? 1 : 0) +
      (/[^0-9a-zA-Z]/.test(a) ? 1 : 0),
  );
});
const pwStrengthPercent = computed(() => ((pwStrengthScore.value / 3) * 100).toFixed(0));
const pwStrengthColor = computed(() => {
  const s = pwStrengthScore.value;
  return s <= 1 ? "rgb(var(--bad))" : s === 2 ? "rgb(var(--warn))" : "rgb(var(--ok))";
});

/** 口令校验文案（对齐旧版：至少 4 位 / 两次不一致） */
const pwError = computed(() => {
  const a = exportPw1.value;
  const b = exportPw2.value;
  if (a && a.length < 4) return t("bk.pwMin");
  if (b && a !== b) return t("bk.pwMismatch");
  return "";
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

async function doExport() {
  if (!exportCats.value.length) {
    showToast(t("bk.selectOneCat"));
    return;
  }
  if (exportPw1.value !== exportPw2.value) {
    errorText.value = t("bk.pwMismatch");
    return;
  }
  if (exportIncludeKeys.value && (exportPw1.value.length < 4 || exportPw1.value !== exportPw2.value)) {
    showToast(t("bk.pwNotSet"));
    return;
  }
  // 旧版流程：先挑保存路径（backup_pick_export_path），用户取消就不导出
  let path = "";
  try {
    path = ((await call("backup_pick_export_path")) as string) ?? "";
  } catch {
    path = "";
  }
  if (!path) return;
  // 旧版：有口令才加密导出（null = 不加密）；成功 toast = 后端消息 + 路径
  const usePw = Boolean(exportPw1.value);
  const msg = (await call<string>("backup_export", {
    path,
    categories: exportIncludeKeys.value
      ? [...exportCats.value, "keys"]
      : exportCats.value,
    password: usePw ? exportPw1.value : null,
  })) || "";
  showToast(msg ? t("bk.exportedMsg", { msg, path }) : t("bk.exported", { path }));
}

/**
 * 导入：**旧版是三段式**（hub.html:4126 起）
 *   ① 选文件 → backup_pick_import_file
 *   ② 取差异 → backup_preview，弹出「导入预览」弹层
 *      （分类勾选 + 同 ID 策略单选 + 冲突警告）
 *   ③ 点「开始导入」→ 若策略是 overwrite 且存在冲突，再弹「确认覆盖」→ 才执行
 */
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
    const preview = (await call("backup_preview", { path, password: importPw.value || null })) as Record<string, unknown>;
    previewRows.value = Object.entries(preview ?? {}).map(([key, value]) => {
      const v = (value ?? {}) as Record<string, number>;
      return {
        label: key,
        inBackup: Number(v.inBackup ?? v.count ?? 0),
        local: Number(v.local ?? 0),
        conflicts: Number(v.conflicts ?? v.sameId ?? 0),
      };
    });
    const totalConflicts = previewRows.value.reduce((sum, row) => sum + row.conflicts, 0);
    previewWarn.value =
      totalConflicts > 0
        ? t("bk.conflictHint", { n: totalConflicts })
        : "";
    importCats.value = ["servers", "jumps", "keys", "terminal", "history"];
    importStrategy.value = "skipExisting";
    previewOpen.value = true;
  } catch (error) {
    errorText.value = String(error);
  }
}

/** 预览弹层里点「开始导入」 */
function startImport() {
  previewOpen.value = false;
  const totalConflicts = previewRows.value.reduce((sum, row) => sum + row.conflicts, 0);
  if (importStrategy.value === "overwrite" && totalConflicts > 0) {
    confirmBody.value = t("bk.overwriteBody", { n: totalConflicts });
    confirmOpen.value = true;
    return;
  }
  void executeImport();
}

/** 覆盖确认后真正执行 */

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
  message.value = "";
  errorText.value = "";
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
  } catch (error) {
    errorText.value = String(error);
  } finally {
    busy.value = "";
  }
}

/** 规范化 WebDAV 地址：没写协议自动补 https://（旧版 normalizeDavUrl） */
function normalizeDavUrl(raw: string): string {
  const url = String(raw || "").trim();
  if (!url) return "";
  if (/^https?:\/\//i.test(url)) return url;
  return "https://" + url.replace(/^\/\//, "");
}

async function saveSync(silent = false) {
  if (!syncUrl.value.trim()) {
    showToast(t("bk.fillServerAddr"));
    return;
  }
  // 旧版 collectAndSaveConfig：分类空校验 / 加密口令校验 / 条件字段
  if (!syncCats.value.length && !silent) {
    showToast(t("bk.selectSyncCat"));
    throw new Error("no-categories");
  }
  const cfg: Record<string, unknown> = {
    // 字段名必须与旧版/Rust 读取键一致（serverUrl/directory/syncOnChange）
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
useEscape(pwOpen, () => { pwOpen.value = false });
useEscape(previewOpen, () => { previewOpen.value = false });
useEscape(confirmOpen, () => { confirmOpen.value = false });
useEscape(resultOpen, () => { resultOpen.value = false });
</script>

<template>
  <div class="hub-page">
    <div class="page-head"><h2>{{ t("bk.title") }}</h2></div>
    <div class="hub-page-body">
      <div class="bk-bk-scroll bk-bk-scroll-thin">
      <!-- ===== 导出 ===== -->
      <section class="bk-panel" id="exportPanel">
        <div class="bk-panel-head">
          <span class="bk-picon" aria-hidden="true">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 15V3M7 8l5-5 5 5M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
            </svg>
          </span>
          <h2>{{ t("bk.exportTitle") }}</h2>
          <span class="bk-sub">{{ t("bk.exportSub") }}</span>
        </div>

        <div class="bk-cats" id="exportCats">
          <label v-for="cat in CATEGORIES" :key="cat.id" class="bk-cat-row">
            <input
              type="checkbox"
              :checked="exportCats.includes(cat.id)"
              @change="toggleCat('export', cat.id)"
            />
            <span class="bk-cname">{{ t(cat.name) }}</span>
            <span class="bk-cdesc">{{ t(cat.desc) }}</span>
          </label>
        </div>

        <div v-if="exportIncludeKeys" class="bk-info" id="exportKeyHint">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <circle cx="12" cy="12" r="9" />
            <path d="M12 8v5M12 16h.01" />
          </svg>
          <span>{{ t("bk.exportKeyWarn") }}</span>
        </div>

        <div v-if="exportIncludeKeys || exportPw1 || exportPw2" class="bk-pw-zone" id="exportPwZone">
          <div class="bk-pw-grid" id="exportPwGrid">
            <div class="bk-field">
              <label for="expPw1">{{ t("bk.exportPw") }}</label>
              <input id="expPw1" v-model="exportPw1" type="password" :placeholder="t('bk.pwPhMin')" autocomplete="new-password" />
              <div class="bk-strength"><i id="expStrength" :style="{ width: pwStrengthPercent + '%', background: pwStrengthColor }" /></div>
            </div>
            <div class="bk-field">
              <label for="expPw2">{{ t("bk.retype") }}</label>
              <input id="expPw2" v-model="exportPw2" type="password" :placeholder="t('bk.pwPhMatch')" autocomplete="new-password" />
              <div class="bk-err" id="expPwErr">{{ pwError }}</div>
            </div>
          </div>
        </div>

        <div class="bk-row-actions">
          <button type="button" id="exportBtn" class="bk-btn bk-primary" :disabled="exportDisabled" @click="doExport">
            {{ t("bk.exportBtn") }}
          </button>
          <span class="bk-note" id="exportNote">{{ exportNoteText }}</span>
        </div>
      </section>

      <!-- ===== 导入 ===== -->
      <section class="bk-panel" id="importPanel">
        <div class="bk-panel-head">
          <span class="bk-picon" aria-hidden="true">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3v12M8 7l4-4 4 4M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
            </svg>
          </span>
          <h2>{{ t("bk.importTitle") }}</h2>
          <span class="bk-sub">{{ t("bk.importSub") }}</span>
        </div>
        <div class="bk-row-actions">
          <button type="button" id="importBtn" class="bk-btn bk-primary" :disabled="!!busy" @click="doImport">
            {{ t("bk.pickFile") }}
          </button>
          <span class="bk-note">{{ t("bk.importNote") }}</span>
        </div>
      </section>

      <hr class="bk-divider" style="max-width: 860px; width: 100%; margin: 0 auto" />

      <!-- ===== WebDAV 同步 ===== -->
      <section class="bk-panel" id="syncPanel">
        <div class="bk-sync-head">
          <span class="bk-picon" aria-hidden="true">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M20 7h-9l-2-2H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2z" />
            </svg>
          </span>
          <h2>{{ t("bk.syncTitle") }}</h2>
          <label class="bk-switch" style="margin-left: auto" :title="t('bk.syncToggleTitle')">
            <input type="checkbox" id="syncEnabled" :checked="syncEnabled" @change="toggleSync" />
            <i />
          </label>
        </div>

        <div class="bk-form-grid" id="syncForm">
          <div class="bk-form-row">
            <div class="bk-field" style="flex: 2 1 280px">
              <label for="syncUrl">{{ t("bk.serverUrl") }}</label>
              <input id="syncUrl" v-model="syncUrl" placeholder="https://dav.jianguoyun.com/dav/" autocomplete="off" />
            </div>
            <div class="bk-field">
              <label for="syncDir">{{ t("bk.dir") }}</label>
              <input id="syncDir" v-model="syncDir" placeholder="AnyBoot" autocomplete="off" />
            </div>
          </div>

          <div class="bk-form-row">
            <div class="bk-field">
              <label for="syncUser">{{ t("bk.account") }}</label>
              <input id="syncUser" v-model="syncUser" :placeholder="t('bk.accountPh')" autocomplete="off" />
            </div>
            <div class="bk-field">
              <label for="syncPass">{{ t("bk.pwdLabel") }}</label>
              <input
                id="syncPass"
                v-model="syncPass"
                type="password"
                :placeholder="sync?.hasPassword ? t('bk.pwdPhSaved') : t('bk.pwdPh')"
                autocomplete="new-password"
              />
            </div>
          </div>

          <div class="bk-form-row">
            <div class="bk-field">
              <label>{{ t("bk.autoFreq") }}</label>
              <div class="bk-seg" id="syncPeriod">
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
                  {{ t(opt.label) }}
                </button>
              </div>
            </div>
          </div>

          <label class="bk-checkline">
            <input id="syncOnChange" v-model="syncOnChange" type="checkbox" />
            {{ t("bk.autoSyncHint") }}
          </label>

          <!-- 云端备份加密（旧版 #syncEncryptZone） -->
          <div class="bk-pw-zone" id="syncEncryptZone">
            <label class="bk-checkline">
              <input id="syncEncrypt" v-model="syncEncrypt" type="checkbox" />
              {{ t("bk.encryptCloud") }}
            </label>
            <div id="syncEncryptPwField" class="bk-field" :hidden="!syncEncrypt" style="max-width: 320px">
              <label for="syncEncryptPw">{{ t("bk.syncPw") }}</label>
              <input
                id="syncEncryptPw"
                v-model="syncEncryptPw"
                type="password"
                :placeholder="sync?.hasPassword ? t('bk.pwdPhSaved') : t('bk.syncPw')"
                autocomplete="new-password"
              />
              <span id="syncEncryptErr" class="bk-err">{{ syncEncryptErr }}</span>
            </div>
          </div>

          <div class="bk-cats" id="syncCats">
            <label v-for="cat in CATEGORIES" :key="cat.id" class="bk-radio-line">
              <input
                type="checkbox"
                :checked="syncCats.includes(cat.id)"
                @change="toggleCat('sync', cat.id)"
              />
              <span class="bk-cname">{{ t(cat.name) }}</span>
              <span class="bk-cdesc">{{ t(cat.desc) }}</span>
            </label>
          </div>
        </div>

        <div class="bk-sync-status" id="syncStatus">
          <span v-if="sync?.last_error" class="bk-err">{{ sync.last_error }}</span>
          <span v-else-if="sync?.last_sync_ms">{{ t("bk.lastSync", { t: new Date(sync.last_sync_ms).toLocaleString() }) }}</span>
            <span v-else>{{ t("bk.neverSynced") }}</span>
        </div>
        <div v-if="locked" id="syncLockInfo" class="bk-info">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <circle cx="12" cy="12" r="9" />
            <path d="M12 8v5M12 16h.01" />
          </svg>
          <span>{{ t("bk.lockedDuringTransfer") }}</span>
        </div>

        <div class="bk-row-actions">
          <button type="button" id="syncTestBtn" class="bk-btn" :disabled="!!busy" @click="testWebdav">{{ t("bk.testConn") }}</button>
          <button type="button" id="syncSaveBtn" class="bk-btn" :disabled="!!busy" @click="() => saveSync()">{{ t("bk.saveCfg") }}</button>
          <button type="button" id="syncNowBtn" class="bk-btn bk-primary" :disabled="!syncEnabled || !!busy" @click="syncNow">
            {{ t("bk.syncNow") }}
          </button>
        </div>
      </section>
      </div><!-- /bk-bk-scroll -->

      <!-- 加密备份口令弹层（旧版 #pwModal） -->
      <div v-if="pwOpen" id="pwModal" class="bk-modal" role="dialog" aria-modal="true" aria-labelledby="pwTitle">
        <div class="bk-modal-card">
          <div class="bk-modal-head">
            <h3 id="pwTitle">{{ t("bk.pwTitle") }}</h3>
            <button type="button" id="pwClose" :aria-label="t('common.close')" @click="pwOpen = false">✕</button>
          </div>
          <form id="pwForm" class="bk-modal-body" @submit.prevent="submitPw">
            <div class="bk-field">
              <label for="importPw">{{ t("bk.pwLabel2") }}</label>
              <input id="importPw" v-model="importPw" type="password" autocomplete="off" />
              <div id="importPwErr" class="bk-err">{{ importPwErr }}</div>
            </div>
            <p class="bk-modal-note">{{ t("bk.pwNote") }}</p>
            <div class="bk-modal-foot" style="border: 0; padding: 4px 0 0">
              <button type="button" id="pwCancel" class="bk-btn bk-plain" @click="pwOpen = false">{{ t("common.cancel") }}</button>
              <button type="submit" class="bk-btn bk-primary">{{ t("bk.unlockPreview") }}</button>
            </div>
          </form>
        </div>
      </div>

      <!-- ① 导入预览（旧版 #previewModal） -->
      <div v-if="previewOpen" id="previewModal" class="bk-modal" role="dialog" aria-modal="true" aria-labelledby="prevTitle">
        <div class="bk-modal-card bk-wide">
          <div class="bk-modal-head">
            <h3 id="prevTitle">{{ t("bk.prevTitle") }}</h3>
            <button type="button" id="prevClose" :aria-label="t('common.close')" @click="previewOpen = false">✕</button>
          </div>
          <div class="bk-modal-body">
            <div class="bk-src-line" id="prevSrc">{{ previewSrc }}</div>
            <table class="bk-prev">
              <thead>
                <tr>
                  <th>{{ t("bk.thCat") }}</th>
                  <th style="text-align: right">{{ t("bk.thInBackup") }}</th>
                  <th style="text-align: right">{{ t("bk.thLocal") }}</th>
                  <th>{{ t("bk.thConflict") }}</th>
                </tr>
              </thead>
              <tbody id="prevRows">
                <tr v-for="row in previewRows" :key="row.label">
                  <td>{{ t(row.label) }}</td>
                  <td class="bk-num" style="text-align: right">{{ row.inBackup }}</td>
                  <td class="bk-num" style="text-align: right">{{ row.local }}</td>
                  <td>
                    <span v-if="row.conflicts" class="bk-chip bk-conflict">{{ t("bk.sameIdCount", { n: row.conflicts }) }}</span>
                    <span v-else class="bk-chip">—</span>
                  </td>
                </tr>
                <tr v-if="!previewRows.length">
                  <td colspan="4" class="bk-note">{{ t("bk.noRecognizable") }}</td>
                </tr>
              </tbody>
            </table>

            <div v-if="previewWarn" id="prevWarn" class="bk-info bk-warn">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
                <path d="M12 9v4M12 17h.01M10.3 3.9L1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
              </svg>
              <span id="prevWarnText">{{ previewWarn }}</span>
            </div>

            <div>
              <label style="display: block; font-size: 12px; font-weight: 500; color: rgb(var(--fg-dim)); margin-bottom: 6px">{{ t("bk.importCats") }}</label>
              <div class="bk-cats" id="importCats">
                <label v-for="cat in CATEGORIES" :key="cat.id" class="bk-cat-row">
                  <input
                    type="checkbox"
                    :checked="importCats.includes(cat.id)"
                    @change="toggleCat('import', cat.id)"
                  />
                  <span class="bk-cname">{{ t(cat.name) }}</span>
                  <span class="bk-cdesc">{{ t(cat.desc) }}</span>
                </label>
              </div>
            </div>

            <div>
              <label style="display: block; font-size: 12px; font-weight: 500; color: rgb(var(--fg-dim)); margin-bottom: 6px">{{ t("bk.sameIdHandling") }}</label>
              <div class="bk-strategy" id="strategyBox">
                <label class="bk-radio-line">
                  <input v-model="importStrategy" type="radio" name="strategy" value="skipExisting" />
                  <span>{{ t("bk.skipExisting") }}<span class="bk-rdesc">{{ t("bk.skipExistingDesc") }}</span></span>
                </label>
                <label class="bk-radio-line">
                  <input v-model="importStrategy" type="radio" name="strategy" value="overwrite" />
                  <span>{{ t("bk.overwriteExisting") }}<span class="bk-rdesc">{{ t("bk.overwriteExistingDesc") }}</span></span>
                </label>
              </div>
            </div>
          </div>
          <div class="bk-modal-foot">
            <button type="button" id="prevCancel" class="bk-btn bk-plain" @click="previewOpen = false">{{ t("common.cancel") }}</button>
            <button type="button" id="prevGo" class="bk-btn bk-primary" @click="startImport">{{ t("bk.startImport") }}</button>
          </div>
        </div>
      </div>

      <!-- ② 覆盖确认（旧版 #confirmModal） -->
      <div v-if="confirmOpen" id="confirmModal" class="bk-modal" role="dialog" aria-modal="true" aria-labelledby="cfmTitle">
        <div class="bk-modal-card">
          <div class="bk-del-hero">
            <span class="bk-icon" aria-hidden="true">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 9v4M12 17h.01M10.3 3.9L1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
              </svg>
            </span>
            <h3 id="cfmTitle">{{ t("bk.confirmOverwrite") }}</h3>
          </div>
          <p id="cfmBody" class="bk-del-body">{{ confirmBody }}</p>
          <div class="bk-modal-foot">
            <button type="button" id="cfmCancel" class="bk-btn bk-plain" @click="confirmOpen = false">{{ t("common.cancel") }}</button>
            <button type="button" id="cfmOk" class="bk-btn bk-danger" @click="confirmOk">{{ t("bk.doOverwrite") }}</button>
          </div>
        </div>
      </div>

      <!-- ③ 导入结果（旧版 #resultModal） -->
      <div v-if="resultOpen" id="resultModal" class="bk-modal" role="dialog" aria-modal="true" aria-labelledby="resTitle">
        <div class="bk-modal-card">
          <div class="bk-modal-head">
            <h3 id="resTitle">{{ t("bk.importDone") }}</h3>
            <button type="button" id="resClose" :aria-label="t('common.close')" @click="resultOpen = false">✕</button>
          </div>
          <div id="resBody" class="bk-modal-body">
            <p v-for="row in resultRows" :key="row.k">{{ row.k }}: {{ row.v }}</p>
            <p v-if="!resultRows.length">{{ t("bk.importFinished") }}</p>
          </div>
          <div class="bk-modal-foot">
            <button type="button" id="resOk" class="bk-btn bk-primary" @click="resultOpen = false">{{ t("bk.done") }}</button>
          </div>
        </div>
      </div>

      <!-- 页脚（旧版 .bk-bk-foot） -->
      <div class="bk-bk-foot">
        <span>{{ t("bk.portabilityNote1") }}</span>
        <span>{{ t("bk.portabilityNote2") }}</span>
      </div>

      <!-- toast（旧版 #bkToast） -->
      <div v-if="bkToast" id="bkToast" class="toast" role="status">{{ bkToast }}</div>

      <p v-if="message" class="bk-note" style="color: rgb(var(--ok))">{{ message }}</p>
      <p v-if="errorText" class="bk-err">{{ errorText }}</p>
    </div>
  </div>
</template>
