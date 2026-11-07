<script setup lang="ts">
/**
 * 「引导器设置」折叠面板：放在每个引导器卡片列表下方，把该引导器**真实的额外设置**
 * （rEFInd：模拟扫描头/摘要/变量目录；GRUB：ESP 配置/分区明细/脚本/读取方式）
 * 收进折叠面板，收起时用户只看到简洁的引导列表。
 *
 * 复用真实区块组件（RefindSection/GrubSection）的 hideList 模式，只渲染设置部分。
 */
import { ref } from "vue";
import RefindSection from "./RefindSection.vue";
import GrubSection from "./GrubSection.vue";
import GrubEsp from "./GrubEsp.vue";
import ChannelSettings from "./ChannelSettings.vue";

const props = defineProps<{ loader: string; defaultOpen?: boolean; plain?: boolean }>();
const open = ref(Boolean(props.defaultOpen));
</script>

<template>
  <div class="loader-settings" :class="{ 'is-plain': props.plain }">
    <button
      v-if="!props.plain"
      type="button"
      class="acc ba-setbar"
      :aria-expanded="open ? 'true' : 'false'"
      @click="open = !open"
    >
      <span class="gear" aria-hidden="true" style="margin-right: 4px">⚙</span>
      <span class="grow">
        <span>引导器设置</span>
        <small>
          {{ props.loader === "refind" ? "重新模拟 · 扫描摘要 · 变量目录" : props.loader === "grub" ? "ESP 分区 · 分区明细 · 同步 · 脚本" : "健康检查 · 免密模式" }}
        </small>
      </span>
      <span class="chevron car" aria-hidden="true" :style="{ transform: open ? 'rotate(180deg)' : '' }">⌄</span>
    </button>
    <div v-show="props.plain || open" class="ba-set-body">
      <template v-if="props.loader === 'refind'">
        <RefindSection hide-list />
      </template>
      <template v-else-if="props.loader === 'grub'">
        <GrubSection hide-list />
        <GrubEsp />
      </template>
      <ChannelSettings v-else :loader="props.loader" />
    </div>
  </div>
</template>
