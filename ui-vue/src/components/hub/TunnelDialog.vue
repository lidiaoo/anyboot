<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { RemoteProfile } from "@/stores/remote";

/**
 * 隧道编辑弹层（原 `ui/hub.html` 的 `#tunnelOverlay`，4732 行）。
 *
 * ## 1:1 结构
 * ```
 * .overlay > .dialog
 *   .dialog-head > h3#tunnelTitle + button.icon-btn
 *   .dialog-body
 *     .field 名称
 *     .field-row > .field 类型（本地转发 / SOCKS5 动态转发）+ .field.fixed 本地端口
 *     #tTargetRow > .field 目标主机 + .field.fixed 目标端口（动态转发时隐藏）
 *     p.field-hint（两种转发方式说明）
 *     p.form-error
 *   .dialog-foot > .spacer + 取消 + 保存
 * ```
 */

const props = defineProps<{
  open: boolean;
  /** 编辑已有隧道时传入 */
  tunnel?: { id?: string; name: string; kind: string; localPort: number; targetHost: string; targetPort: number } | null;
  /** 当前电脑（隧道挂在它身上） */
  server?: RemoteProfile | null;
}>();

const emit = defineEmits<{
  (e: "save", payload: Record<string, unknown>): void;
  (e: "close"): void;
}>();

const name = ref("");
const kind = ref<"local" | "dynamic">("local");
const localPort = ref("");
const targetHost = ref("127.0.0.1");
const targetPort = ref("");
const errorText = ref("");

const isEdit = computed(() => Boolean(props.tunnel?.id));
const showTarget = computed(() => kind.value === "local");

watch(
  () => props.tunnel,
  (tunnel) => {
    if (!tunnel) {
      name.value = "";
      kind.value = "local";
      localPort.value = "";
      targetHost.value = "127.0.0.1";
      targetPort.value = "";
      errorText.value = "";
      return;
    }
    name.value = tunnel.name;
    kind.value = tunnel.kind === "dynamic" ? "dynamic" : "local";
    localPort.value = String(tunnel.localPort || "");
    targetHost.value = tunnel.targetHost || "127.0.0.1";
    targetPort.value = String(tunnel.targetPort || "");
    errorText.value = "";
  },
  { immediate: true },
);

function submit() {
  errorText.value = "";
  if (!name.value.trim()) {
    errorText.value = "名称不能为空";
    return;
  }
  const port = Number(localPort.value);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    errorText.value = "本地端口需为 1–65535";
    return;
  }
  if (showTarget.value) {
    const tport = Number(targetPort.value);
    if (!Number.isInteger(tport) || tport <= 0 || tport > 65535) {
      errorText.value = "目标端口需为 1–65535";
      return;
    }
  }
  emit("save", {
    serverId: props.server?.id ?? "",
    id: props.tunnel?.id ?? "",
    name: name.value.trim(),
    kind: kind.value,
    localPort: port,
    targetHost: showTarget.value ? targetHost.value.trim() : "",
    targetPort: showTarget.value ? Number(targetPort.value) : 0,
  });
}
</script>

<template>
  <div v-if="open" class="overlay">
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="tunnelTitle">
      <div class="dialog-head">
        <h3 id="tunnelTitle">{{ isEdit ? "编辑隧道" : "新建隧道" }}</h3>
        <button type="button" class="icon-btn" aria-label="关闭" @click="emit('close')">✕</button>
      </div>

      <div class="dialog-body">
        <div class="field">
          <label for="tName">名称</label>
          <input id="tName" v-model="name" type="text" placeholder="如：内网 Web" />
        </div>

        <div class="field-row">
          <div class="field">
            <label for="tKind">类型</label>
            <select id="tKind" v-model="kind" class="dlg-select">
              <option value="local">本地转发</option>
              <option value="dynamic">SOCKS5 动态转发</option>
            </select>
          </div>
          <div class="field fixed">
            <label for="tLocalPort">本地端口</label>
            <input id="tLocalPort" v-model="localPort" type="text" class="mono" inputmode="numeric" placeholder="8080" />
          </div>
        </div>

        <div v-if="showTarget" class="field-row">
          <div class="field">
            <label for="tTargetHost">目标主机（从服务器侧看）</label>
            <input id="tTargetHost" v-model="targetHost" type="text" class="mono" placeholder="127.0.0.1" />
          </div>
          <div class="field fixed" style="flex: 0 0 110px">
            <label for="tTargetPort">目标端口</label>
            <input id="tTargetPort" v-model="targetPort" type="text" class="mono" inputmode="numeric" placeholder="80" />
          </div>
        </div>

        <p class="field-hint">
          本地转发：访问本机端口 = 经该服务器访问目标；动态转发：本机端口变成 SOCKS5 代理。
        </p>
        <p class="form-error">{{ errorText }}</p>
      </div>

      <div class="dialog-foot">
        <span class="spacer" />
        <button type="button" class="btn ghost" @click="emit('close')">取消</button>
        <button type="button" class="btn primary" @click="submit">保存</button>
      </div>
    </div>
  </div>
</template>
