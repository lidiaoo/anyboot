<script setup lang="ts">
import { onMounted, ref } from "vue";
import { call } from "@/api/tauri";

/**
 * 密钥窗（原 `ui/hub-keys.html`，465 行）—— hub_key* 命令族。
 * 与 hub 内嵌的「密钥」页共用同一套命令，这里做成独立窗形态。
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

onMounted(() => {
  void refresh().then(() => {
    window.setTimeout(() => {
      const line = `[vue-keys] keys=${keys.value.length}`;
      void import("@/api/tauri")
        .then(({ isTauri, call }) =>
          isTauri()
            ? call<void>("hub_report_error", { source: "vue-keys", message: line, stack: "" })
            : undefined,
        )
        .catch(() => {});
    }, 2000);
  });
});

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

async function copyPublic(id: string) {
  try {
    const text = await call<string>("hub_key_public_key", { id });
    await navigator.clipboard.writeText(text);
    message.value = "公钥已复制";
  } catch (error) {
    errorText.value = String(error);
  }
}
</script>

<template>
  <div class="keys-window">
    <header>
      <h1>SSH 密钥</h1>
      <span class="count">{{ keys.length }} 把</span>
      <span class="spacer" />
      <button
        type="button"
        class="btn sm"
        :disabled="!!busy"
        @click="run('生成', () => call('hub_key_generate', { name: '' }), '密钥已生成')"
      >
        生成
      </button>
      <button
        type="button"
        class="btn sm"
        :disabled="!!busy"
        @click="run('导入', () => call('hub_key_import'), '密钥已导入')"
      >
        导入
      </button>
    </header>

    <p v-if="loading" class="hint">读取中…</p>
    <div v-else-if="keys.length" class="list">
      <div v-for="key in keys" :key="key.id" class="row">
        <span class="name mono">{{ key.name }}</span>
        <span class="kind">{{ key.kind || "—" }}</span>
        <span class="pass">{{ key.has_passphrase ? "有口令" : "无口令" }}</span>
        <span class="spacer" />
        <button type="button" class="link-btn" @click="copyPublic(key.id)">复制公钥</button>
        <button
          type="button"
          class="link-btn danger"
          :disabled="!!busy"
          @click="run('删除', () => call('hub_key_delete', { id: key.id }), '已删除')"
        >
          删除
        </button>
      </div>
    </div>
    <div v-else class="empty">
      <p>还没有密钥</p>
      <p class="hint">点「生成」新建一把，或「导入」使用已有的私钥。</p>
    </div>

    <p v-if="message" class="ok">{{ message }}</p>
    <p v-if="errorText" class="err">{{ errorText }}</p>
  </div>
</template>

<style scoped>
.keys-window {
  padding: 16px 18px;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  display: flex;
  flex-direction: column;
  gap: 12px;
  height: 100vh;
  overflow-y: auto;
}
header {
  display: flex;
  align-items: center;
  gap: 10px;
}
h1 {
  font-size: 16px;
  margin: 0;
}
.count {
  font-size: 11.5px;
  color: #9ca3af;
}
.spacer {
  flex: 1;
}
.list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 9px 12px;
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  font-size: 12.5px;
  background: #fff;
}
.mono {
  font-family: ui-monospace, Menlo, monospace;
  font-size: 11.5px;
}
.kind,
.pass {
  font-size: 11.5px;
  color: #9ca3af;
}
.link-btn {
  border: 0;
  background: transparent;
  color: #2563eb;
  font-size: 12px;
  cursor: pointer;
  padding: 0;
}
.link-btn.danger {
  color: #dc2626;
}
.empty {
  text-align: center;
  padding: 40px 0;
  color: #6b7280;
  font-size: 14px;
}
.hint {
  font-size: 12.5px;
  color: #9ca3af;
  margin: 4px 0 0;
}
.ok {
  font-size: 12.5px;
  color: #15803d;
  margin: 0;
}
.err {
  font-size: 12.5px;
  color: #dc2626;
  margin: 0;
}
</style>
