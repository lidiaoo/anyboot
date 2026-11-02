<script setup lang="ts">
import { onMounted, ref } from "vue";
import { call } from "@/api/tauri";

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
  has_passphrase: boolean;
}

const keys = ref<KeyRow[]>([]);
const loading = ref(true);
const busy = ref("");
const message = ref("");
const errorText = ref("");

/** 生成密钥弹层（旧版 #genModal） */
const genOpen = ref(false);
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
  await run(
    "生成密钥",
    () =>
      call("hub_key_generate", {
        name: genName.value.trim(),
        kind: genKind.value,
        passphrase: genPassphrase.value,
      }),
    "密钥已生成",
  );
}

function doImport() {
  void run("导入私钥", () => call("hub_key_import"), "私钥已导入");
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

function removeKey(id: string) {
  void run("删除密钥", () => call("hub_key_delete", { id }), "已删除");
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
    <span class="count mono">{{ keys.length }} 把</span>
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

  <div class="toolbar">
    <button type="button" class="btn primary" @click="doGenerate">
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round">
        <path d="M12 5v14M5 12h14" />
      </svg>
      生成密钥
    </button>
    <button type="button" class="btn" :disabled="!!busy" @click="doImport">
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3v12M8 7l4-4 4 4M4 15v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4" />
      </svg>
      导入私钥
    </button>
    <p class="hint">私钥加密存本机 · 公钥可一键复制</p>
  </div>

  <ul class="keys scroll-thin">
    <li v-for="key in keys" :key="key.id">
      <span class="mono key-name">{{ key.name }}</span>
      <span class="key-kind">{{ key.kind || "—" }}</span>
      <span v-if="key.has_passphrase" class="key-pass">有口令</span>
      <span style="flex: 1" />
      <button type="button" class="link-btn" @click="copyPublic(key.id)">复制公钥</button>
      <button type="button" class="link-btn danger" :disabled="!!busy" @click="removeKey(key.id)">
        删除
      </button>
    </li>
  </ul>

  <div v-if="!loading && !keys.length" class="empty-tip">
    还没有密钥 —— 生成一把 Ed25519（推荐）或导入现有私钥
  </div>

  <footer>
    使用方式：在服务器编辑里把认证方式切到「密钥」并选择钥匙；把对应<b>公钥</b>追加到服务器的
    <code>~/.ssh/authorized_keys</code> 即可。私钥不离开本机，删除服务器不会删除密钥。
  </footer>

  <p v-if="message" class="hint" style="color: rgb(var(--ok))">{{ message }}</p>
  <p v-if="errorText" class="hint" style="color: rgb(var(--bad))">{{ errorText }}</p>

  <!-- 生成密钥弹层（旧版 #genModal） -->
  <div v-if="genOpen" class="modal" @click.self="genOpen = false">
    <div class="modal-card" role="dialog" aria-modal="true">
      <div class="modal-head">
        <h3>生成密钥</h3>
        <button type="button" class="icon-btn" aria-label="关闭" @click="genOpen = false">✕</button>
      </div>
      <div class="modal-body">
        <div class="field">
          <label for="genName">名称（可留空，自动生成）</label>
          <input id="genName" v-model="genName" type="text" placeholder="如：home-nas" />
        </div>
        <div class="field">
          <label>类型</label>
          <div class="segmented">
            <button type="button" :aria-selected="genKind === 'ed25519'" @click="genKind = 'ed25519'">
              Ed25519（推荐）
            </button>
            <button type="button" :aria-selected="genKind === 'rsa'" @click="genKind = 'rsa'">
              RSA 4096
            </button>
          </div>
        </div>
        <div class="field">
          <label for="genPass">口令（可留空）</label>
          <input id="genPass" v-model="genPassphrase" type="password" placeholder="留空 = 无口令" />
        </div>
      </div>
      <div class="modal-foot">
        <button type="button" class="btn" @click="genOpen = false">取消</button>
        <button type="button" class="btn primary" @click="confirmGenerate">生成</button>
      </div>
    </div>
  </div>
</template>
