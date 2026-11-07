<script setup lang="ts">
/**
 * 通道（OpenCore / systemd-boot / Clover）的「引导器设置」内容：
 * 真实设置 = 免密模式 helper（OpenCore）+ 健康检查 / 菜单问题。
 * 供各主题列表下方折叠面板使用（列表本身由 BootList 渲染）。
 */
import { computed, ref } from "vue";
import { useToastStore } from "@/stores/toast";
import { call } from "@/api/tauri";
import { useUiStateStore } from "@/stores/uiState";

const props = defineProps<{ loader: string }>();
const ui = useUiStateStore();
const toast = useToastStore();
const busy = ref(false);
async function refreshChannel() {
  if (busy.value) return;
  busy.value = true;
  try {
    await call("refresh");
    const c = ui.channels.find((x) => x.id === props.loader) ?? null;
    const n = (c?.menuPreview?.entries?.length ?? c?.entries?.length ?? 0);
    if (n > 0) toast.show(`重新读取成功：识别到 ${n} 个条目`, { duration: 6000 });
    else toast.show("重新读取完成，但没有识别到条目", { bad: true, duration: 6000 });
  } catch (e) {
    toast.show("重新读取失败：" + e, "bad");
  } finally {
    busy.value = false;
  }
}
const ch = computed(() => ui.channels.find((c) => c.id === props.loader) ?? null);

</script>

<template>
  <!-- 通道未探测 / 无该通道数据时，也给出明确空态，不留白、不回落到别的引导器 -->
  <div v-if="!ch" class="channel-settings">
    <p class="channel-settings-empty">这个引导器没有可调设置（未探测到通道，或该通道无额外配置）</p>
  </div>
  <div v-else class="channel-settings">
    <div class="channel-settings-head" style="display:flex;align-items:center;gap:8px;margin-bottom:6px">
      <span class="grow" style="flex:1"></span>
      <button type="button" class="secondary" :disabled="busy" @click="refreshChannel">
        {{ busy ? "读取中…" : "重新读取" }}
      </button>
    </div>
    <!-- 健康检查 / 菜单问题（真实 issues） -->
    <div v-if="(ch.health?.length ?? 0) || (ch.menuPreview?.issues?.length ?? 0)" class="oc-menu-issues">
      <small v-for="(h, i) in [...(ch.health ?? []), ...(ch.menuPreview?.issues ?? [])]" :key="i" class="issue" :data-sev="h.severity">{{ h.text }}</small>
    </div>
    <p v-if="!((ch.health?.length ?? 0) || (ch.menuPreview?.issues?.length ?? 0))" class="channel-settings-empty">
      {{ ch.detail || "没有额外的可调设置" }}
    </p>
  </div>
</template>
