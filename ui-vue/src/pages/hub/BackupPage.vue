<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { call } from "@/api/tauri";
import { useEscape } from "@/composables/useEscape";

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
  url: string;
  username: string;
  hasPassword: boolean;
  remoteDir: string;
  lastSyncAt: string;
  lastError: string;
  periodMinutes?: number;
  onChanged?: boolean;
}

/** 备份分类（旧版 .bk-cats 的复选项） */
/**
 * 备份分类（逐字照抄旧版 `CATS`，hub.html:3922）：
 * key / name / desc 三项都必须与旧版一致 —— 导出/导入的**载荷字段名**就是 key，
 * 用错 key 会让导出的备份文件与手机端 / 旧版本互不兼容。
 */
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

/** 导出：是否含私钥 → 需要口令 */
/** 是否勾了「SSH 密钥」（旧版 `hasKeys = exportChecked.has('keys')`） */
const exportIncludeKeys = computed(() => exportCats.value.includes("keys"));
const exportPw1 = ref("");
const exportPw2 = ref("");
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
const previewSrc = computed(() => (importPath.value ? `来源：${importPath.value}` : ""));
/** 加密备份口令弹层（旧版 #pwModal） */
const pwOpen = ref(false);
const importPw = ref("");
const importPwErr = ref("");

