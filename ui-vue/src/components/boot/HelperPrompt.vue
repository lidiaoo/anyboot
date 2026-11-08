<script setup lang="ts">
/**
 * 「首次启动扫描需要免密助手」的 App 内提示弹窗。
 *
 * 背景：启动「管理员扫描」要读 Linux 分区裸设备（ext4 内核清单/grub.cfg），
 * macOS 上这需要 root —— 首选装免密助手（一次授权，之后全程无感）。
 *
 * 交互（严格按用户定案 2026-11-07）：
 *  - 用户【开启免密】→ 走系统授权安装助手 → 装好后由调用方静默扫描，**绝不再弹系统框**。
 *  - 用户【取消提醒】→ 选 1 天/7 天/30 天/永久，记下到期时间，本次不扫。
 *  - 用户【暂不】→ 本次不扫。
 *  任何选择都要**先等这里操作完毕**，调用方再决定是否降级到密码授权框。
 */
import { ref } from "vue";
import { call } from "@/api/tauri";
import { setHelperReminderSnooze } from "@/api/tauri";

const emit = defineEmits<{
  (e: "resolved", action: "installed" | "snoozed" | "cancelled"): void;
}>();

const busy = ref(false);
const error = ref("");

async function install() {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  try {
    await call("boot_helper", { action: "install" });
    emit("resolved", "installed");
  } catch (e) {
    error.value = "开启免密助手失败（可能取消了系统授权）：" + e;
  } finally {
    busy.value = false;
  }
}

async function snooze(days: number) {
  // days=0 表示永久（用一个很远的未来）
  const until = days === 0 ? 4_102_444_800 : Math.floor(Date.now() / 1000) + days * 86400;
  try {
    await setHelperReminderSnooze(until);
  } catch {
    /* 记录失败也不拦，本次就当取消 */
  }
  emit("resolved", "snoozed");
}

function cancel() {
  emit("resolved", "cancelled");
}
</script>

<template>
  <div class="token-modal-overlay" style="z-index: 200">
    <div class="token-modal" role="dialog" aria-modal="true" aria-label="开启免密助手">
      <h2>开启免密扫描</h2>
      <p>
        启动时扫描 Linux 分区的引导项/内核，需要读取分区裸数据（需要管理员权限）。
        建议开启「免密提权（root 助手）」：只需授权这一次，之后每次启动都能自动扫描，
        不再反复弹密码框。
      </p>
      <p v-if="error" class="error" style="color: #d9534f; font-size: 12px">{{ error }}</p>

      <div style="display: flex; flex-direction: column; gap: 8px; margin-top: 6px">
        <button type="button" class="btn" :disabled="busy" @click="install">
          {{ busy ? "授权中…" : "开启免密助手（输一次密码）" }}
        </button>

        <!-- 取消提醒：直接展示 1/7/30 天/永久（不再折叠成「取消提醒…」按钮） -->
        <small style="font-size: 12px; color: var(--text-faint)">在下列时间内不再提醒：</small>
        <div style="display: flex; gap: 8px; flex-wrap: wrap">
          <button type="button" class="btn sm" :disabled="busy" @click="snooze(1)">1 天</button>
          <button type="button" class="btn sm" :disabled="busy" @click="snooze(7)">7 天</button>
          <button type="button" class="btn sm" :disabled="busy" @click="snooze(30)">30 天</button>
          <button type="button" class="btn sm" :disabled="busy" @click="snooze(0)">永久</button>
        </div>
        <button type="button" class="btn ghost" :disabled="busy" @click="cancel">暂不</button>
      </div>
    </div>
  </div>
</template>
