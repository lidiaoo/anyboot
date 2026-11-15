<script setup lang="ts">
import { onMounted, ref, watch , computed , nextTick } from "vue";
import { t } from "@/i18n";
import { call } from "@/api/tauri";
import { useEscape } from "@/composables/useEscape";

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
useEscape(computed(() => props.open), () => emit('close'));

type AddrKind = "tunnel" | "tailscale" | "funnel" | "tsnet" | "none";

const clientName = ref("");
const addrKind = ref<AddrKind>("tunnel");
const busy = ref(false);
const errorText = ref("");
/** 生成结果 */
const resultText = ref("");
const qrRef = ref<HTMLElement | null>(null);

const ADDR_TABS: { key: AddrKind; label: string; hint: string }[] = [
  { key: "tunnel", label: "pair.kind.tunnel", hint: "pair.hint.tunnel" },
  { key: "tailscale", label: "pair.kind.ts", hint: "pair.hint.ts" },
  { key: "funnel", label: "pair.kind.funnel", hint: "pair.hint.funnel" },
  { key: "tsnet", label: "pair.kind.tsnet", hint: "pair.hint.tsnet" },
  { key: "none", label: "pair.kind.none", hint: "pair.hint.none" },
];

const currentHint = ref(ADDR_TABS[0]!.hint);

watch(addrKind, (kind) => {
  currentHint.value = ADDR_TABS.find((t) => t.key === kind)?.hint ?? "";
});

/** 二维码库按需加载（public/qrcode.min.js，不进打包产物） */
function loadQrCode(): Promise<void> {
  const w = window as unknown as { qrcode?: unknown };
  if (w.qrcode) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const el = document.createElement("script");
    el.src = "./qrcode.min.js";
    el.onload = () => resolve();
    el.onerror = () => reject(new Error(t("pair.qrLoadFail")));
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
    // 等结果分支（v-else 含 qrRef 容器）挂载后再画二维码
    await nextTick();
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
    // 旧版 drawQr：qrcode-generator（window.qrcode），白底黑点，静区 2 模块
    const qrcode = (window as unknown as {
      qrcode?: (typeNumber: number, errorCorrectionLevel: string) => {
        addData: (data: string) => void;
        make: () => void;
        getModuleCount: () => number;
        isDark: (row: number, col: number) => boolean;
      };
    }).qrcode;
    if (!qrcode) return;
    const qr = qrcode(0, "M");
    qr.addData(resultText.value);
    qr.make();
    const count = qr.getModuleCount();
    const cell = 4, margin = 2;
    const size = (count + margin * 2) * cell;
    const canvas = document.createElement("canvas");
    canvas.width = size;
    canvas.height = size;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(0, 0, size, size);
    ctx.fillStyle = "#000000";
    for (let row = 0; row < count; row++) {
      for (let col = 0; col < count; col++) {
        if (qr.isDark(row, col)) {
          ctx.fillRect((col + margin) * cell, (row + margin) * cell, cell, cell);
        }
      }
    }
    qrRef.value.innerHTML = "";
    qrRef.value.appendChild(canvas);
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
      <button type="button" id="gwPairX" class="gw-pair-x" :title="t('pair.closeNoGen')" :aria-label="t('pair.closeNoGen')" @click="emit('close')">
        ✕
      </button>

      <h2 id="gwPairTitle">{{ t("pair.title") }}</h2>
      <p class="gw-pair-warn">{{ t("pair.warn") }}</p>

      <template v-if="!resultText">
        <div class="field" id="gwPairNameRow">
          <label for="gwPairName">{{ t("pair.nameLabel") }}</label>
          <input
            id="gwPairName"
            v-model="clientName"
            type="text"
            :placeholder="t('pair.namePh')"
            maxlength="40"
          />
        </div>

        <div class="field" id="gwPairAddrRow">
          <label>{{ t("pair.addrLabel") }}</label>
          <div class="segmented" id="gwPairAddrTabs" role="tablist" :aria-label="t('pair.addrAria')">
            <button
              v-for="tab in ADDR_TABS"
              :key="tab.key"
              type="button"
              role="tab"
              :data-addr="tab.key"
              :aria-selected="addrKind === tab.key"
              @click="addrKind = tab.key"
            >
              {{ t(tab.label) }}
            </button>
          </div>
          <p class="field-hint" id="gwPairAddrHint" style="margin: 6px 0 0">{{ currentHint }}</p>
        </div>

        <div id="gwPairGoRow" style="display: flex; gap: 8px; justify-content: flex-end">
          <button type="button" id="gwPairGo" class="btn primary" :disabled="busy" @click="generate">
            {{ busy ? t("pair.generating") : t("pair.generate") }}
          </button>
        </div>
      </template>

      <div v-else id="gwPairResult">
        <div ref="qrRef" id="gwPairQr" class="gw-qr" />
        <textarea id="gwPairText" v-model="resultText" class="mono" readonly rows="3" spellcheck="false" />
        <div style="display: flex; gap: 8px; justify-content: flex-end; margin-top: 12px">
          <button type="button" id="gwPairCopy" class="btn" @click="copyPair">{{ t("pair.copy") }}</button>
          <button type="button" id="gwPairClose" class="btn primary" @click="emit('close')">{{ t("pair.done") }}</button>
        </div>
      </div>

      <p v-if="errorText" class="field-hint" style="color: rgb(var(--bad))">{{ errorText }}</p>
    </div>
  </div>
</template>
