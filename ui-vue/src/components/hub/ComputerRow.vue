<script setup lang="ts">
import { computed } from "vue";
import type { RemoteProfile } from "@/stores/remote";
import type { RemoteBootInfo } from "@/api/types";

/**
 * 电脑列表的一行（**1:1 对齐旧版 `cqRowEl()` hub.html:8347**）。
 *
 * 旧版结构（`hub.html:8634` 起，用户 2026-11-01 定案的「移动端同款上下布局」）：
 *
 * ```
 * .cq-row[data-rid]
 *   .cq-row-head          头部行
 *     .cq-col-dev          图标 + 名称 + 徽章 + 地址
 *     .cq-col-st           状态
 *     .cq-col-more         更多
 *   .cq-row-boots         引导项区（wrap 一行）
 *     .cq-col-boot[data-label="rEFInd 启动项"]
 *     .cq-col-boot[data-label="GRUB 启动项"]
 *     .cq-col-boot.cq-col-boot--ch[…]   三通道（检测到才有）
 *   .cq-row-pwr           电源区
 *     .cq-col-pwr > .cq-pwr > button.cq-pwr-btn × 4
 * ```
 *
 * **注意：不是六列横排**。我第一版做成了 `.cq-col-*` 六列 grid，
 * 而 CSS 里 `.cq-row` 是 `flex-direction: column`，`.cq-thead { display: none }`。
 */

const props = defineProps<{
  row: RemoteProfile;
  info: RemoteBootInfo | undefined;
  pending: boolean;
  /** 是否选中（.cq-selected） */
  selected: boolean;
  /** 自定义排序模式下显示 ↑↓ */
  sortMode: string;
  /** 该行在 remotes 里的序号（排序用） */
  orderIndex: number;
  /** remotes 总数（判断 ↑↓ 是否到头） */
  orderTotal: number;
}>();

const emit = defineEmits<{
  (e: "select", id: string): void;
  (e: "switchBoot", payload: Record<string, unknown>): void;
  (e: "power", payload: Record<string, unknown>): void;
  (e: "sysInfo", id: string): void;
  (e: "more", id: string): void;
  (e: "view", id: string): void;
  (e: "openBrowser", id: string): void;
  (e: "startTunnel", id: string): void;
  (e: "stopTunnel", id: string): void;
  (e: "move", payload: { id: string; delta: number }): void;
}>();

const isLocal = computed(() => props.row.id === "local");
const online = computed(() => props.info?.online ?? false);
const loading = computed(() => props.info === undefined);

const statusText = computed(() => {
  if (loading.value) return "读取中…";
  return online.value ? "在线" : "未探测";
});

/** 系统徽标 */
const osBadge = computed(() => {
  const os = String(props.row.machine_os ?? "").toLowerCase();
  if (os.includes("win")) return { text: "WIN", kind: "windows" };
  if (os.includes("mac")) return { mac: true, text: "MAC", kind: "macos" };
  if (os.includes("linux")) return { text: "LNX", kind: "linux" };
  return null;
});

/** 连接标注（旧版 cq-chip-grp / cq-chip-remote） */
const connLabel = computed(() => (isLocal.value ? "本机" : online.value ? "直连" : "未连接"));

/** 引导项值 */
function bootLabel(kind: "refind" | "grub") {
  const group = props.info?.[kind];
  if (!group) return "读取中…";
  if (!group.available) return "未检测到";
  return group.current || "未设置";
}

function bootUsable(kind: "refind" | "grub") {
  const group = props.info?.[kind];
  return Boolean(group?.available) && online.value;
}

/** 电源键定义（图标 + 标签 + 提示，对齐旧版 CQ_POWER） */
const POWER = [
  { act: "wake", label: "唤醒", hint: isLocal.value ? "本机无需唤醒" : "魔术包", icon: "power" },
  { act: "sleep", label: "睡眠", hint: isLocal.value ? "挂起到内存" : "挂起到内存", icon: "moon" },
  { act: "reboot", label: "重启", hint: isLocal.value ? "" : "HTTP", icon: "refresh" },
  { act: "shutdown", label: "关机", hint: isLocal.value ? "" : "HTTP", icon: "shutdown" },
] as const;

/** 电源图标（对齐旧版 CQ_POWER 的 svg） */
const POWER_ICONS: Record<string, string> = {
  wake: '<path d="M12 3v9"/><path d="M6.4 6.4a8 8 0 1 0 11.2 0"/>',
  sleep: '<path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z"/>',
  reboot: '<path d="M21 12a9 9 0 1 1-2.6-6.4"/><path d="M21 3v6h-6"/>',
  shutdown: '<path d="M12 3v9"/><path d="M18 6a9 9 0 1 1-12 0"/>',
};

const powerHints = computed(() =>
  POWER.map((spec) => ({
    ...spec,
    hint:
      spec.act === "wake" && !isLocal.value
        ? online.value
          ? "魔术包"
          : "未配 MAC"
        : spec.hint,
  })),
);

function onRowClick() {
  emit("select", props.row.id);
}
</script>

