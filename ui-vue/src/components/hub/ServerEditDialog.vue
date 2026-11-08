<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useEscape, useCtrlEnter } from "@/composables/useEscape";
import type { HubServer } from "@/stores/servers";

/**
 * 添加 / 编辑服务器弹窗（原 `ui/hub.html` 的 `#editOverlay`，4603~4695 行）。
 *
 * ## 1:1 结构（逐字照搬）
 * ```
 * .overlay
 *   .dialog[role=dialog]
 *     .dialog-head > h3#editTitle + button.icon-btn
 *     .dialog-body
 *       .field-row > .field(label+input) + .field.fixed(.flabel + .segmented)
 *       .field-row > .field 地址 + .field.fixed 端口
 *       .field 用户名
 *       .field-row > .field.fixed 认证方式(.segmented) + #fKeyField(select)
 *       #fPasswordField
 *       #fJumpField(select)
 *       .field-row > 终端启动路径 + 终端 shell(+datalist)
 *       .field 备注(textarea)
 *       p.field-hint（三种认证方式说明，原文照抄）
 *       p.test-line
 *       p.form-error
 *     .dialog-foot > 测试连接 + .spacer + 取消 + 保存
 * ```
 */

const props = defineProps<{
  open: boolean;
  server?: HubServer | null;
  /** 可选的密钥列表（#fKey 的选项） */
  keys?: { id: string; name: string }[];
  /** 可选的跳板机列表（#fJump 的选项） */
  jumpOptions?: HubServer[];
}>();

const emit = defineEmits<{
  (e: "save", payload: Record<string, unknown>): void;
  (e: "test", payload: Record<string, unknown>): void;
  (e: "close"): void;
}>();

// 弹层交互：Esc 关闭、Ctrl/Cmd+Enter 提交（对齐旧版）
const openRef = computed(() => Boolean(props.open));
useEscape(openRef, () => emit("close"));
useCtrlEnter(openRef, submit);


const name = ref("");
const protocol = ref<"ssh" | "telnet">("ssh");
const host = ref("");
const port = ref("");
const username = ref("");
const auth = ref<"interactive" | "password" | "key">("interactive");
const keyId = ref("");
const password = ref("");
const jumpId = ref("");
const termPath = ref("");
const termShell = ref("");
const notes = ref("");
const testLine = ref("");
const errorText = ref("");

const isEdit = computed(() => Boolean(props.server?.id));

watch(
  () => props.server,
  (server) => {
    if (!server) {
      name.value = "";
      protocol.value = "ssh";
      host.value = "";
      port.value = "";
      username.value = "";
      auth.value = "interactive";
      keyId.value = "";
      password.value = "";
      jumpId.value = "";
      termPath.value = "";
      termShell.value = "";
      notes.value = "";
      testLine.value = "";
      errorText.value = "";
      return;
    }
    name.value = server.name;
    protocol.value = server.protocol === "telnet" ? "telnet" : "ssh";
    host.value = server.host;
    port.value = server.port ? String(server.port) : "";
    username.value = server.username;
    auth.value = server.key_id ? "key" : server.password_enc ? "password" : "interactive";
    keyId.value = server.key_id;
    password.value = "";
    jumpId.value = server.jump_id;
    termPath.value = server.term_path;
    termShell.value = server.term_shell;
    notes.value = server.notes;
    testLine.value = "";
    errorText.value = "";
  },
  { immediate: true },
);

function buildPayload(): Record<string, unknown> {
  const payload: Record<string, unknown> = {
    id: props.server?.id ?? "",
    name: name.value.trim(),
    protocol: protocol.value,
    host: host.value.trim(),
    port: Number(port.value) || 0,
    addresses: [],
    username: username.value.trim(),
    notes: notes.value,
    // 三选一：只在对应模式下提交，其余清空
    key_id: auth.value === "key" ? keyId.value : "",
    jump_id: jumpId.value,
    term_path: termPath.value.trim(),
    term_shell: termShell.value.trim(),
  };
  if (auth.value === "password" && password.value) {
    payload.password_new = password.value;
  } else if (auth.value === "interactive") {
    payload.password_new = "";
  }
  return payload;
}

function submit() {
  errorText.value = "";
  if (!name.value.trim()) {
    errorText.value = "名称不能为空";
    return;
  }
  if (!host.value.trim()) {
    errorText.value = "地址不能为空";
    return;
  }
  emit("save", buildPayload());
}

async function doTest() {
  testLine.value = "正在测试…";
  try {
    const { call } = await import("@/api/tauri");
    const r = await call<{ ok?: boolean; message?: string; ms?: number }>("hub_test_server", { server: buildPayload() });
    testLine.value = r?.ok ? `通 · ${r.ms ?? "?"} ms` : (r?.message || "不通");
  } catch (e) {
    testLine.value = "测试失败：" + e;
  }
}
</script>

