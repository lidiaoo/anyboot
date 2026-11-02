<script setup lang="ts">
import { computed } from "vue";
import type { RemoteProfile } from "@/stores/remote";
import type { RemoteBootInfo } from "@/api/types";

/**
 * 电脑列表的一行（原 `ui/hub.html` 的 `cqRowEl()`）。
 *
 * ```
 * .cq-row
 *   .cq-col-dev   电脑：图标 + 名称 + 系统徽标 + 连接标注 + 地址/MAC
 *   .cq-col-st    状态：在线 / 离线圆点
 *   .cq-col-boot  rEFInd 启动项按钮
 *   .cq-col-boot  GRUB 启动项按钮
 *   .cq-col-pwr   电源：唤醒 / 重启 / 关机 / 睡眠
 *   .cq-col-more  ⋯
 * ```
 *
 * **四条既有定案**（勿改）：
 * 1. 四个电源键**任何时候都能按、不置灰**（设备离线时也能发魔术包唤醒）
 * 2. 引导项按钮在**离线时不可点**，并显示「离线不可改」
 * 3. 引导项文字**完整多行显示，不省略号**
 * 4. 引导项变更**下次重启才生效**（不即时生效）
 */

const props = defineProps<{
  row: RemoteProfile;
  info: RemoteBootInfo | undefined;
  pending: boolean;
}>();

const emit = defineEmits<{
  (e: "switchBoot", payload: Record<string, unknown>): void;
  (e: "power", payload: Record<string, unknown>): void;
  (e: "sysInfo", id: string): void;
}>();

const isLocal = computed(() => props.row.id === "local");
const online = computed(() => props.info?.online ?? false);
/** 还没读到数据（快照未到） */
const loading = computed(() => props.info === undefined);

const statusText = computed(() => {
  if (loading.value) return "读取中…";
  return online.value ? "在线" : "离线";
});

/** rEFInd 的值文字：当前项 / 未设置 / 各种不可用态 */
const refindLabel = computed(() => valueLabel(props.info?.refind));
const grubLabel = computed(() => valueLabel(props.info?.grub));

function valueLabel(group: { available?: boolean; current?: string; note?: string } | undefined) {
  if (!group) return "读取中…";
  if (!group.available) {
    return group.note || "未检测到";
  }
  return group.current || "未设置";
}

const refindUsable = computed(() => Boolean(props.info?.refind?.available) && online.value);
const grubUsable = computed(() => Boolean(props.info?.grub?.available) && online.value);

function onSwitch(kind: "refind" | "grub") {
  if (!online.value) return;
  emit("switchBoot", { serverId: props.row.id, kind });
}

function onPower(action: string) {
  // 电源键任何时候都能按（离线也能发魔术包唤醒）
  emit("power", { serverId: props.row.id, action });
}
</script>

<template>
  <div :class="['cq-row', { 'cq-row--local': isLocal }]" :data-rid="row.id">
    <!-- ① 电脑 -->
    <div class="cq-col cq-col-dev">
      <div class="cq-dev">
        <div class="cq-dev-ico" aria-hidden="true">🖥</div>
        <div class="cq-dev-txt">
          <div class="cq-dev-name">
            <span class="nm">{{ row.name }}</span>
            <button
              v-if="!isLocal"
              type="button"
              class="os-badge"
              title="查看系统信息"
              @click.stop="emit('sysInfo', row.id)"
            >
              {{ (row.machine_os || "").toLowerCase().includes("win") ? "WIN"
                : (row.machine_os || "").toLowerCase().includes("mac") ? "MAC" : "LNX" }}
            </button>
            <span v-if="isLocal" class="cq-chip-grp">本机</span>
            <span v-if="pending" class="cq-chip-pend">待生效</span>
          </div>
          <div class="cq-dev-meta">
            <div class="cq-dev-line">
              <span class="cq-dev-addr">{{ info?.addr || (row.addresses?.[0] ?? "") }}</span>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- ② 状态 -->
    <div class="cq-col cq-col-st">
      <span :class="['cq-state', online ? 'is-on' : 'is-off']">
        <i class="dot" aria-hidden="true" />
        {{ statusText }}
      </span>
    </div>

    <!-- ③ rEFInd -->
    <div class="cq-col cq-col-boot">
      <button
        type="button"
        :class="['cq-boot-btn', { 'is-off': !refindUsable }]"
        :disabled="!refindUsable || loading"
        @click="onSwitch('refind')"
      >
        <span class="t">{{ refindLabel }}</span>
      </button>
    </div>

    <!-- ④ GRUB -->
    <div class="cq-col cq-col-boot">
      <button
        type="button"
        :class="['cq-boot-btn', { 'is-off': !grubUsable }]"
        :disabled="!grubUsable || loading"
        @click="onSwitch('grub')"
      >
        <span class="t">{{ grubLabel }}</span>
      </button>
    </div>

    <!-- ⑤ 电源（任何时候都能按，不置灰） -->
    <div class="cq-col cq-col-pwr">
      <button type="button" class="cq-pwr" title="唤醒" @click="onPower('wake')">⏻</button>
      <button type="button" class="cq-pwr" title="重启" @click="onPower('restart')">↻</button>
      <button type="button" class="cq-pwr" title="关机" @click="onPower('shutdown')">⏼</button>
      <button type="button" class="cq-pwr" title="睡眠" @click="onPower('sleep')">☾</button>
    </div>

    <!-- ⑥ 更多 -->
    <div class="cq-col cq-col-more">
      <button type="button" class="btn icon ghost sm" title="更多" aria-label="更多">⋯</button>
    </div>
  </div>
