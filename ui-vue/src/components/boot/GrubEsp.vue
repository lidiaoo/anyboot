<script setup lang="ts">
/**
 * GRUB「ESP 分区」共享表单（自动读取与手动解析共用）——从 GrubSection 剥离，
 * 由 LoaderSettings 在 grub 分支独立渲染（修复 G 主题 slot 场景下表单渲染为空的问题）。
 */
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { call } from "@/api/tauri";
import { useUiStateStore } from "@/stores/uiState";
import { useToastStore } from "@/stores/toast";
import { useBootInstancesStore } from "@/stores/bootInstances";
import { useSettingsStore } from "@/stores/settings";
const { t } = useI18n();

const ui = useUiStateStore();
const toast = useToastStore();
const instancePrefs = useBootInstancesStore();
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
    ui.setBootNotice(res || t("grubEsp.recognized", { name: "GRUB" }));
    void call("refresh").then(() => instancePrefs.load(true)).catch(() => {});
  } catch (e) {
    toast.show(t("grubEsp.fail", { msg: String(e) }), "bad");
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
    void call("refresh").then(() => instancePrefs.load(true)).catch(() => {});
  }
}
function savePartuuid() {
  void call("set_grub_partuuid", { partuuid: partuuid.value }).catch(() => {});
}
async function syncManual() {
  try {
    await call("sync_grub");
  } finally {
    void call("refresh").then(() => instancePrefs.load(true)).catch(() => {});
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
          <strong>{{ t("grubEsp.espPart") }}</strong>
      <small class="grub-esp-shared-note">{{ t("grubEsp.sharedNote") }}</small>
          <small>{{ t("grubEsp.sharedShort") }}</small>
        </div>
        <div class="grub-row">
          <label for="grub-dir">{{ t("grubEsp.dir") }}</label>
          <input
            id="grub-dir"
            :value="espDir"
            :placeholder="t('grubEsp.dirPh')"
            spellcheck="false"
            type="text"
          />
          <button type="button" id="grub-dir-pick" class="secondary" @click="pickEspDir">{{ t("grubEsp.pick") }}</button>
          <button v-if="isManual" type="button" id="grub-dir-detect" class="secondary" @click="detectEsp">{{ t("grubEsp.detect") }}</button>
          <button type="button" id="grub-dir-save" class="secondary" @click="saveEspDir">{{ t("common.save") }}</button>
          <button type="button" id="grub-dir-open" class="secondary" @click="openGrubDir">{{ t("grubEsp.openFM") }}</button>
        </div>
        <div class="grub-row">
          <label class="grub-check" for="grub-mount">
            <input id="grub-mount" type="checkbox" :checked="Boolean(grubManual?.keepMounted)" @change="toggleMount" />
            <span id="grub-mount-label">{{ t("grubEsp.mountLabel") }}</span>
          </label>
          <small class="grub-part" id="grub-part" />
        </div>
        <div class="grub-row">
          <label for="grub-partuuid">{{ t("grubEsp.partUuidLabel") }}</label>
          <input
            id="grub-partuuid"
            :value="partuuid"
            :placeholder="t('grubEsp.partUuidPh')"
            spellcheck="false"
            type="text"
          />
          <button type="button" id="grub-partuuid-save" class="secondary" @click="savePartuuid">{{ t("common.save") }}</button>
          <button v-if="isManual" type="button" id="grub-partuuid-detect" class="secondary" @click="detectPartuuid">{{ t("grubEsp.detect") }}</button>
        </div>
        <div v-if="isManual" class="grub-row">
          <button type="button" class="secondary" id="grub-sync" @click="syncManual">{{ t("grubEsp.sync") }}</button>
          <label class="grub-part" id="grub-sync-note">{{ t("grubEsp.syncNote") }}</label>
        </div>
        <p class="grub-error" id="grub-error" role="status" :hidden="!grubErrorText">{{ grubErrorText }}</p>
      </div>
</template>