<template>
  <div v-if="open" id="editOverlay" class="overlay">
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="editTitle">
      <div class="dialog-head">
        <h3 id="editTitle">{{ isEdit ? "编辑服务器" : "添加服务器" }}</h3>
        <button type="button" class="icon-btn" id="editX" aria-label="关闭" @click="emit('close')">✕</button>
      </div>

      <div class="dialog-body">
        <div class="field-row">
          <div class="field">
            <label for="fName">名称</label>
            <input id="fName" v-model="name" type="text" placeholder="如：办公室 NAS" />
          </div>
          <div class="field fixed" style="flex: 0 0 auto">
            <span class="flabel">协议</span>
            <div class="segmented" id="fProtoSeg">
              <button type="button" :aria-selected="protocol === 'ssh'" @click="protocol = 'ssh'">SSH</button>
              <button type="button" :aria-selected="protocol === 'telnet'" @click="protocol = 'telnet'">
                Telnet
              </button>
            </div>
          </div>
        </div>

        <div class="field-row">
          <div class="field">
            <label for="fHost">地址</label>
            <input id="fHost" v-model="host" type="text" class="mono" placeholder="IP 或域名" />
          </div>
          <div class="field fixed">
            <label for="fPort">端口</label>
            <input id="fPort" v-model="port" type="text" class="mono" inputmode="numeric" placeholder="默认" />
          </div>
        </div>

        <div class="field">
          <label for="fUser">用户名</label>
          <input id="fUser" v-model="username" type="text" class="mono" placeholder="root" />
        </div>

        <div class="field-row">
          <div class="field fixed" style="flex: 0 0 auto">
            <span class="flabel">认证方式</span>
            <div class="segmented" id="fAuthSeg">
              <button type="button" :aria-selected="auth === 'interactive'" @click="auth = 'interactive'">
                交互
              </button>
              <button type="button" :aria-selected="auth === 'password'" @click="auth = 'password'">
                密码
              </button>
              <button type="button" :aria-selected="auth === 'key'" @click="auth = 'key'">密钥</button>
            </div>
          </div>
          <div v-show="auth === 'key'" class="field" id="fKeyField">
            <label for="fKey">选择钥匙</label>
            <select id="fKey" v-model="keyId" class="dlg-select">
              <option value="">（还没有密钥，去工具栏「密钥」生成）</option>
              <option v-for="k in keys ?? []" :key="k.id" :value="k.id">{{ k.name }}</option>
            </select>
          </div>
        </div>

        <div v-show="auth === 'password'" class="field" id="fPasswordField">
          <label for="fPassword">密码（AES-256-GCM 加密存本机，连接免输）</label>
          <input
            id="fPassword"
            v-model="password"
            type="password"
            autocomplete="off"
            :placeholder="isEdit ? '留空则不修改已存密码' : ''"
          />
        </div>

        <div class="field" id="fJumpField">
          <label for="fJump">经由跳板机（借助另一台已保存的 SSH 服务器中转：外网连内网机器）</label>
          <select id="fJump" v-model="jumpId" class="dlg-select">
            <option value="">直连（默认）</option>
            <option v-for="s in jumpOptions ?? []" :key="s.id" :value="s.id">{{ s.name }}</option>
          </select>
        </div>

        <div class="field-row">
          <div class="field">
            <label for="fTermPath">终端启动路径（登录后先 cd 过去；进不去会报错并落在家目录）</label>
            <input id="fTermPath" v-model="termPath" type="text" class="mono" placeholder="如 /data/logs，留空 = 远端家目录" />
          </div>
          <div class="field">
            <label for="fTermShell">终端 shell（留空 = 远端登录 shell）</label>
            <input
              id="fTermShell"
              v-model="termShell"
              type="text"
              class="mono"
              list="termShellPresets"
              placeholder="/bin/bash 等，或自定义命令"
            />
            <datalist id="termShellPresets">
              <option value="/bin/bash" />
              <option value="/bin/sh" />
              <option value="/bin/zsh" />
              <option value="/bin/fish" />
            </datalist>
          </div>
        </div>

        <div class="field">
          <label for="fNotes">备注</label>
          <textarea id="fNotes" v-model="notes" rows="2" placeholder="可选" />
        </div>

        <p class="field-hint">
          「交互」= 系统 ssh 在终端内要密码 / 确认指纹，凭据不落盘；「密码」= 密文落盘（AES-256-GCM），ssh
          经 askpass 免输，指纹确认仍在终端里；「密钥」= 起 ssh 带 -i（带口令的钥匙仍在终端里解锁一次）。三种方式互斥，切换即清掉另一种存的凭据。
        </p>
        <p class="test-line" id="editTestLine">{{ testLine }}</p>
        <p class="form-error" id="editError">{{ errorText }}</p>
      </div>

      <div class="dialog-foot">
        <button type="button" class="btn" id="editTest" @click="doTest">测试连接</button>
        <span class="spacer" />
        <button type="button" class="btn ghost" id="editCancel" @click="emit('close')">取消</button>
        <button type="button" class="btn primary" id="editSave" @click="submit">保存</button>
      </div>
    </div>
  </div>
</template>
