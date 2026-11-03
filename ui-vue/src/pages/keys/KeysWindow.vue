<script setup lang="ts">
const toast = useToastStore();
import { useEscape } from "@/composables/useEscape";
import { computed, onMounted, ref } from "vue";
import { call } from "@/api/tauri";
import { useToastStore } from "@/stores/toast";

/**
 * 密钥窗（原 `ui/hub-keys.html`，465 行）。
 *
 * ## 1:1 结构
 * ```
 * header
 *   span.appicon(svg) + h1「密钥管理」+ span.count#count
 *   .tools > button.toolbtn#hubBtn / #themeBtn
 * .toolbar
 *   button.btn.primary#genBtn（生成密钥，+ 图标）
 *   button.btn#importBtn（导入私钥，↓ 图标）
 *   p.hint「私钥加密存本机 · 公钥可一键复制」
 * ul.keys.scroll-thin#list
 * .empty-tip#emptyTip
 * footer（使用方式说明，原文照抄）
 * .modal#genModal（生成密钥弹层）
 * ```
 */

interface KeyRow {
  id: string;
  name: string;
  kind: string;
  created_at: number;
  updated_at?: number;
  fingerprint?: string;
  used_by?: number;
  has_passphrase: boolean;
}

const keys = ref<KeyRow[]>([]);
const loading = ref(true);
const busy = ref("");
const message = ref("");
const errorText = ref("");

/** 生成密钥弹层（旧版 #genModal） */
const genOpen = ref(false);

// Esc 关闭生成密钥弹层（对齐旧版 #genModal）
useEscape(genOpen, () => { genOpen.value = false; });
const genName = ref("");
const genKind = ref<"ed25519" | "rsa">("ed25519");
const genPassphrase = ref("");

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

// 主题跟随（旧版 keys 窗监听 theme-changed）
try {
  const { listen } = await import("@tauri-apps/api/event");
  void listen<{ theme?: string }>("theme-changed", (event) => {
    const theme = event.payload?.theme;
    if (theme) applyTheme(theme !== "light");
  });
} catch { /* 非 Tauri 环境 */ }

function toggleTheme() {
  dark.value = !dark.value;
  applyTheme(dark.value);
  try {
    localStorage.setItem("anyboot.theme", dark.value ? "dark" : "light");
  } catch {
    /* 忽略 */
  }
}

async function refresh() {
  try {
    keys.value = (await call<KeyRow[]>("hub_keys_list")) ?? [];
  } catch (error) {
    errorText.value = String(error);
    keys.value = [];
  } finally {
    loading.value = false;
  }
}

onMounted(refresh);

async function run(label: string, action: () => Promise<unknown>, done: string) {
  if (busy.value) return;
  busy.value = label;
  message.value = "";
  errorText.value = "";
  try {
    await action();
    message.value = done;
    showToast(done);
    await refresh();
  } catch (error) {
    errorText.value = String(error);
  } finally {
    busy.value = "";
  }
}

function doGenerate() {
  genName.value = "";
  genKind.value = "ed25519";
  genPassphrase.value = "";
  genOpen.value = true;
}

async function confirmGenerate() {
  genOpen.value = false;
  busy.value = "生成密钥";
  try {
    const meta = await call<{ id: string; fingerprint?: string }>("hub_key_generate", {
      name: genName.value.trim(),
      kind: genKind.value,
      passphrase: genPassphrase.value,
    });
    // 旧版 toast：已生成 {id}（指纹）— 记得把公钥装到服务器
    toast.show(`已生成 ${meta.id}（${shortenFp(meta.fingerprint ?? "")}）— 记得把公钥装到服务器`);
    await refresh();
  } catch (error) {
    toast.show("生成失败：" + error, "bad");
  } finally {
    busy.value = "";
  }
}

