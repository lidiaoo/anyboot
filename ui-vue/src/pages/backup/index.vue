<script setup lang="ts">
import { onMounted, ref } from "vue";
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
  url: string;
  username: string;
  hasPassword: boolean;
  remoteDir: string;
  lastSyncAt: string;
  lastError: string;
  periodMinutes?: number;
  onChanged?: boolean;
}

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

const dark = ref(false);
try {
  dark.value = localStorage.getItem("anyboot.theme") === "dark";
} catch {
  /* 吃默认 */
}
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

function doImport() {
  void run("导入", () => call("backup_import"), "导入完成（设置已恢复）");
}

function saveSync() {
  void run(
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

function syncNow() {
  void run("立即同步", () => call("backup_sync_now"), "同步完成");
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
      <button type="button" class="toolbtn" aria-label="打开远程管理" title="打开远程管理" @click="openHub">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <rect x="2" y="3" width="20" height="7" rx="2" />
          <rect x="2" y="14" width="20" height="7" rx="2" />
          <path d="M6 6.5h.01M6 17.5h.01" />
        </svg>
        远程管理
      </button>
      <button type="button" class="toolbtn" aria-label="切换主题" @click="toggleTheme">
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
    <section class="panel">
      <div class="panel-head">
        <span class="picon" aria-hidden="true">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 15V3M7 8l5-5 5 5M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
          </svg>
        </span>
        <h2>导出备份</h2>
        <span class="sub">导出为 .json 备份文件，可在手机 / 电脑间互导</span>
      </div>

      <div class="cats">
        <label v-for="cat in CATEGORIES" :key="cat.id" class="radio-line">
          <input
            type="checkbox"
            :checked="exportCats.includes(cat.id)"
            @change="toggleCat('export', cat.id)"
          />
          {{ cat.label }}
        </label>
      </div>

      <div v-if="exportIncludeKeys" class="info">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <circle cx="12" cy="12" r="9" />
          <path d="M12 8v5M12 16h.01" />
        </svg>
        <span>导出包含 <b>SSH 私钥明文</b>，请设置口令加密，并妥善保管备份文件。</span>
      </div>

      <div v-if="exportIncludeKeys" class="pw-zone">
        <div class="pw-grid">
          <div class="field">
            <label for="expPw1">备份口令</label>
            <input id="expPw1" v-model="exportPw1" type="password" placeholder="至少 4 位" autocomplete="new-password" />
            <div class="strength"><i :style="{ width: Math.min(100, exportPw1.length * 12) + '%' }" /></div>
          </div>
          <div class="field">
            <label for="expPw2">再输一次</label>
            <input id="expPw2" v-model="exportPw2" type="password" placeholder="两次需一致" autocomplete="new-password" />
            <div class="err">{{ exportPw1 && exportPw2 && exportPw1 !== exportPw2 ? "两次口令不一致" : "" }}</div>
          </div>
        </div>
      </div>

      <div class="row-actions">
        <button type="button" class="btn primary" :disabled="!!busy" @click="doExport">导出备份文件</button>
        <span class="note">不加密也能导出，但密钥明文会直接写进文件</span>
      </div>
    </section>

    <!-- 导入 -->
    <section class="panel">
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
        <button type="button" class="btn primary" :disabled="!!busy" @click="doImport">选择备份文件…</button>
        <span class="note">导入前会先预览差异，再让你确认</span>
      </div>
    </section>

    <hr class="divider" />

    <!-- WebDAV 同步 -->
    <section class="panel">
      <div class="sync-head">
        <span class="picon" aria-hidden="true">
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 7h-9l-2-2H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2z" />
          </svg>
        </span>
        <h2>WebDAV 云同步</h2>
        <label class="switch" style="margin-left: auto" title="开启 / 关闭同步">
          <input type="checkbox" :checked="syncEnabled" @change="toggleSync" />
          <i />
        </label>
      </div>

      <div class="form-grid">
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
            <div class="seg">
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
          <input v-model="syncOnChange" type="checkbox" />
          有变化时自动同步（改动后约 30 秒内推送）
        </label>

        <div class="cats">
          <label v-for="cat in CATEGORIES" :key="cat.id" class="radio-line">
            <input
              type="checkbox"
              :checked="syncCats.includes(cat.id)"
              @change="toggleCat('sync', cat.id)"
            />
            {{ cat.label }}
          </label>
        </div>
      </div>

      <div class="sync-status">
        <span v-if="sync?.lastError" class="err">{{ sync.lastError }}</span>
        <span v-else>上次同步：{{ sync?.lastSyncAt || "从未" }}</span>
      </div>

      <div class="row-actions">
        <button type="button" class="btn" :disabled="!!busy" @click="testWebdav">测试连接</button>
        <button type="button" class="btn" :disabled="!!busy" @click="saveSync">保存配置</button>
        <button type="button" class="btn primary" :disabled="!syncEnabled || !!busy" @click="syncNow">
          立即同步
        </button>
      </div>
    </section>

    <p v-if="message" class="note" style="color: rgb(var(--ok))">{{ message }}</p>
    <p v-if="errorText" class="err">{{ errorText }}</p>
  </main>
</template>
