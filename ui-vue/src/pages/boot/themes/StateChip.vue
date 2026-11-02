<script setup lang="ts">
import { computed } from "vue";

/** 状态胶囊：已检测免提权 / 已检测需提权 / 未检测到 */
const props = defineProps<{ state: "ok" | "warn" | "off" }>();

const text = computed(() => {
  switch (props.state) {
    case "warn":
      return "已检测 · 需提权";
    case "off":
      return "未检测到";
    default:
      return "已检测 · 免提权";
  }
});
</script>

<template>
  <span :class="['chip', state]">{{ text }}</span>
</template>
