<script setup lang="ts">
import { computed } from "vue";
import { t } from "@/i18n";
import { useEscape } from "@/composables/useEscape";
import type { RemoteProfile } from "@/stores/remote";

/**
 * 服务端列表弹窗（原 `ui/hub.html` 的 `#rsListOverlay`，2706 行）。
 *
 * ```
 * .overlay > .dialog
 *   .dialog-head > h3#rsListTitle + button.icon-btn
 *   .dialog-body  （每个服务端一行：名称 + 地址 + 编辑/删除）
 * ```
 */

const props = defineProps<{
  open: boolean;
  servers: RemoteProfile[];
}>();

useEscape(computed(() => props.open), () => emit("close"));
const emit = defineEmits<{
  (e: "edit", id: string): void;
  (e: "remove", id: string): void;
  (e: "add"): void;
  (e: "close"): void;
}>();
</script>

<template>
  <div v-if="open" id="rsListOverlay" class="overlay">
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="rsListTitle">
      <div class="dialog-head">
        <h3 id="rsListTitle">{{ t("server.list") }}</h3>
        <button type="button" class="icon-btn" id="rsListClose" :aria-label="t('common.close')" @click="emit('close')">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true">
            <path d="M6 6l12 12M18 6L6 18" />
          </svg>
        </button>
      </div>

      <div class="dialog-body">
        <div id="rsListRows">
        <div v-for="server in servers" :key="server.id" class="rs-list-row">
          <span class="nm">{{ server.name }}</span>
          <span class="mono addr">{{ (server.addresses ?? []).join(" / ") || t("server.noAddr") }}</span>
          <span class="spacer" />
          <button type="button" class="link-btn" @click="emit('edit', server.id)">{{ t("common.edit") }}</button>
          <button type="button" class="link-btn" @click="emit('remove', server.id)">{{ t("common.delete") }}</button>
        </div>
        </div>
        <p v-if="!servers.length" class="field-hint">{{ t("server.listEmpty") }}</p>
        <button type="button" class="secondary" @click="emit('add')">{{ t("server.listAdd") }}</button>
        <p class="field-hint">{{ t("server.listHint") }}</p>
      </div>

      <div class="dialog-foot">
        <span class="spacer" />
        <button type="button" class="btn" @click="emit('close')">{{ t("common.close") }}</button>
      </div>
    </div>
  </div>
</template>
