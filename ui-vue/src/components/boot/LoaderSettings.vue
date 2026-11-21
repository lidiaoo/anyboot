<script setup lang="ts">
import { useI18n } from "vue-i18n";
/**
 * 「引导器设置」折叠面板：放在每个引导器卡片列表下方，把该引导器**真实的额外设置**
 * （rEFInd：模拟扫描头/摘要/变量目录；GRUB：ESP 配置/分区明细/脚本/读取方式）
 * 收进折叠面板，收起时用户只看到简洁的引导列表。
 *
 * 复用真实区块组件（RefindSection/GrubSection）的 hideList 模式，只渲染设置部分。
 */
import { computed, ref } from "vue";
import RefindSection from "./RefindSection.vue";
import GrubSection from "./GrubSection.vue";
import GrubEsp from "./GrubEsp.vue";
import ChannelSettings from "./ChannelSettings.vue";
const { t } = useI18n();

const props = defineProps<{ loader: string; defaultOpen?: boolean; plain?: boolean }>();
const open = ref(Boolean(props.defaultOpen));

/** 多实例后 `loader` 是实例 id（`grub@b3fb…/efi`），比较前要先取 kind。
 *  否则所有引导器都落到 ChannelSettings 兜底 → 显示「没有可调设置」（用户实测）。 */
const kindOf = computed(() => String(props.loader || "").split("@")[0]);

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
        <span>{{ t("boot.loaderSettings") }}</span>
      </span>
      <span class="chevron car" aria-hidden="true" :style="{ transform: open ? 'rotate(180deg)' : '' }">⌄</span>
    </button>
    <div v-show="props.plain || open" class="ba-set-body">
      <template v-if="kindOf === 'refind'">
        <RefindSection hide-list />
      </template>
      <template v-else-if="kindOf === 'refindplus'">
        <RefindSection channel="refindplus" hide-list />
      </template>
      <template v-else-if="kindOf === 'grub'">
        <GrubSection hide-list />
        <GrubEsp />
      </template>
      <ChannelSettings v-else :loader="kindOf" />
    </div>
  </div>
</template>
