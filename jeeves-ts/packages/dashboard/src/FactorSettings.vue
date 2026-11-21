<script setup lang="ts">
import { ref } from "vue";
import { t } from "./i18n";
defineProps<{ enabled: boolean }>();
const emit = defineEmits<{ updated: [] }>();
const password = ref("");
const secret = ref("");
const code = ref("");
const backups = ref<string[]>([]);
const error = ref("");
const busy = ref(false);
async function update(action: "start" | "confirm" | "disable") {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  try {
    const response = await fetch("/api/account/factor/" + action, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ password: password.value, code: code.value }),
    });
    if (!response.ok) throw Error();
    const result = await response.json();
    secret.value = result.secret ?? "";
    backups.value = result.codes ?? [];
    code.value = "";
    if (action !== "start") {
      password.value = "";
      emit("updated");
    }
  } catch {
    error.value = t("操作失败，请检查密码、验证码或稍后重试。");
  } finally {
    busy.value = false;
  }
}
</script>
<template>
  <details>
    <summary id="account-factor-settings">
      {{ t("双因素认证") }} · {{ enabled ? t("已启用") : t("未启用") }}
    </summary>
    <div v-if="backups.length">
      <p>{{ t("请保存这些一次性恢复码。每个码只能使用一次。") }}</p>
      <pre>{{ backups.join("\n") }}</pre>
      <button class="quiet" @click="backups = []">
        {{ t("已保存恢复码") }}
      </button>
    </div>
    <form
      v-else
      @submit.prevent="
        update(enabled ? 'disable' : secret ? 'confirm' : 'start')
      "
    >
      <label
        >{{ t("当前密码")
        }}<input
          type="password"
          id="factor-password"
          v-model="password"
          autocomplete="current-password"
          required
      /></label>
      <p v-if="secret">
        {{ t("在验证器中选择手动输入密钥，使用基于时间的验证码。") }}
        <code id="factor-setup-key">{{ secret }}</code>
      </p>
      <label v-if="secret || enabled"
        >{{ t("验证码或恢复码")
        }}<input
          id="factor-code"
          v-model="code"
          autocomplete="one-time-code"
          required
      /></label>
      <button id="factor-submit" class="primary" :disabled="busy">
        {{
          enabled
            ? t("关闭双因素认证")
            : secret
              ? t("确认启用")
              : t("绑定验证器")
        }}
      </button>
    </form>
    <p v-if="error" role="alert">{{ error }}</p>
  </details>
</template>
