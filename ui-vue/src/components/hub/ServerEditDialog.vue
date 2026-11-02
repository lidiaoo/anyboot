<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { HubServer } from "@/stores/servers";

/**
 * 添加 / 编辑服务器弹窗（原 `ui/hub.html` 的「添加 / 编辑弹窗」1450~1762 行）。
 *
 * 表单字段对齐 Rust `hub::HubServer`；`password_new` 是**只写**字段
 * （`skip_serializing`）：不填 = 不动已存密码，填空串 = 清除。
 *
 * 认证三选一：
 *   · 交互 —— 三个字段全空，密码/首次指纹在 ssh 里输（凭据不落盘）
 *   · 密码 —— `password_new` 非空
 *   · 密钥 —— `key_id` 非空
 */

const props = defineProps<{
  open: boolean;
  server?: HubServer | null;
}>();

const emit = defineEmits<{
  (e: "save", payload: Record<string, unknown>): void;
  (e: "close"): void;
}>();

const form = ref({
  id: "",
  name: "",
  protocol: "ssh",
  host: "",
  addressesText: "",
  username: "",
  port: 0,
  notes: "",
  key_id: "",
  jump_id: "",
  term_path: "",
  term_shell: "",
  password_new: "",
});

const authMode = ref<"interactive" | "password" | "key">("interactive");
const saving = ref(false);
const errorText = ref("");

const isEdit = computed(() => Boolean(props.server?.id));

watch(
  () => props.server,
  (server) => {
    if (!server) {
      form.value = {
        id: "",
        name: "",
        protocol: "ssh",
        host: "",
        addressesText: "",
        username: "",
        port: 0,
        notes: "",
        key_id: "",
        jump_id: "",
        term_path: "",
        term_shell: "",
        password_new: "",
      };
      authMode.value = "interactive";
      return;
    }
    form.value = {
      id: server.id,
      name: server.name,
      protocol: server.protocol,
      host: server.host,
      addressesText: (server.addresses ?? []).join("\n"),
      username: server.username,
      port: server.port,
      notes: server.notes,
      key_id: server.key_id,
      jump_id: server.jump_id,
      term_path: server.term_path,
      term_shell: server.term_shell,
      password_new: "",
    };
    if (server.key_id) authMode.value = "key";
    else if (server.password_enc) authMode.value = "password";
    else authMode.value = "interactive";
  },
  { immediate: true },
);

