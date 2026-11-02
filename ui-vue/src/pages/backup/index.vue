<script setup lang="ts">
import BackupPage from "@/pages/hub/BackupPage.vue";

/**
 * 备份窗（原 `ui/hub-backup.html`，1057 行）—— backup 命令族。
 *
 * 复用 hub 内嵌的 `BackupPage`：同一套后端命令，独立窗只是整窗形态。
 */
</script>

<template>
  <div class="backup-window">
    <BackupPage />
  </div>
</template>

<style scoped>
.backup-window {
  height: 100vh;
  overflow-y: auto;
  padding: 18px;
}
</style>
