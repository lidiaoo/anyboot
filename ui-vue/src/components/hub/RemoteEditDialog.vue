<script setup lang="ts">
import { ref, watch } from "vue";
import type { RemoteProfile } from "@/stores/remote";

/**
 * 添加 / 编辑 AnyBoot 服务端弹窗（原 `ui/hub.html` 的 `#rsEditOverlay`，2533 行）。
 *
 * 注意：这是**电脑（AnyBoot 服务端）**，不是 SSH 服务器（那个是 `#editOverlay`）。
 *
 * ## 1:1 结构
 * ```
 * .overlay > .dialog.rs-edit
 *   .dialog-head > h3#rsEditTitle + button.icon-btn
 *   .dialog-body
 *     .field 名称#rsName
 *     .field 地址快速识别（#rsAddrQuick + #rsAddrRecognize + #rsAddrHint）
 *     .field 地址列表（#rsAddrList，每行 host:port + MAC 多值）
 *   .dialog-foot > 测试 + 取消 + 保存
 * ```
 */

const props = defineProps<{
  open: boolean;
  remote?: RemoteProfile | null;
}>();

const emit = defineEmits<{
  (e: "save", payload: Record<string, unknown>): void;
  (e: "close"): void;
}>();

const name = ref("");
const quick = ref("");
const hint = ref("");
/** 地址行：[{ addr, macs }] */
const rows = ref<{ addr: string; macs: string }[]>([{ addr: "", macs: "" }]);

watch(
  () => props.remote,
  (r) => {
    if (!r) {
      name.value = "";
      quick.value = "";
      hint.value = "";
      rows.value = [{ addr: "", macs: "" }];
      return;
    }
    name.value = r.name;
    // 把 addresses + macs 摊平成行
    const addrs = r.addresses ?? [];
    const macs = (r.macs ?? {}) as Record<string, string[]>;
    rows.value = addrs.length
      ? addrs.map((addr) => ({ addr, macs: (macs[addr] ?? []).join(", ") }))
      : [{ addr: "", macs: "" }];
  },
  { immediate: true },
);

/** 地址快速识别：粘贴一整条 host:port 或带 token 的 URL */
function recognize() {
  const text = quick.value.trim();
  if (!text) return;
  const match = text.match(/(\d+\.\d+\.\d+\.\d+(?::\d+)?)/);
  if (!match) {
    hint.value = "没识别出地址，请手动填写";
    return;
  }
  const addr = match[1]!;
  const exists = rows.value.some((r) => r.addr === addr);
  if (exists) {
    hint.value = "这条地址已经加过了";
    return;
  }
  const empty = rows.value.find((r) => !r.addr);
  if (empty) empty.addr = addr;
  else rows.value.push({ addr, macs: "" });
  hint.value = "已加入地址列表";
  quick.value = "";
}

function addRow() {
  rows.value.push({ addr: "", macs: "" });
}

function removeRow(index: number) {
  rows.value = rows.value.filter((_, i) => i !== index);
  if (!rows.value.length) rows.value = [{ addr: "", macs: "" }];
}

function submit() {
  const addresses: string[] = [];
  const macs: Record<string, string[]> = {};
  for (const row of rows.value) {
    const addr = row.addr.trim();
    if (!addr) continue;
    addresses.push(addr);
    const list = row.macs
      .split(/[,;\s]+/)
      .map((m) => m.trim())
      .filter(Boolean);
    if (list.length) macs[addr] = list;
  }
  emit("save", {
    id: props.remote?.id ?? "",
    name: name.value.trim(),
    addresses,
    macs,
  });
}
</script>

<template>
  <div v-if="open" class="overlay">
    <div class="dialog rs-edit" role="dialog" aria-modal="true" aria-labelledby="rsEditTitle">
      <div class="dialog-head">
        <h3 id="rsEditTitle">{{ remote?.id ? "编辑服务端" : "连接 AnyBoot 服务端" }}</h3>
        <button type="button" class="icon-btn" aria-label="关闭" @click="emit('close')">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>

      <div class="dialog-body">
        <div class="field">
          <label for="rsName">名称</label>
          <input id="rsName" v-model="name" type="text" placeholder="如：客厅 PC / 办公室 Mac" />
        </div>

        <div class="field">
          <label for="rsAddrQuick">粘贴地址快速识别</label>
          <input
            id="rsAddrQuick"
            v-model="quick"
            type="text"
            class="mono"
            placeholder="粘一整条 host:port 或带 token 的 URL，回车自动拆分"
            @keydown.enter.prevent="recognize"
          />
          <div class="actions">
            <button type="button" id="rsAddrRecognize" class="secondary" @click="recognize">识别并添加</button>
          </div>
          <p class="field-hint" id="rsAddrHint">{{ hint }}</p>
        </div>

        <div class="field">
          <label>地址列表（每条 = 一个转发端点或直连地址）</label>
          <div id="rsAddrList" class="addr-list">
            <div v-for="(row, i) in rows" :key="i" class="addr-row">
              <input v-model="row.addr" type="text" class="mono" placeholder="127.0.0.1:12300" spellcheck="false" />
              <input v-model="row.macs" type="text" class="mono" placeholder="MAC（多个用逗号分隔，可留空）" spellcheck="false" />
              <button type="button" class="icon-btn" aria-label="删除这一行" @click="removeRow(i)">✕</button>
            </div>
            <button type="button" class="secondary" @click="addRow">＋ 添加一行</button>
          </div>
        </div>
      </div>

      <div class="dialog-foot">
        <span class="spacer" />
        <button type="button" class="btn ghost" @click="emit('close')">取消</button>
        <button type="button" class="btn primary" @click="submit">保存</button>
      </div>
    </div>
  </div>
</template>
