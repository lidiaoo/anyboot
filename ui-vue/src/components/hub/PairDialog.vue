<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import { call } from "@/api/tauri";

/**
 * 生成配对码弹层（原 `ui/hub.html` 的 `#gwPairOverlay`，3256 行）。
 *
 * ## 1:1 结构
 * ```
 * .token-modal-overlay > .token-modal.gw-pair-modal
 *   button.gw-pair-x        右上角常驻 ✕（用户不想生成时必须能走）
 *   h2                      标题
 *   p.gw-pair-warn          「等同密码」警告
 *   .field#gwPairNameRow    客户端名称
 *   .field#gwPairAddrRow    配对串带哪个地址（.segmented 5 档）
 *   #gwPairGoRow            生成按钮
 *   #gwPairResult           .gw-qr 二维码 + textarea + 复制/完成
 * ```
 */

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ (e: "close"): void }>();

type AddrKind = "tunnel" | "tailscale" | "funnel" | "tsnet" | "none";

const clientName = ref("");
const addrKind = ref<AddrKind>("tunnel");
const busy = ref(false);
const errorText = ref("");
/** 生成结果 */
const resultText = ref("");
const qrRef = ref<HTMLElement | null>(null);

const ADDR_TABS: { key: AddrKind; label: string; hint: string }[] = [
  { key: "tunnel", label: "隧道地址", hint: "隧道地址 = 本机 Cloudflare 通道的当前地址；没开启通道则自动退化为「不带地址」。" },
  { key: "tailscale", label: "Tailscale 地址", hint: "用系统 Tailscale 的 100.x 地址（需要两边在同一 tailnet）。" },
  { key: "funnel", label: "Funnel 地址", hint: "用 Tailscale Funnel 暴露到公网的地址。" },
  { key: "tsnet", label: "内置 TS 地址", hint: "用内置 tsnet 引擎的地址（不需要安装系统 Tailscale）。" },
  { key: "none", label: "不带地址", hint: "配对串里不带地址，客户端要自己填。" },
];

const currentHint = ref(ADDR_TABS[0]!.hint);

watch(addrKind, (kind) => {
  currentHint.value = ADDR_TABS.find((t) => t.key === kind)?.hint ?? "";
});

/** 二维码库按需加载（public/qrcode.min.js，不进打包产物） */
function loadQrCode(): Promise<void> {
  const w = window as unknown as { QRCode?: unknown };
  if (w.QRCode) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const el = document.createElement("script");
    el.src = "./qrcode.min.js";
    el.onload = () => resolve();
    el.onerror = () => reject(new Error("二维码库加载失败"));
    document.head.appendChild(el);
  });
}

async function generate() {
  if (busy.value) return;
  busy.value = true;
  errorText.value = "";
  try {
    const text = await call<string>("gateway_pair_issue", {
      name: clientName.value.trim(),
      kind: addrKind.value,
      tunnel: addrKind.value === "tunnel",
    });
    resultText.value = text || "";
    await renderQr();
  } catch (error) {
    errorText.value = String(error);
  } finally {
    busy.value = false;
  }
}

async function renderQr() {
  if (!resultText.value || !qrRef.value) return;
  try {
    await loadQrCode();
    const QR = (window as unknown as { QRCode?: { toCanvas: Function } }).QRCode;
    if (!QR) return;
    qrRef.value.innerHTML = "";
    const canvas = document.createElement("canvas");
    qrRef.value.appendChild(canvas);
    // qrcode.min.js 的 toCanvas 签名：toCanvas(canvas, text, opts, callback)
    QR.toCanvas(canvas, resultText.value, { width: 200, margin: 1 }, () => {});
  } catch {
    /* 二维码失败不影响复制配对串 */
  }
}

async function copyPair() {
  try {
    await navigator.clipboard.writeText(resultText.value);
  } catch {
    /* 剪贴板不可用 */
  }
}

onMounted(() => {
  if (props.open) void renderQr();
});
</script>

<template>
  <div v-if="open" id="gwPairOverlay" class="token-modal-overlay" @click.self="emit('close')">
    <div class="token-modal gw-pair-modal">
      <button type="button" id="gwPairX" class="gw-pair-x" title="关闭（不生成）" aria-label="关闭（不生成）" @click="emit('close')">
        ✕
      </button>

      <h2 id="gwPairTitle">生成配对码</h2>
      <p class="gw-pair-warn">等同密码：拿到它就能访问这台电脑，不要转发给别人。</p>

      <template v-if="!resultText">
        <div class="field" id="gwPairNameRow">
          <label for="gwPairName">客户端名称（谁来配对）</label>
          <input
            id="gwPairName"
            v-model="clientName"
            type="text"
            placeholder="例如：我的手机 / 客厅笔记本"
            maxlength="40"
          />
        </div>

        <div class="field" id="gwPairAddrRow">
          <label>配对串带哪个地址</label>
          <div class="segmented" id="gwPairAddrTabs" role="tablist" aria-label="地址类型">
            <button
              v-for="tab in ADDR_TABS"
              :key="tab.key"
              type="button"
              role="tab"
              :data-addr="tab.key"
              :aria-selected="addrKind === tab.key"
              @click="addrKind = tab.key"
            >
              {{ tab.label }}
            </button>
          </div>
          <p class="field-hint" id="gwPairAddrHint" style="margin: 6px 0 0">{{ currentHint }}</p>
        </div>

        <div id="gwPairGoRow" style="display: flex; gap: 8px; justify-content: flex-end">
          <button type="button" id="gwPairGo" class="btn primary" :disabled="busy" @click="generate">
            {{ busy ? "生成中…" : "生成配对码" }}
          </button>
        </div>
      </template>

      <div v-else id="gwPairResult">
        <div ref="qrRef" id="gwPairQr" class="gw-qr" />
        <textarea id="gwPairText" v-model="resultText" class="mono" readonly rows="3" spellcheck="false" />
        <div style="display: flex; gap: 8px; justify-content: flex-end; margin-top: 12px">
          <button type="button" id="gwPairCopy" class="btn" @click="copyPair">复制配对串</button>
          <button type="button" id="gwPairClose" class="btn primary" @click="emit('close')">完成</button>
        </div>
      </div>

      <p v-if="errorText" class="field-hint" style="color: rgb(var(--bad))">{{ errorText }}</p>
    </div>
  </div>
</template>
