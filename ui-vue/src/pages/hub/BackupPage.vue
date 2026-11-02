<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 备份与恢复页（原 `ui/hub.html` 的 `#page-backup`，948 行）—— backup 命令族 11 个。
 *
 * 两块：
 *   ① **本地备份**：导出（选路径 → 导出 → 预览） / 导入（选文件 → 预览 → 导入）
 *   ② **WebDAV 同步**：地址 / 账号 / 路径配置 → 测试连接 → 立即同步
 *
 * ## 历史坑（务必保留）
 *
 * WebDAV 地址**没写协议**曾被判成「未配置」，导致启用同步反复弹提示。
 * 现在后端会自动补 `https://`（`sync_is_configured_accepts_bare_host` 有回归测试），
 * 前端这边也给出明确提示。
 */

interface SyncState {
  enabled: boolean;
  url: string;
  username: string;
  hasPassword: boolean;
  remoteDir: string;
  lastSyncAt: string;
  lastError: string;
  [key: string]: unknown;
}

const sync = ref<SyncState | null>(null);
const busy = ref("");
const message = ref("");
const errorText = ref("");

/** 本地导出/导入的预览信息 */
const exportPath = ref("");
const importPath = ref("");
const preview = ref<string>("");

const syncEnabled = computed(() => Boolean(sync.value?.enabled));
const syncConfigured = computed(() => Boolean(sync.value?.url?.trim()));

async function refreshSync() {
  try {
    sync.value = await call<SyncState>("backup_sync_get_state");
  } catch (error) {
    errorText.value = String(error);
  }
}

onMounted(refreshSync);

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

async function pickExportPath() {
  try {
    const path = await call<string>("backup_pick_export_path");
    if (path) exportPath.value = path;
  } catch (error) {
    errorText.value = String(error);
  }
}

async function doExport() {
  if (!exportPath.value) return;
  await run(
    "导出",
    () => call("backup_export", { path: exportPath.value }),
    `已导出到 ${exportPath.value}`,
  );
}

async function pickImportFile() {
  try {
    const path = await call<string>("backup_pick_import_file");
    if (!path) return;
    importPath.value = path;
    const info = await call<unknown>("backup_preview", { path });
    preview.value = JSON.stringify(info, null, 2);
  } catch (error) {
    errorText.value = String(error);
  }
}

async function doImport() {
  if (!importPath.value) return;
  await run(
    "导入",
    () => call("backup_import", { path: importPath.value }),
    "导入完成（设置已恢复）",
  );
}

async function testWebdav() {
  await run("测试 WebDAV", () => call("backup_webdav_test"), "WebDAV 连接正常");
}

async function syncNow() {
  await run("立即同步", () => call("backup_sync_now"), "同步完成");
}

async function toggleSync() {
  const next = !syncEnabled.value;
  await run(
    next ? "启用同步" : "关闭同步",
    () => call("backup_sync_set_enabled", { enabled: next }),
    next ? "已启用 WebDAV 同步" : "已关闭 WebDAV 同步",
  );
}
</script>

<template>
  <div class="backup-page">
    <!-- ① 本地备份 -->
    <section class="card">
      <header><h2>本地备份</h2></header>

      <div class="row">
        <label>导出到</label>
        <input v-model="exportPath" class="mono" placeholder="选择保存位置…" readonly />
        <button type="button" class="btn sm" @click="pickExportPath">选择</button>
        <button type="button" class="btn primary sm" :disabled="!exportPath || !!busy" @click="doExport">
          导出
        </button>
      </div>

      <div class="row">
        <label>从文件恢复</label>
        <input v-model="importPath" class="mono" placeholder="选择一个备份文件…" readonly />
        <button type="button" class="btn sm" @click="pickImportFile">选择</button>
        <button type="button" class="btn sm" :disabled="!importPath || !!busy" @click="doImport">
          导入
        </button>
      </div>

      <pre v-if="preview" class="preview">{{ preview }}</pre>
    </section>

    <!-- ② WebDAV 同步 -->
    <section class="card">
      <header>
        <h2>WebDAV 同步</h2>
        <span :class="['pill', syncEnabled ? 'is-on' : 'is-off']">
          {{ syncEnabled ? "已启用" : "已关闭" }}
        </span>
      </header>

      <dl v-if="sync">
        <dt>地址</dt>
        <dd class="mono">{{ sync.url || "（未配置）" }}</dd>
        <dt>账号</dt>
        <dd>{{ sync.username || "（未配置）" }}</dd>
        <dt>密码</dt>
        <dd>{{ sync.hasPassword ? "已保存" : "未保存" }}</dd>
        <dt>远端目录</dt>
        <dd class="mono">{{ sync.remoteDir || "/" }}</dd>
        <dt>上次同步</dt>
        <dd>{{ sync.lastSyncAt || "从未" }}</dd>
        <dt v-if="sync.lastError">错误</dt>
        <dd v-if="sync.lastError" class="err">{{ sync.lastError }}</dd>
      </dl>

      <p v-if="!syncConfigured" class="hint">
        填好 WebDAV 地址（可以只写主机名，会自动补 https://）后才能启用同步。
      </p>

      <div class="actions">
        <button type="button" class="btn sm" :disabled="!!busy" @click="testWebdav">测试连接</button>
        <button type="button" class="btn sm" :disabled="!syncConfigured || !!busy" @click="toggleSync">
          {{ syncEnabled ? "关闭同步" : "启用同步" }}
        </button>
        <button type="button" class="btn primary sm" :disabled="!syncEnabled || !!busy" @click="syncNow">
          立即同步
        </button>
      </div>
    </section>

    <p v-if="message" class="ok">{{ message }}</p>
    <p v-if="errorText" class="err">{{ errorText }}</p>
    <p v-if="busy" class="hint">{{ busy }}中…</p>
  </div>
</template>

<style scoped>
.backup-page {
  display: flex;
  flex-direction: column;
  gap: 14px;
  max-width: 820px;
}
.card {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  background: var(--panel, #fff);
  padding: 16px 18px;
}
header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 12px;
}
h2 {
  font-size: 14px;
  margin: 0;
}
.pill {
  font-size: 11px;
  padding: 2px 9px;
  border-radius: 999px;
  background: var(--soft, #f3f4f6);
  color: var(--fg-faint, #9ca3af);
}
.pill.is-on {
  background: #dcfce7;
  color: #15803d;
}
.row {
  display: grid;
  grid-template-columns: 90px 1fr auto auto;
  gap: 8px;
  align-items: center;
  margin-bottom: 10px;
}
.row label {
  font-size: 12.5px;
  color: var(--fg-faint, #9ca3af);
}
input {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 7px;
  padding: 6px 10px;
  font-size: 12.5px;
  background: var(--soft, #f9fafb);
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
}
dl {
  display: grid;
  grid-template-columns: 90px 1fr;
  gap: 6px 14px;
  font-size: 12.5px;
  margin: 0 0 14px;
}
dt {
  color: var(--fg-faint, #9ca3af);
}
dd {
  margin: 0;
  word-break: break-all;
}
.preview {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11px;
  background: var(--soft, #f9fafb);
  border-radius: 8px;
  padding: 10px 12px;
  max-height: 220px;
  overflow: auto;
  margin: 10px 0 0;
}
.actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.hint {
  font-size: 12px;
  color: var(--fg-faint, #9ca3af);
  margin: 0 0 10px;
}
.ok {
  font-size: 12.5px;
  color: #15803d;
}
.err {
  font-size: 12.5px;
  color: #dc2626;
}
</style>