function submit() {
  errorText.value = "";
  if (!form.value.name.trim()) {
    errorText.value = "名称不能为空";
    return;
  }
  if (!form.value.host.trim()) {
    errorText.value = "主机地址不能为空";
    return;
  }
  const addresses = form.value.addressesText
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  const payload: Record<string, unknown> = {
    id: form.value.id,
    name: form.value.name.trim(),
    protocol: form.value.protocol,
    host: form.value.host.trim(),
    port: Number(form.value.port) || 0,
    addresses,
    username: form.value.username.trim(),
    notes: form.value.notes,
    // 认证三选一：只在对应模式下提交，其余清空
    key_id: authMode.value === "key" ? form.value.key_id.trim() : "",
    jump_id: form.value.jump_id.trim(),
    term_path: form.value.term_path.trim(),
    term_shell: form.value.term_shell.trim(),
  };
  if (authMode.value === "password" && form.value.password_new) {
    payload.password_new = form.value.password_new;
  } else if (authMode.value === "interactive") {
    payload.password_new = ""; // 显式清除已存密码
  }
  saving.value = true;
  emit("save", payload);
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="backdrop" @click.self="emit('close')">
      <div class="dialog" role="dialog" aria-modal="true">
        <header>
          <h2>{{ isEdit ? "编辑服务器" : "添加服务器" }}</h2>
          <button type="button" class="btn icon ghost sm" @click="emit('close')">✕</button>
        </header>

        <div class="body">
          <div class="row">
            <label>名称</label>
            <input v-model="form.name" placeholder="给这台服务器起个名字" />
          </div>

          <div class="row">
            <label>协议</label>
            <select v-model="form.protocol">
              <option value="ssh">SSH</option>
              <option value="telnet">Telnet</option>
            </select>
          </div>

          <div class="row">
            <label>主机</label>
            <input v-model="form.host" placeholder="192.168.1.10 或域名" />
            <input v-model.number="form.port" class="port" type="number" placeholder="端口" />
          </div>

          <div class="row">
            <label>其它地址</label>
            <textarea
              v-model="form.addressesText"
              rows="2"
              placeholder="一行一个（测试连接时会逐个探测，可留空）"
            />
          </div>

          <div class="row">
            <label>用户名</label>
            <input v-model="form.username" placeholder="登录用户名" />
          </div>

          <div class="row">
            <label>认证</label>
            <div class="segmented">
              <button
                type="button"
                :aria-selected="authMode === 'interactive'"
                @click="authMode = 'interactive'"
              >
                交互输入
              </button>
              <button
                type="button"
                :aria-selected="authMode === 'password'"
                @click="authMode = 'password'"
              >
                密码
              </button>
              <button type="button" :aria-selected="authMode === 'key'" @click="authMode = 'key'">
                密钥
              </button>
            </div>
          </div>

          <div v-if="authMode === 'password'" class="row">
            <label>密码</label>
            <input
              v-model="form.password_new"
              type="password"
              :placeholder="isEdit ? '留空 = 不改已存密码' : '登录密码'"
            />
          </div>

          <div v-if="authMode === 'key'" class="row">
            <label>密钥</label>
            <input v-model="form.key_id" placeholder="密钥文件基名（到「密钥」页管理）" />
          </div>

          <div class="row">
            <label>跳板机</label>
            <input v-model="form.jump_id" placeholder="经哪台服务器中转（留空 = 直连）" />
          </div>

          <div class="row">
            <label>终端路径</label>
            <input v-model="form.term_path" placeholder="登录后先 cd 到这里（可留空）" />
          </div>

          <div class="row">
            <label>终端 shell</label>
            <input v-model="form.term_shell" placeholder="/bin/bash 等（留空 = 登录 shell）" />
          </div>

          <div class="row">
            <label>备注</label>
            <textarea v-model="form.notes" rows="2" placeholder="可选" />
          </div>

          <p v-if="errorText" class="err">{{ errorText }}</p>
        </div>

        <footer>
          <button type="button" class="btn ghost" @click="emit('close')">取消</button>
          <button type="button" class="btn primary" :disabled="saving" @click="submit">
            {{ saving ? "保存中…" : "保存" }}
          </button>
        </footer>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.backdrop {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.35);
  display: grid;
  place-items: center;
  z-index: 80;
  padding: 20px;
}
.dialog {
  width: min(560px, 100%);
  max-height: 88vh;
  background: var(--panel, #fff);
  border-radius: 12px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  box-shadow: 0 20px 50px rgba(15, 23, 42, 0.25);
}
header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 18px;
  border-bottom: 1px solid var(--line, #e5e7eb);
}
h2 {
  font-size: 15px;
  margin: 0;
}
.body {
  padding: 16px 18px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.row {
  display: grid;
  grid-template-columns: 84px 1fr;
  gap: 10px;
  align-items: start;
}
.row label {
  font-size: 12.5px;
  color: var(--fg-faint, #9ca3af);
  padding-top: 7px;
}
input,
textarea,
select {
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 7px;
  padding: 6px 10px;
  font-size: 12.5px;
  font-family: inherit;
  width: 100%;
}
.row input.port {
  grid-column: 3;
  width: 90px;
}
textarea {
  resize: vertical;
}
.segmented {
  display: flex;
  gap: 2px;
  border: 1px solid var(--line, #e5e7eb);
  border-radius: 8px;
  padding: 2px;
  width: fit-content;
}
.segmented button {
  border: 0;
  background: transparent;
  font-size: 12px;
  padding: 4px 12px;
  border-radius: 6px;
  cursor: pointer;
  color: var(--fg-dim, #4b5563);
}
.segmented button[aria-selected="true"] {
  background: var(--accent, #2563eb);
  color: #fff;
}
footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 18px;
  border-top: 1px solid var(--line, #e5e7eb);
}
.err {
  font-size: 12px;
  color: #dc2626;
  margin: 0;
}
</style>