</template>

<style scoped>
.cq-row {
  display: grid;
  grid-template-columns:
    minmax(180px, 1.6fr) 84px minmax(120px, 1fr) minmax(120px, 1fr) 118px 42px;
  align-items: center;
  gap: 10px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--line, #f1f5f9);
  font-size: 12.5px;
}
.cq-row:last-child {
  border-bottom: 0;
}
.cq-dev {
  display: flex;
  gap: 9px;
  min-width: 0;
}
.cq-dev-ico {
  width: 26px;
  height: 26px;
  flex: none;
  display: grid;
  place-items: center;
  border-radius: 7px;
  background: var(--soft, #f3f4f6);
  font-size: 13px;
}
.cq-dev-txt {
  min-width: 0;
}
.cq-dev-name {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  font-weight: 600;
  color: var(--fg, #111827);
}
.cq-dev-name .nm {
  /* 完整显示，不省略号（用户定案） */
  white-space: normal;
  overflow-wrap: anywhere;
}
.cq-dev-meta {
  margin-top: 2px;
  color: var(--fg-faint, #9ca3af);
  font-size: 11px;
  font-family: ui-monospace, Menlo, monospace;
}
.cq-dev-addr {
  overflow-wrap: anywhere;
}
.cq-chip-grp,
.os-badge {
  font-size: 9.5px;
  font-weight: 600;
  padding: 1px 5px;
  border-radius: 4px;
  border: 1px solid var(--line, #e5e7eb);
  background: var(--soft, #f9fafb);
  color: var(--fg-dim, #6b7280);
  cursor: pointer;
  font-family: ui-monospace, Menlo, monospace;
}
.os-badge:hover {
  background: var(--soft, #f3f4f6);
}
.cq-chip-pend {
  font-size: 10px;
  font-weight: 500;
  padding: 1px 6px;
  border-radius: 999px;
  background: var(--soft, #f3f4f6);
  color: var(--fg-dim, #6b7280);
}
.os-badge {
  font-size: 9.5px;
  font-weight: 600;
  padding: 1px 5px;
  border-radius: 4px;
  border: 1px solid var(--line, #e5e7eb);
  background: var(--soft, #f9fafb);
  color: var(--fg-dim, #6b7280);
  cursor: pointer;
  font-family: ui-monospace, Menlo, monospace;
}
.os-badge:hover {
  background: var(--soft, #f3f4f6);
}
.cq-chip-pend {
  background: #fff7ed;
  color: #c2410c;
}
.cq-state {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
}
.cq-state .dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #9ca3af;
}
.cq-state.is-on .dot {
  background: #16a34a;
}
.cq-state.is-off {
  color: var(--fg-faint, #9ca3af);
}
.cq-boot-btn {
  width: 100%;
  text-align: left;
  padding: 6px 9px;
  border-radius: 8px;
  border: 1px solid var(--line, #e5e7eb);
  background: var(--panel, #fff);
  cursor: pointer;
  font-size: 12px;
  color: var(--fg, #111827);
}
.cq-boot-btn .t {
  /* 条目名完整多行，不省略号（用户定案） */
  white-space: normal;
  overflow-wrap: anywhere;
  line-height: 1.45;
}
.cq-boot-btn.is-off {
  cursor: not-allowed;
  color: var(--fg-faint, #9ca3af);
  background: var(--soft, #f9fafb);
}
.cq-pwr {
  border: 1px solid var(--line, #e5e7eb);
  background: var(--panel, #fff);
  border-radius: 6px;
  width: 26px;
  height: 24px;
  cursor: pointer;
  font-size: 12px;
  color: var(--fg-dim, #4b5563);
}
.cq-pwr:hover {
  background: var(--soft, #f3f4f6);
}
.cq-col-pwr {
  display: flex;
  gap: 4px;
}
</style>
