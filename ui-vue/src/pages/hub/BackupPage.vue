<script setup lang="ts">
import { onMounted, ref } from "vue";
import { call } from "@/api/tauri";

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
const CATEGORIES = [
  { id: "settings", label: "设置" },
  { id: "servers", label: "服务器档案" },
  { id: "computers", label: "电脑档案" },
  { id: "keys", label: "SSH 私钥" },
  { id: "history", label: "动作历史" },
] as const;

const sync = ref<SyncState | null>(null);
const busy = ref("");
const message = ref("");
const errorText = ref("");

/** 导出：是否含私钥 → 需要口令 */
const exportIncludeKeys = ref(false);
const exportPw1 = ref("");
const exportPw2 = ref("");
const exportCats = ref<string[]>(["settings", "servers", "computers"]);
const syncCats = ref<string[]>(["settings", "servers", "computers"]);

const syncEnabled = ref(false);
const syncUrl = ref("");
const syncUser = ref("");
const syncPass = ref("");
const syncDir = ref("AnyBoot");
const syncPeriod = ref(15);
const syncOnChange = ref(false);

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
function toggleCat(which: "export" | "sync", id: string) {
  const source = which === "export" ? exportCats : syncCats;
  const next = source.value.includes(id)
    ? source.value.filter((x) => x !== id)
    : [...source.value, id];
  if (which === "export") exportCats.value = next;
  else syncCats.value = next;
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

async function doImport() {
  // 旧版流程：选文件 → 预览差异 → 确认导入
  let path = "";
  try {
    path = ((await call("backup_pick_import_file")) as string) ?? "";
  } catch {
    path = "";
  }
  if (!path) return;
  try {
    const preview = (await call("backup_preview", { path, password: "" })) as Record<string, unknown>;
    const summary = Object.entries(preview ?? {})
      .map(([key, value]) => `${key}: ${String(value)}`)
      .join("\n");
    if (!window.confirm(`导入预览\n\n${summary || "（无差异信息）"}\n\n确认导入？`)) return;
  } catch (error) {
    errorText.value = String(error);
    return;
  }
  await run("导入", () => call("backup_import", { path, password: "", categories: [], strategy: "merge" }), "导入完成（设置已恢复）");
}

async function saveSync() {
  await run(
    "保存同步配置",
    () =>
      call("backup_sync_save_config", {
        config: {
          url: syncUrl.value.trim(),
          username: syncUser.value.trim(),
          password: syncPass.value,
          remoteDir: syncDir.value.trim() || "AnyBoot",
          periodMinutes: syncPeriod.value,
          onChanged: syncOnChange.value,
          categories: syncCats.value,
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
  void run("测试 WebDAV", () => call("backup_webdav_test", { config: {} }), "WebDAV 连接正常");
}

function createWebdavDir() {
  void run(
    "创建远端目录",
    () => call("backup_webdav_mkdir", { config: { remoteDir: syncDir.value.trim() || "AnyBoot" } }),
    "远端目录已创建",
  );
}

function syncNow() {
  void run("立即同步", () => call("backup_sync_now"), "同步完成");
}
</script>

<template>
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
      <label v-for="cat in CATEGORIES" :key="cat.id" class="bk-radio-line">
        <input
          type="checkbox"
          :checked="exportCats.includes(cat.id)"
          @change="toggleCat('export', cat.id)"
        />
        {{ cat.label }}
      </label>
    </div>

    <div v-if="exportIncludeKeys" class="bk-info" id="exportKeyHint">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <circle cx="12" cy="12" r="9" />
        <path d="M12 8v5M12 16h.01" />
      </svg>
      <span>导出包含 <b>SSH 私钥明文</b>，请设置口令加密，并妥善保管备份文件。</span>
    </div>

    <div v-if="exportIncludeKeys" class="bk-pw-zone" id="exportPwZone">
      <div class="bk-pw-grid" id="exportPwGrid">
        <div class="bk-field">
          <label for="expPw1">备份口令</label>
          <input id="expPw1" v-model="exportPw1" type="password" placeholder="至少 4 位" autocomplete="new-password" />
          <div class="bk-strength"><i id="expStrength" :style="{ width: Math.min(100, exportPw1.length * 12) + '%' }" /></div>
        </div>
        <div class="bk-field">
          <label for="expPw2">再输一次</label>
          <input id="expPw2" v-model="exportPw2" type="password" placeholder="两次需一致" autocomplete="new-password" />
          <div class="bk-err" id="expPwErr">{{ exportPw1 && exportPw2 && exportPw1 !== exportPw2 ? "两次口令不一致" : "" }}</div>
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

      <div class="bk-cats" id="syncCats">
        <label v-for="cat in CATEGORIES" :key="cat.id" class="bk-radio-line">
          <input
            type="checkbox"
            :checked="syncCats.includes(cat.id)"
            @change="toggleCat('sync', cat.id)"
          />
          {{ cat.label }}
        </label>
      </div>
    </div>

    <div class="bk-sync-status" id="syncStatus">
      <span v-if="sync?.lastError" class="bk-err">{{ sync.lastError }}</span>
      <span v-else>上次同步：{{ sync?.lastSyncAt || "从未" }}</span>
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

  <p v-if="message" class="bk-note" style="color: rgb(var(--ok))">{{ message }}</p>
  <p v-if="errorText" class="bk-err">{{ errorText }}</p>
</template>
