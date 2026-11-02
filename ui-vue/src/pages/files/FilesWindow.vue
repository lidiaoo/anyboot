<script setup lang="ts">
import FilesPage from "@/pages/hub/FilesPage.vue";

/**
 * 文件窗（原 `ui/hub-files.html`，1262 行）—— hub_sftp 命令族。
 *
 * 直接复用 hub 内嵌的 `FilesPage`：两者是同一套后端命令与交互，
 * 独立窗只是把它放大到整窗（旧版也是这么做的第二入口）。
 */
</script>

<template>
  <div class="files-window">
    <FilesPage />
  </div>
</template>

<style scoped>
.files-window {
  height: 100vh;
  overflow: hidden;
}
</style>
