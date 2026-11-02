<script setup lang="ts">
/**
 * files 独立窗（原 `ui/hub-files.html`）。
 *
 * 真实实现在 `FilesWindow.vue`；这里只做转出，保证入口文件稳定
 * （`entries/hub-files.ts` 指向它）。
 */
import FilesWindow from "./FilesWindow.vue";
</script>

<template>
  <FilesWindow />
</template>
