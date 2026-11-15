<script setup lang="ts">
const toast = useToastStore();
import { computed, onMounted, ref } from "vue";
import { t } from "@/i18n";
import { call, isTauri } from "@/api/tauri";
import { useToastStore } from "@/stores/toast";

/**
 * 密钥页（原 `ui/hub.html` 的 `#page-keys`，3446 行）。
 *
 * 1:1 结构（就 4 个元素）：
 * ```
 * .act-card
 *   svg（钥匙图标）
 *   p.t  「密钥」
 *   p.s  #keysPageHint（密钥数量提示）
 *   button.btn.primary  #keysBtn「打开密钥管理」（带钥匙图标）
 * ```
 * 真正的密钥管理在独立窗（`hub-keys.html`），这里只是入口。
 */

interface KeyRow {
  id: string;
  name: string;
}

const keys = ref<KeyRow[]>([]);
const loading = ref(true);

/** 提示文案（逐字对齐旧版 renderKeysPage，hub.html:6898） */
const keysHint = computed(() => {
  if (loading.value) return t("keys.loading");
  if (!keys.value.length) return t("keys.empty");
  return t("keys.summary", { n: keys.value.length, ids: keys.value.map((k) => k.id).join(", ") });
});

onMounted(async () => {
  try {
    keys.value = (await call<KeyRow[]>("hub_keys_list")) ?? [];
  } catch {
    keys.value = [];
  } finally {
    loading.value = false;
  }
});

function openKeys() {
  if (!isTauri()) { toast.show(t("keys.needApp"), "bad"); return; }
  void call("hub_open_keys_window", {}).catch((e) => toast.show("打开密钥管理失败：" + e, "bad"));
}
</script>

<template>
  <div class="hub-page">
    <div class="page-head"><h2>{{ $t("nav.keys") }}</h2></div>
    <div class="hub-page-body">
      <div class="act-card">
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.6"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <circle cx="8" cy="15" r="4" />
          <path d="M10.8 12.2 20 3M17 6l2.5 2.5" />
        </svg>
        <p class="t">{{ $t("nav.keys") }}</p>
        <p class="s" id="keysPageHint">{{ keysHint }}</p>
        <button type="button" id="keysBtn" class="btn primary" @click="openKeys">
          <svg
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <circle cx="8" cy="15" r="4" />
            <path d="M10.8 12.2 20 3M17 6l2.5 2.5" />
          </svg>
          {{ $t("keys.open") }}
        </button>
      </div>
    </div>
  </div>
</template>