async function doImport() {
  busy.value = "导入私钥";
  try {
    const meta = await call<{ id: string; fingerprint?: string }>("hub_key_import");
    // 旧版 toast：已导入 {id}（指纹）
    toast.show(`已导入 ${meta.id}（${shortenFp(meta.fingerprint ?? "")}）`);
    await refresh();
  } catch (error) {
    toast.show("导入失败：" + error, "bad");
  } finally {
    busy.value = "";
  }
}

/** 密钥元信息（旧版 key-meta 文案） */
function keyMeta(key: KeyRow): string {
  const fp = key.fingerprint ? shortenFp(key.fingerprint) : "—";
  const date = fmtDate(key.updated_at ?? key.created_at);
  return `${fp} · ${date} · 被 ${key.used_by ?? 0} 台服务器使用`;
}
function shortenFp(fp: string): string {
  return fp.length > 24 ? `${fp.slice(0, 12)}…${fp.slice(-12)}` : fp;
}
function fmtDate(ts: number): string {
  if (!ts) return "—";
  return new Date(ts).toLocaleDateString("zh-CN");
}

async function copyPublic(id: string) {
  try {
    const text = await call<string>("hub_key_public_key", { id });
    await navigator.clipboard.writeText(text);
    message.value = "公钥已复制到剪贴板";
  } catch (error) {
    errorText.value = String(error);
  }
}

/** 删除确认弹层（旧版 #delModal） */
const delOpen = ref(false);
const delTarget = ref("");
const delBody = computed(() => {
  const k = keys.value.find((x) => x.id === delTarget.value);
  const name = k?.name || delTarget.value || "";
  // 旧版双分支：被引用 → 警告认证失效；未引用 → 仅不可恢复
  if ((k?.used_by ?? 0) > 0) {
    return `「${name}」正被 ${k!.used_by} 台服务器使用，删除后这些服务器的密钥认证将失效。此操作不可撤销。`;
  }
  return `「${name}」没有被任何服务器引用。删除后不可恢复。`;
});
const toastText = ref("");
function showToast(text: string) {
  toastText.value = text;
  window.setTimeout(() => {
    if (toastText.value === text) toastText.value = "";
  }, 2600);
}

function removeKey(id: string) {
  delTarget.value = id;
  delOpen.value = true;
}

async function confirmDelete() {
  delOpen.value = false;
  await run("删除密钥", () => call("hub_key_delete", { id: delTarget.value }), "已删除");
}

function openHub() {
  void call("hub_open_window", {}).catch(() => {});
}
</script>

