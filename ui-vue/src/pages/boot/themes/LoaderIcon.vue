<script setup lang="ts">
import { computed } from "vue";
import { loaderMeta } from "./loaders";

/** 引导器图标（`.ldr`）：圆形底 + 两个字母，颜色按引导器区分 */
const props = withDefaults(defineProps<{ id: string; size?: "sm" | "md" }>(), {
  size: "md",
});

const meta = computed(() => loaderMeta(props.id));
</script>

<template>
  <span :class="['ldr', size]" :style="{ '--c': meta.color }">{{ meta.mono }}</span>
</template>
