<script setup lang="ts">
/**
 * GRUB「ESP 分区」共享表单（自动读取与手动解析共用）——从 GrubSection 剥离，
 * 由 LoaderSettings 在 grub 分支独立渲染（修复 G 主题 slot 场景下表单渲染为空的问题）。
 */
import { computed, ref } from "vue";
import { call } from "@/api/tauri";
import { useUiStateStore } from "@/stores/uiState";
import { useToastStore } from "@/stores/toast";
import { useSettingsStore } from "@/stores/settings";

const ui = useUiStateStore();
const toast = useToastStore();
const settings = useSettingsStore();
/** 手动模式才显示「识别」按钮（识别=解析 ESP 上的 grub.cfg，仅手动解析用得到） */
const isManual = computed(() => settings.grubMode === "manual");
const grubManual = computed(() => ui.grub);
const espDir = computed(() => grubManual.value?.efiDir ?? "");
const partuuid = computed(() => grubManual.value?.partuuid ?? "");
const grubErrorText = ref("");

// 后端错误回执经 push 事件更新（与 GrubSection 同步留空亦可）

async function detectEsp() {
  try {
    // 用后端返回的统一文案「「GRUB」识别完成，识别到 N 条」，并且用 toast 显示
    // （每次都弹，不受状态栏「同一条不重复显示」的去重影响 —— 之前点第二次因
    // message 与第一次完全相同，状态栏去重后就不提示了）。
    const res = await call<string>("detect_esp", { path: espDir.value });
    // 走统一的**底部状态栏**提示（ui.setBootNotice → BootApp 的 status-bar），
    // 不是浮层 toast。setBootNotice 每次赋新对象，连续点相同文案也会重复显示。
    ui.setBootNotice(res || "「GRUB」识别完成");
    void call("refresh").catch(() => {});
  } catch (e) {
    toast.show("识别 ESP 失败：" + e, "bad");
  }
}
function saveEspDir() {
  void call("set_grub_dir", { dir: espDir.value }).catch(() => {});
}
function pickEspDir() {
  void call("pick_grub_dir").catch(() => {});
}
function openGrubDir() {
  void call("open_grub_dir").catch(() => {});
}
async function detectPartuuid() {
  try {
    await call("detect_grub_by_partuuid");
  } finally {
    void call("refresh").catch(() => {});
  }
}
function savePartuuid() {
  void call("set_grub_partuuid", { partuuid: partuuid.value }).catch(() => {});
}
async function syncManual() {
  try {
    await call("sync_grub");
  } finally {
    void call("refresh").catch(() => {});
  }
}
function toggleMount() {
  void call("set_grub_mount", { enabled: !grubManual.value?.keepMounted }).catch(() => {});
}
</script>

<template>
  <!-- ESP 配置（旧版 .grub-esp）：自动读取与手动解析共用 -->
  <div class="grub-esp" id="grub-esp" data-esp-root="1">
        <div class="grub-esp-head">
          <strong>ESP 分区</strong>
      <small class="grub-esp-shared-note">自动读取与手动解析共用这里的分区设置</small>
          <small>手动 / 自动共用</small>
        </div>
        <div class="grub-row">
          <label for="grub-dir">ESP 目录</label>
          <input
            id="grub-dir"
            :value="espDir"
            placeholder="H:\ 或 /boot/efi 或 /Volumes/ARCH-EFI（分区里任意路径都行）"
            spellcheck="false"
            type="text"
          />
          <button type="button" id="grub-dir-pick" class="secondary" @click="pickEspDir">选择文件夹…</button>
          <button v-if="isManual" type="button" id="grub-dir-detect" class="secondary" @click="detectEsp">识别</button>
          <button type="button" id="grub-dir-save" class="secondary" @click="saveEspDir">保存</button>
          <button type="button" id="grub-dir-open" class="secondary" @click="openGrubDir">打开文件管理器</button>
        </div>
        <div class="grub-row">
          <label class="grub-check" for="grub-mount">
            <input id="grub-mount" type="checkbox" :checked="Boolean(grubManual?.keepMounted)" @change="toggleMount" />
            <span id="grub-mount-label">挂载 ESP（勾上后可直接读写）</span>
          </label>
          <small class="grub-part" id="grub-part" />
        </div>
        <div class="grub-row">
          <label for="grub-partuuid">ESP 的分区 UUID</label>
          <input
            id="grub-partuuid"
            :value="partuuid"
            placeholder="GPT 分区 GUID，如 F69D7BF9-E8A9-3135-A5CB-4BD0CA7344E3（三端同值：Linux lsblk -o PARTUUID / macOS diskutil / Windows Get-Partition）"
            spellcheck="false"
            type="text"
          />
          <button type="button" id="grub-partuuid-save" class="secondary" @click="savePartuuid">保存</button>
          <button v-if="isManual" type="button" id="grub-partuuid-detect" class="secondary" @click="detectPartuuid">识别</button>
        </div>
        <div v-if="isManual" class="grub-row">
          <button type="button" class="secondary" id="grub-sync" @click="syncManual">同步</button>
          <label class="grub-part" id="grub-sync-note">把当前系统 /boot/grub/grub.cfg 同步到 ESP（需已挂载到 /boot 或 /boot/efi）</label>
        </div>
        <p class="grub-error" id="grub-error" role="status" :hidden="!grubErrorText">{{ grubErrorText }}</p>
      </div>
</template>