<template>
  <header>
    <span class="appicon" aria-hidden="true">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="8" cy="15" r="4" />
        <path d="M10.8 12.2 20 3M17 6l2.5 2.5M14 9l2 2" />
      </svg>
    </span>
    <h1>密钥管理</h1>
    <span class="count mono" id="count">{{ keys.length }} 把</span>
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

  <div class="toolbar">
    <button type="button" class="btn primary" id="genBtn" @click="doGenerate">
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
        <path d="M12 5v14M5 12h14" />
      </svg>
      生成密钥
    </button>
    <button type="button" class="btn" id="importBtn" :disabled="!!busy" @click="doImport">
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3v12M8 7l4-4 4 4M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
      </svg>
      导入私钥
    </button>
    <p class="hint">私钥加密存本机 · 公钥可一键复制</p>
  </div>

  <ul class="keys scroll-thin" id="list">
    <li v-for="key in keys" :key="key.id" class="key-card">
      <span class="key-icon" aria-hidden="true">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="8" cy="15" r="4" />
          <path d="M10.8 12.2 20 3M17 6l2.5 2.5" />
        </svg>
      </span>
      <div class="key-main">
        <div class="key-badges">
          <p class="name">{{ key.id }}</p>
          <span class="badge kind">{{ key.kind }}</span>
          <span v-if="!key.has_passphrase" class="badge nowarn">
            <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
              <path d="M12 9v4M12 17h.01" />
            </svg>
            未设口令
          </span>
        </div>
        <p class="key-meta">{{ keyMeta(key) }}</p>
      </div>
      <div class="key-actions">
        <button type="button" class="btn small copy" @click="copyPublic(key.id)">复制公钥</button>
        <button type="button" class="delbtn" aria-label="删除密钥" title="删除密钥" :disabled="!!busy" @click="removeKey(key.id)">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 6h18M8 6V4h8v2M6 6l1 15h10l1-15" />
          </svg>
        </button>
      </div>
    </li>
  </ul>

  <div v-if="!loading && !keys.length" class="empty-tip" id="emptyTip">
    还没有密钥 —— 生成一把 Ed25519（推荐）或导入现有私钥
  </div>

  <footer>
    使用方式：在服务器编辑里把认证方式切到「密钥」并选择钥匙；把对应<b>公钥</b>追加到服务器的
    <code>~/.ssh/authorized_keys</code> 即可。私钥不离开本机，删除服务器不会删除密钥。
  </footer>

  <p v-if="message" class="hint" style="color: rgb(var(--ok))">{{ message }}</p>
  <p v-if="errorText" class="hint" style="color: rgb(var(--bad))">{{ errorText }}</p>

  <!-- 生成密钥弹层（旧版 #genModal） -->
  <div v-if="genOpen" id="genModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="genTitle">
    <div class="modal-card">
      <div class="modal-head">
        <h3 id="genTitle">生成密钥</h3>
        <button type="button" id="genClose" aria-label="关闭" @click="genOpen = false">✕</button>
      </div>
      <form id="genForm" class="modal-body" @submit.prevent="confirmGenerate">
        <div>
          <label>类型</label>
          <div class="seg" id="genKind">
            <button type="button" data-kind="ed25519" :aria-selected="genKind === 'ed25519'" @click="genKind = 'ed25519'">
              Ed25519 <span class="mono" style="font-size: 10px; opacity: 0.7">推荐</span>
            </button>
            <button type="button" data-kind="rsa" :aria-selected="genKind === 'rsa'" @click="genKind = 'rsa'">
              RSA-4096
            </button>
          </div>
        </div>
        <div>
          <label for="genName">名称</label>
          <input id="genName" v-model="genName" placeholder="例如 id_ed25519_home" autocomplete="off" required />
        </div>
        <div>
          <label for="genPass">口令（可选）</label>
          <input id="genPass" v-model="genPassphrase" type="password" placeholder="留空则不设口令" autocomplete="new-password" />
          <span class="field-tip">设口令后每次连接需在终端里解锁一次；不设则开箱即用，泄露风险略高</span>
        </div>
        <p class="modal-note">生成后将展示公钥指纹，并提示复制公钥到目标服务器</p>
        <div class="modal-foot" style="border: 0; padding: 4px 0 0">
          <button type="button" class="btn plain" id="genCancel" @click="genOpen = false">取消</button>
          <button type="submit" class="btn primary">生成</button>
        </div>
      </form>
    </div>
  </div>

  <!-- 删除确认弹层（旧版 #delModal） -->
  <div v-if="delOpen" id="delModal" class="modal" role="dialog" aria-modal="true" aria-labelledby="delTitle">
    <div class="modal-card">
      <div class="del-hero">
        <span class="icon" aria-hidden="true">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 9v4M12 17h.01M10.3 3.9L1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" />
          </svg>
        </span>
        <h3 id="delTitle">删除密钥？</h3>
      </div>
      <p id="delBody" class="del-body">{{ delBody }}</p>
      <div class="modal-foot">
        <button type="button" class="btn plain" id="delCancel" @click="delOpen = false">取消</button>
        <button type="button" class="btn danger" id="delOk" @click="confirmDelete">删除</button>
      </div>
    </div>
  </div>

  <!-- toast（旧版 #toast） -->
  <div v-if="toastText" id="toast">{{ toastText }}</div>
</template>
