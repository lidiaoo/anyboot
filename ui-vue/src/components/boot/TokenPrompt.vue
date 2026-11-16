<script setup lang="ts">
/**
 * 浏览器/手机端「访问令牌(token)」输入框。
 *
 * 背景：远程页面（网页/手机 App）通过 `?token=` 或 cookie 带 token 访问本机 HTTP 服务；
 * 直接打开（无 token）或 token 失效(401) 时，旧版会弹这个框让用户输入。
 * Vue 迁移时漏了它 → 无 token 直接空白/读不到数据。这里补回。
 */
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { setAuthToken } from "@/api/http";
const { t } = useI18n();

const emit = defineEmits<{ (e: "confirmed"): void }>();
const input = ref("");
const error = ref("");
const busy = ref(false);

function submit() {
  const tok = input.value.trim();
  if (!tok) {
    error.value = t("token.errEmpty");
    return;
  }
  if (/\s/.test(tok)) {
    error.value = t("token.errSpace");
    return;
  }
  busy.value = true;
  setAuthToken(tok);
  // 存进 cookie，刷新/后续请求沿用（与旧版 REMOTE_TOKEN_COOKIE 同义）
  try {
    document.cookie = `anyboot_token=${encodeURIComponent(tok)}; path=/; max-age=${60 * 60 * 24 * 365}; SameSite=Strict`;
  } catch {
    /* ignore */
  }
  busy.value = false;
  emit("confirmed");
}
</script>

<template>
  <div class="token-modal-overlay" style="z-index: 300">
    <div class="token-modal" role="dialog" aria-modal="true" :aria-label="t('token.ariaTitle')">
      <h2>{{ t("token.title") }}</h2>
      <p>
        {{ t("token.body") }}
      </p>
      <input
        v-model="input"
        type="text"
        spellcheck="false"
        :placeholder="t('token.ph')"
        @keyup.enter="submit"
      />
      <p v-if="error" style="color: #d9534f; font-size: 12px">{{ error }}</p>
      <div style="display: flex; gap: 8px; margin-top: 12px; justify-content: flex-end">
        <button type="button" class="btn" :disabled="busy" @click="submit">{{ t("token.connect") }}</button>
      </div>
    </div>
  </div>
</template>