<template>
  <div
    class="cq-row"
    :class="{ 'cq-selected': selected }"
    :data-rid="row.id"
    role="row"
    @click="onRowClick"
  >
    <!-- 头部行 -->
    <div class="cq-row-head">
      <div class="cq-col cq-col-dev" role="cell">
        <div v-if="sortMode === 'custom'" class="cq-move">
          <template v-if="!isLocal">
            <button
              type="button"
              title="上移一位（自定义排序）"
              :disabled="orderIndex <= 0"
              @click.stop="emit('move', { id: row.id, delta: -1 })"
            >
              ▴
            </button>
            <button
              type="button"
              title="下移一位（自定义排序）"
              :disabled="orderIndex >= orderTotal - 1"
              @click.stop="emit('move', { id: row.id, delta: 1 })"
            >
              ▾
            </button>
          </template>
          <span v-else style="width: 18px" />
        </div>

        <div class="cq-dev">
          <div class="cq-dev-ico">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
              <rect x="2" y="4" width="20" height="13" rx="2" />
              <path d="M8 21h8M12 17v4" />
            </svg>
          </div>
          <div class="cq-dev-txt">
            <div class="cq-dev-name">
              <span class="nm">{{ row.name }}</span>
              <button
                v-if="!isLocal && osBadge"
                type="button"
                class="os-badge"
                :data-os="osBadge.kind"
                title="查看系统信息"
                @click.stop="emit('sysInfo', row.id)"
              >
                {{ osBadge.text }}
              </button>
              <span v-if="selected" class="cq-chip-cur">当前</span>
              <span
                class="cq-chip-grp"
                :class="{ 'cq-chip-remote': false }"
                :title="connLabel === '直连' ? '局域网直连' : '当前没探通 —— 目标机可能离线'"
              >
                {{ connLabel }}
              </span>
            </div>
            <div class="cq-dev-meta">
              <button
                type="button"
                class="cq-remote-act"
                title="在弹层里打开这台电脑的 AnyBoot 页面"
                @click.stop="emit('view', row.id)"
              >
                打开页面
              </button>
              <button
                type="button"
                class="cq-remote-act"
                title="用系统默认浏览器打开这条通道（window.open 只会开新 webview）"
                @click.stop="emit('openBrowser', row.id)"
              >
                浏览器打开
              </button>
              <div class="cq-dev-line">
                <div class="cq-dev-top">
                  <span
                    class="cq-dev-addr"
                    :style="{ color: online ? 'rgb(var(--ok))' : 'rgb(var(--fg-faint))' }"
                  >
                    {{ online ? "● 在线" : "○ 离线" }}
                  </span>
                  <span class="sep">·</span>
                  <span class="cq-dev-addr">{{ info?.addr || row.addresses?.[0] || "" }}</span>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 状态 -->
      <div class="cq-col cq-col-st" role="cell">
        <div class="cq-st">
          <small>{{ statusText }}</small>
        </div>
      </div>

      <!-- 更多 -->
      <div class="cq-col cq-col-more" role="cell">
        <button
          type="button"
          class="cq-more"
          title="更多操作"
          aria-label="更多操作"
          @click.stop="emit('more', row.id)"
        >
          <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <circle cx="12" cy="5" r="1.7" />
            <circle cx="12" cy="12" r="1.7" />
            <circle cx="12" cy="19" r="1.7" />
          </svg>
        </button>
      </div>
    </div>

    <!-- 引导项区 -->
    <div class="cq-row-boots">
      <div class="cq-col cq-col-boot" role="cell" data-label="rEFInd 启动项">
        <button
          type="button"
          class="cq-boot-btn"
          :disabled="!bootUsable('refind')"
          @click.stop="emit('switchBoot', { server_id: row.id, kind: 'refind' })"
        >
          <span class="t">{{ bootLabel("refind") }}</span>
        </button>
      </div>
      <div class="cq-col cq-col-boot" role="cell" data-label="GRUB 启动项">
        <button
          type="button"
          class="cq-boot-btn"
          :disabled="!bootUsable('grub')"
          @click.stop="emit('switchBoot', { server_id: row.id, kind: 'grub' })"
        >
          <span class="t">{{ bootLabel("grub") }}</span>
        </button>
      </div>
      <div
        v-for="ch in info?.channels ?? []"
        :key="(ch as any).id"
        class="cq-col cq-col-boot cq-col-boot--ch"
        role="cell"
        :data-label="`${(ch as any).label || '引导器'} 启动项`"
      >
        <button type="button" class="cq-boot-btn" disabled>
          <span class="t">{{ (ch as any).currentDefault || ((ch as any).detected ? "已检测" : "未检测到") }}</span>
        </button>
      </div>
    </div>

    <!-- 电源区 -->
    <div class="cq-row-pwr">
      <div class="cq-col cq-col-pwr" role="cell">
        <div class="cq-pwr">
          <button
            v-for="spec in powerHints"
            :key="spec.act"
            type="button"
            class="cq-pwr-btn"
            :title="spec.hint ? `${spec.label} · ${spec.hint}` : spec.label"
            :aria-label="spec.hint ? `${spec.label} · ${spec.hint}` : spec.label"
            @click.stop="emit('power', { server_id: row.id, action: spec.act })"
          >
            <span class="lbl">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
                   stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"
                   v-html="POWER_ICONS[spec.act]" />
              {{ spec.label }}
            </span>
            <span class="hint">{{ spec.hint }}</span>
          </button>
        </div>
      </div>
    </div>
  </div>
</template>
