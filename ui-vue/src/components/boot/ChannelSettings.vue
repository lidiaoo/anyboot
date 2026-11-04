<script setup lang="ts">
/**
 * 通道（OpenCore / systemd-boot / Clover）的「引导器设置」内容：
 * 真实设置 = 免密模式 helper（OpenCore）+ 健康检查 / 菜单问题。
 * 供各主题列表下方折叠面板使用（列表本身由 BootList 渲染）。
 */
import { computed, ref } from "vue";
import { useUiStateStore } from "@/stores/uiState";

const props = defineProps<{ loader: string }>();
const ui = useUiStateStore();
const ch = computed(() => ui.channels.find((c) => c.id === props.loader) ?? null);
const helperBusy = ref(false);

async function toggleBootHelper(action: "install" | "uninstall") {
  if (helperBusy.value) return;
  helperBusy.value = true;
  try {
    const { call } = await import("@/api/tauri");
    await call("boot_helper", { action });
  } catch {
    /* 状态由下次刷新带回 */
  } finally {
    helperBusy.value = false;
  }
}
</script>

<template>
  <div v-if="ch" class="channel-settings">
    <!-- 免密模式 helper（OpenCore） -->
    <div
      v-if="props.loader === 'opencore' && ui.state?.helper && ui.state.helper.supported"
      class="channel-helper"
      :data-state="ui.state.helper.installed ? (ui.state.helper.stale ? 'stale' : 'on') : 'off'"
    >
      <span class="channel-helper-label">免密模式</span>
      <small class="channel-helper-text">{{ ui.state.helper.text }}</small>
      <button
        type="button"
        class="channel-helper-action needs-elev"
        :disabled="helperBusy"
        @click="toggleBootHelper(ui.state.helper.installed && !ui.state.helper.stale ? 'uninstall' : 'install')"
      >{{ ui.state.helper.installed && ui.state.helper.stale ? '重装 ⤴' : ui.state.helper.installed ? '关闭' : '开启 ⤴' }}</button>
    </div>
    <!-- 健康检查 / 菜单问题（真实 issues） -->
    <div v-if="(ch.health?.length ?? 0) || (ch.menuPreview?.issues?.length ?? 0)" class="oc-menu-issues">
      <small v-for="(h, i) in [...(ch.health ?? []), ...(ch.menuPreview?.issues ?? [])]" :key="i" class="issue" :data-sev="h.severity">{{ h.text }}</small>
    </div>
    <p v-if="!((ch.health?.length ?? 0) || (ch.menuPreview?.issues?.length ?? 0)) && !(props.loader === 'opencore' && ui.state?.helper?.supported)" class="channel-settings-empty">
      {{ ch.detail || "没有额外的可调设置" }}
    </p>
  </div>
</template>
