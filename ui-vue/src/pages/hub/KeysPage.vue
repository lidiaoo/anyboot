<script setup lang="ts">
import { onMounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 密钥页（原 `ui/hub.html` 的 `#page-keys`）—— hub_key* 命令族 5 个。
 *
 * SSH 公钥管理：列出 / 生成 / 导入 / 复制公钥 / 删除。
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

async function refresh() {
  try {
    keys.value = (await call<KeyRow[]>("hub_keys_list")) ?? [];
  } catch (error) {
    errorText.value = String(error);
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

function generate() {
  const name = window.prompt("新密钥的名称（留空用默认名）", "");
  if (name === null) return;
  void run(
    "生成密钥",
    () => call("hub_key_generate", { name: name.trim() }),
    "密钥已生成",
  );
}

function importKey() {
  void run("导入密钥", () => call("hub_key_import"), "密钥已导入");
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

function remove(id: string) {
  void run("删除密钥", () => call("hub_key_delete", { id }), "已删除");
}
</script>

<template>
  <div class="keys-page">
    <div class="head">
      <h2>SSH 密钥</h2>
      <span class="spacer" />
      <button type="button" class="btn sm" :disabled="!!busy" @click="generate">生成密钥</button>
      <button type="button" class="btn sm" :disabled="!!busy" @click="importKey">导入密钥</button>
    </div>

    <p v-if="loading" class="hint">读取中…</p>
    <table v-else-if="keys.length" class="keys">
      <thead>
        <tr>
          <th>名称</th>
          <th>类型</th>
          <th>口令</th>
          <th />
        </tr>
      </thead>
      <tbody>
        <tr v-for="key in keys" :key="key.id">
          <td class="mono">{{ key.name }}</td>
          <td>{{ key.kind || "—" }}</td>
          <td>{{ key.has_passphrase ? "有" : "无" }}</td>
          <td class="ops">
            <button type="button" class="link-btn" @click="copyPublic(key.id)">复制公钥</button>
            <button type="button" class="link-btn danger" :disabled="!!busy" @click="remove(key.id)">
              删除
            </button>
          </td>
        </tr>
      </tbody>
    </table>
    <div v-else class="empty">
      <p class="t">还没有密钥</p>
      <p class="s">点「生成密钥」新建一把，或「导入密钥」使用已有的私钥。</p>
    </div>

    <p v-if="message" class="ok">{{ message }}</p>
    <p v-if="errorText" class="err">{{ errorText }}</p>
  </div>
</template>

<style scoped>
.keys-page {
  max-width: 720px;
}
.head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 14px;
}
.head h2 {
  font-size: 16px;
  margin: 0;
}
.spacer {
  flex: 1;
}
.keys {
  width: 100%;
  border-collapse: collapse;
  font-size: 12.5px;
  background: var(--panel, #fff);
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 10px;
  overflow: hidden;
}
.keys th {
  text-align: left;
  font-weight: 500;
  font-size: 11.5px;
  color: var(--fg-faint, #9ca3af);
  padding: 8px 12px;
  border-bottom: 1px solid var(--line, #f1f5f9);
}
.keys td {
  padding: 9px 12px;
  border-bottom: 1px solid var(--line, #f8fafc);
}
.ops {
  display: flex;
  gap: 10px;
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
}
.link-btn {
  border: 0;
  background: transparent;
  color: var(--accent, #2563eb);
  font-size: 12px;
  cursor: pointer;
  padding: 0;
}
.link-btn.danger {
  color: #dc2626;
}
.hint,
.empty .s {
  font-size: 12.5px;
  color: var(--fg-faint, #9ca3af);
}
.empty {
  padding: 40px 0;
  text-align: center;
}
.empty .t {
  font-size: 14px;
  color: var(--fg-dim, #6b7280);
  margin: 0 0 6px;
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