function submitPw() {
  if (!importPw.value.trim()) {
    importPwErr.value = "请输入口令";
    return;
  }
  pwOpen.value = false;
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
const confirmBody = ref("");
const resultOpen = ref(false);
const resultRows = ref<{ k: string; v: string }[]>([]);

async function refreshSync() {
  try {
    sync.value = await call<SyncState>("backup_sync_get_state");
    syncEnabled.value = Boolean(sync.value?.enabled);
    syncUrl.value = sync.value?.url ?? "";
    syncUser.value = sync.value?.username ?? "";
    syncDir.value = sync.value?.remoteDir || "AnyBoot";
    syncPeriod.value = sync.value?.periodMinutes ?? 15;
    syncOnChange.value = Boolean(sync.value?.onChanged);
  } catch (error) {
    errorText.value = String(error);
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
  if (a && a.length < 4) return "口令至少 4 位";
  if (b && a !== b) return "两次输入不一致";
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

async function run(label: string, action: () => Promise<unknown>, done: string) {
  if (busy.value) return;
  busy.value = label;
  message.value = "";
  errorText.value = "";
  try {
    await action();
    message.value = done;
    showToast(done);
    await refreshSync();
  } catch (error) {
    errorText.value = String(error);
    showToast(label + "失败：" + error);
  } finally {
    busy.value = "";
  }
}

async function doExport() {
  if (!exportCats.value.length) {
    showToast("至少选择一个分类");
    return;
  }
  if (exportPw1.value !== exportPw2.value) {
    errorText.value = "两次口令不一致";
    return;
  }
  if (exportIncludeKeys.value && (exportPw1.value.length < 4 || exportPw1.value !== exportPw2.value)) {
    showToast("请先把口令设置好（≥4 位，两次一致）");
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
  await run(
    "导出",
    () =>
      call("backup_export", {
        path,
        categories: exportIncludeKeys.value
          ? [...exportCats.value, "keys"]
          : exportCats.value,
        password: exportPw1.value || "",
      }),
    "备份文件已导出",
  );
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
    const preview = (await call("backup_preview", { path, password: "" })) as Record<string, unknown>;
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
        ? `备份中有 ${totalConflicts} 条与本机同 ID 的条目 —— 选「跳过已存在」保持不变，选「覆盖」则替换。`
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
    confirmBody.value = `备份中有 ${totalConflicts} 条与本机同 ID 的条目，将以备份内容整条覆盖。本机独有的条目保留。`;
    confirmOpen.value = true;
    return;
  }
  void executeImport();
}

/** 覆盖确认后真正执行 */
async function executeImport() {
  confirmOpen.value = false;
  if (busy.value) return;
  busy.value = "导入";
  message.value = "";
  errorText.value = "";
  try {
    const res = (await call("backup_import", {
      path: importPath.value,
      password: "",
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

async function saveSync() {
  if (!syncUrl.value.trim()) {
    showToast("请先填写服务器地址并保存配置");
    return;
  }
  await run(
    "保存同步配置",
    () =>
      call("backup_sync_save_config", {
        config: {
          // 字段名必须与旧版/Rust 读取键一致（serverUrl/directory/syncOnChange）
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
  if (!syncUrl.value.trim()) {
    showToast("请先填写服务器地址并保存配置");
    return;
  }
  void run("测试 WebDAV", () => call("backup_webdav_test", { config: {} }), "连接成功 ✓");
}

function createWebdavDir() {
  void run(
    "创建远端目录",
    () => call("backup_webdav_mkdir", { config: { remoteDir: syncDir.value.trim() || "AnyBoot" } }),
    "远端目录已创建",
  );
}

function syncNow() {
  if (!syncUrl.value.trim()) {
    showToast("请先填写服务器地址并保存配置");
    return;
  }
  if (!syncCats.value.length) {
    showToast("至少选择一个同步分类");
    return;
  }
  void run("立即同步", () => call("backup_sync_now"), "同步完成");
}
useEscape(pwOpen, () => { pwOpen.value = false });
useEscape(previewOpen, () => { previewOpen.value = false });
useEscape(confirmOpen, () => { confirmOpen.value = false });
useEscape(resultOpen, () => { resultOpen.value = false });
</script>

<template>
  <div class="bk-bk-scroll bk-bk-scroll-thin">
  <!-- ===== 导出 ===== -->
  <section class="bk-panel" id="exportPanel">
    <div class="bk-panel-head">
      <span class="bk-picon" aria-hidden="true">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 15V3M7 8l5-5 5 5M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
        </svg>
      </span>
      <h2>导出备份</h2>
      <span class="bk-sub">导出为 .json 备份文件，可在手机 / 电脑间互导</span>
    </div>

    <div class="bk-cats" id="exportCats">
      <label v-for="cat in CATEGORIES" :key="cat.id" class="bk-cat-row">
        <input
          type="checkbox"
          :checked="exportCats.includes(cat.id)"
          @change="toggleCat('export', cat.id)"
        />
        <span class="bk-cname">{{ cat.name }}</span>
        <span class="bk-cdesc">{{ cat.desc }}</span>
      </label>
    </div>

    <div v-if="exportIncludeKeys" class="bk-info" id="exportKeyHint">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <circle cx="12" cy="12" r="9" />
        <path d="M12 8v5M12 16h.01" />
      </svg>
      <span>导出包含 <b>SSH 私钥明文</b>，请设置口令加密，并妥善保管备份文件。</span>
    </div>

    <div v-if="exportIncludeKeys || exportPw1 || exportPw2" class="bk-pw-zone" id="exportPwZone">
      <div class="bk-pw-grid" id="exportPwGrid">
        <div class="bk-field">
          <label for="expPw1">备份口令</label>
          <input id="expPw1" v-model="exportPw1" type="password" placeholder="至少 4 位" autocomplete="new-password" />
          <div class="bk-strength"><i id="expStrength" :style="{ width: pwStrengthPercent + '%', background: pwStrengthColor }" /></div>
        </div>
        <div class="bk-field">
          <label for="expPw2">再输一次</label>
          <input id="expPw2" v-model="exportPw2" type="password" placeholder="两次需一致" autocomplete="new-password" />
          <div class="bk-err" id="expPwErr">{{ pwError }}</div>
        </div>
      </div>
    </div>

    <div class="bk-row-actions">
      <button type="button" id="exportBtn" class="bk-btn bk-primary" :disabled="!!busy" @click="doExport">
        导出备份文件
      </button>
      <span class="bk-note" id="exportNote">不加密也能导出，但密钥明文会直接写进文件</span>
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
      <h2>导入备份</h2>
      <span class="bk-sub">从 .json 备份文件恢复（手机导出的备份同样可用）</span>
    </div>
    <div class="bk-row-actions">
      <button type="button" id="importBtn" class="bk-btn bk-primary" :disabled="!!busy" @click="doImport">
        选择备份文件…
      </button>
      <span class="bk-note">导入前会先预览差异，再让你确认</span>
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
      <h2>WebDAV 云同步</h2>
      <label class="bk-switch" style="margin-left: auto" title="开启 / 关闭同步">
        <input type="checkbox" id="syncEnabled" :checked="syncEnabled" @change="toggleSync" />
        <i />
      </label>
    </div>

    <div class="bk-form-grid" id="syncForm">
      <div class="bk-form-row">
        <div class="bk-field" style="flex: 2 1 280px">
          <label for="syncUrl">服务器地址</label>
          <input id="syncUrl" v-model="syncUrl" placeholder="https://dav.jianguoyun.com/dav/" autocomplete="off" />
        </div>
        <div class="bk-field">
          <label for="syncDir">目录</label>
          <input id="syncDir" v-model="syncDir" placeholder="AnyBoot" autocomplete="off" />
        </div>
      </div>

      <div class="bk-form-row">
        <div class="bk-field">
          <label for="syncUser">账号</label>
          <input id="syncUser" v-model="syncUser" placeholder="WebDAV 账号" autocomplete="off" />
        </div>
        <div class="bk-field">
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

      <div class="bk-form-row">
        <div class="bk-field">
          <label>自动同步频率</label>
          <div class="bk-seg" id="syncPeriod">
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

      <label class="bk-checkline">
        <input id="syncOnChange" v-model="syncOnChange" type="checkbox" />
        有变化时自动同步（改动后约 30 秒内推送）
      </label>

      <!-- 云端备份加密（旧版 #syncEncryptZone） -->
      <div class="bk-pw-zone" id="syncEncryptZone">
        <label class="bk-checkline">
          <input id="syncEncrypt" v-model="syncEncrypt" type="checkbox" />
          云端备份加密（推荐 —— 同步文件含密钥明文）
        </label>
        <div id="syncEncryptPwField" class="bk-field" :hidden="!syncEncrypt" style="max-width: 320px">
          <label for="syncEncryptPw">同步口令</label>
          <input
            id="syncEncryptPw"
            v-model="syncEncryptPw"
            type="password"
            :placeholder="sync?.hasPassword ? '已保存，留空则不修改' : '同步口令'"
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
          <span class="bk-cname">{{ cat.name }}</span>
          <span class="bk-cdesc">{{ cat.desc }}</span>
        </label>
      </div>
    </div>

    <div class="bk-sync-status" id="syncStatus">
      <span v-if="sync?.lastError" class="bk-err">{{ sync.lastError }}</span>
      <span v-else>上次同步：{{ sync?.lastSyncAt || "从未" }}</span>
    </div>
    <div v-if="locked" id="syncLockInfo" class="bk-info">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <circle cx="12" cy="12" r="9" />
        <path d="M12 8v5M12 16h.01" />
      </svg>
      <span>导入 / 导出进行中，同步暂时锁定。</span>
    </div>

    <div class="bk-row-actions">
      <button type="button" id="syncTestBtn" class="bk-btn" :disabled="!!busy" @click="testWebdav">测试连接</button>
      <button type="button" id="syncMkdirBtn" class="bk-btn" :disabled="!!busy" @click="createWebdavDir">创建远端目录</button>
      <button type="button" id="syncSaveBtn" class="bk-btn" :disabled="!!busy" @click="saveSync">保存配置</button>
      <button type="button" id="syncNowBtn" class="bk-btn bk-primary" :disabled="!syncEnabled || !!busy" @click="syncNow">
        立即同步
      </button>
    </div>
  </section>
  </div><!-- /bk-bk-scroll -->

  <!-- 加密备份口令弹层（旧版 #pwModal） -->
  <div v-if="pwOpen" id="pwModal" class="bk-modal" role="dialog" aria-modal="true" aria-labelledby="pwTitle">
    <div class="bk-modal-card">
      <div class="bk-modal-head">
        <h3 id="pwTitle">此备份已加密</h3>
        <button type="button" id="pwClose" aria-label="关闭" @click="pwOpen = false">✕</button>
      </div>
      <form id="pwForm" class="bk-modal-body" @submit.prevent="submitPw">
        <div class="bk-field">
          <label for="importPw">备份口令</label>
          <input id="importPw" v-model="importPw" type="password" autocomplete="off" />
          <div id="importPwErr" class="bk-err">{{ importPwErr }}</div>
        </div>
        <p class="bk-modal-note">口令错误 3 次后提示：口令由导出方设置，本机无法找回。</p>
        <div class="bk-modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" id="pwCancel" class="bk-btn bk-plain" @click="pwOpen = false">取消</button>
          <button type="submit" class="bk-btn bk-primary">解锁预览</button>
        </div>
      </form>
    </div>
  </div>

  <!-- ① 导入预览（旧版 #previewModal） -->
  <div v-if="previewOpen" id="previewModal" class="bk-modal" role="dialog" aria-modal="true" aria-labelledby="prevTitle">
    <div class="bk-modal-card bk-wide">
      <div class="bk-modal-head">
        <h3 id="prevTitle">导入预览</h3>
        <button type="button" id="prevClose" aria-label="关闭" @click="previewOpen = false">✕</button>
      </div>
      <div class="bk-modal-body">
        <div class="bk-src-line" id="prevSrc">{{ previewSrc }}</div>
        <table class="bk-prev">
          <thead>
            <tr>
              <th>分类</th>
              <th style="text-align: right">备份内</th>
              <th style="text-align: right">本机现有</th>
              <th>同 ID 冲突</th>
            </tr>
          </thead>
          <tbody id="prevRows">
            <tr v-for="row in previewRows" :key="row.label">
              <td>{{ row.label }}</td>
              <td class="bk-num" style="text-align: right">{{ row.inBackup }}</td>
              <td class="bk-num" style="text-align: right">{{ row.local }}</td>
              <td>
                <span v-if="row.conflicts" class="bk-chip bk-conflict">{{ row.conflicts }} 个同 ID</span>
                <span v-else class="bk-chip">—</span>
              </td>
            </tr>
            <tr v-if="!previewRows.length">
              <td colspan="4" class="bk-note">（备份文件里没有可识别的内容）</td>
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
          <label style="display: block; font-size: 12px; font-weight: 500; color: rgb(var(--fg-dim)); margin-bottom: 6px">要导入的分类</label>
          <div class="bk-cats" id="importCats">
            <label v-for="cat in CATEGORIES" :key="cat.id" class="bk-cat-row">
              <input
                type="checkbox"
                :checked="importCats.includes(cat.id)"
                @change="toggleCat('import', cat.id)"
              />
              <span class="bk-cname">{{ cat.name }}</span>
              <span class="bk-cdesc">{{ cat.desc }}</span>
            </label>
          </div>
        </div>

        <div>
          <label style="display: block; font-size: 12px; font-weight: 500; color: rgb(var(--fg-dim)); margin-bottom: 6px">同 ID 条目处理</label>
          <div class="bk-strategy" id="strategyBox">
            <label class="bk-radio-line">
              <input v-model="importStrategy" type="radio" name="strategy" value="skipExisting" />
              <span>跳过已存在的条目<span class="bk-rdesc">只添加本机没有的；现有配置一律不动（安全）</span></span>
            </label>
            <label class="bk-radio-line">
              <input v-model="importStrategy" type="radio" name="strategy" value="overwrite" />
              <span>用备份覆盖同 ID 条目<span class="bk-rdesc">同 ID 的本机档案会被备份内容整条替换；本机独有的条目保留</span></span>
            </label>
          </div>
        </div>
      </div>
      <div class="bk-modal-foot">
        <button type="button" id="prevCancel" class="bk-btn bk-plain" @click="previewOpen = false">取消</button>
        <button type="button" id="prevGo" class="bk-btn bk-primary" @click="startImport">开始导入</button>
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
        <h3 id="cfmTitle">确认覆盖导入？</h3>
      </div>
      <p id="cfmBody" class="bk-del-body">{{ confirmBody }}</p>
      <div class="bk-modal-foot">
        <button type="button" id="cfmCancel" class="bk-btn bk-plain" @click="confirmOpen = false">取消</button>
        <button type="button" id="cfmOk" class="bk-btn bk-danger" @click="executeImport">覆盖导入</button>
      </div>
    </div>
  </div>

  <!-- ③ 导入结果（旧版 #resultModal） -->
  <div v-if="resultOpen" id="resultModal" class="bk-modal" role="dialog" aria-modal="true" aria-labelledby="resTitle">
    <div class="bk-modal-card">
      <div class="bk-modal-head">
        <h3 id="resTitle">导入完成</h3>
        <button type="button" id="resClose" aria-label="关闭" @click="resultOpen = false">✕</button>
      </div>
      <div id="resBody" class="bk-modal-body">
        <p v-for="row in resultRows" :key="row.k">{{ row.k }}：{{ row.v }}</p>
        <p v-if="!resultRows.length">导入已完成</p>
      </div>
      <div class="bk-modal-foot">
        <button type="button" id="resOk" class="bk-btn bk-primary" @click="resultOpen = false">完成</button>
      </div>
    </div>
  </div>

  <!-- 页脚（旧版 .bk-bk-foot） -->
  <div class="bk-bk-foot">
    <span><b>互导口径</b>：手机端口令加密的备份在电脑导入时需输入同一口令；电脑导出的私钥一律不带口令写入。</span>
    <span><b>带口令的私钥</b>只导出元数据不导出材料（桌面只出不进）。</span>
  </div>

  <!-- toast（旧版 #bkToast） -->
  <div v-if="bkToast" id="bkToast" class="toast" role="status">{{ bkToast }}</div>

  <p v-if="message" class="bk-note" style="color: rgb(var(--ok))">{{ message }}</p>
  <p v-if="errorText" class="bk-err">{{ errorText }}</p>
</template>
