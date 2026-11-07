<script setup lang="ts">
/**
 * 通道（OpenCore / systemd-boot / Clover）的「引导器设置」内容：
 * 真实设置 = 免密模式 helper（OpenCore）+ 健康检查 / 菜单问题。
 * 供各主题列表下方折叠面板使用（列表本身由 BootList 渲染）。
 */
import { computed } from "vue";
import { useUiStateStore } from "@/stores/uiState";

const props = defineProps<{ loader: string }>();
const ui = useUiStateStore();
const ch = computed(() => ui.channels.find((c) => c.id === props.loader) ?? null);

</script>

<template>
  <!-- 通道未探测 / 无该通道数据时，也给出明确空态，不留白、不回落到别的引导器 -->
  <div v-if="!ch" class="channel-settings">
    <p class="channel-settings-empty">这个引导器没有可调设置（未探测到通道，或该通道无额外配置）</p>
  </div>
  <div v-else class="channel-settings">
    <!-- 健康检查 / 菜单问题（真实 issues） -->
    <div v-if="(ch.health?.length ?? 0) || (ch.menuPreview?.issues?.length ?? 0)" class="oc-menu-issues">
      <small v-for="(h, i) in [...(ch.health ?? []), ...(ch.menuPreview?.issues ?? [])]" :key="i" class="issue" :data-sev="h.severity">{{ h.text }}</small>
    </div>
    <p v-if="!((ch.health?.length ?? 0) || (ch.menuPreview?.issues?.length ?? 0))" class="channel-settings-empty">
      {{ ch.detail || "没有额外的可调设置" }}
    </p>
  </div>
</template>
