<script setup lang="ts">
import { computed, ref } from "vue";
import { t } from "./i18n";
import ApprovalDevicePairing from "./ApprovalDevicePairing.vue";
import { checkApprovalPush, enableApprovalPush, prepareApprovalPush } from "./approval-push";
const deviceId = ref("");
const busy = ref(false);
const error = ref("");
const enabled = ref(false);
const paired = ref(false);
const errorText = computed(() => {
  switch (error.value) {
    case "": return "";
    case "unsupported": return t("当前环境不支持推送。在 iPhone 上请从主屏幕打开 Jeeves。");
    case "unpaired": return t("请先在可信驾驶舱核对此公钥，为此设备授予审批权限。");
    case "unconfigured": return t("此 Jeeves 服务尚未配置 Web Push。");
    case "denied": return t("此设备未允许通知，可在系统设置中更改。");
    default: return t("未能完成设置，请检查连接后重试。");
  }
});
async function configure(step: "key" | "pair" | "enable") {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  try {
    if (step === "enable") {
      await enableApprovalPush();
      enabled.value = true;
    } else if (step === "pair") {
      await checkApprovalPush();
      paired.value = true;
    } else {
      deviceId.value = await prepareApprovalPush();
    }
  } catch (cause) {
    paired.value = false;
    error.value = cause instanceof Error ? cause.message : "failed";
  } finally {
    busy.value = false;
  }
}
</script>
<template>
  <section :aria-label="t('审批通知')" class="approval-push">
    <p class="settings-footnote">{{ t("通知在这台设备上提醒你回到任务中核对操作。") }}</p>
    <button v-if="!deviceId" type="button" class="quiet-button" :disabled="busy" @click="configure('key')">
      {{ t("显示本机配对公钥") }}
    </button>
    <template v-else>
      <label class="push-key">
        {{ t("本机配对公钥") }}
        <input type="text" readonly :value="deviceId" dir="ltr" spellcheck="false" @focus="($event.target as HTMLInputElement).select()" />
      </label>
      <p class="settings-footnote">{{ t("在可信驾驶舱核对公钥并授予审批权限后，即可启用。") }}</p>
      <button v-if="!paired" type="button" class="quiet-button" :disabled="busy" @click="configure('pair')">
        {{ t("核对配对状态") }}
      </button>
      <button v-else-if="!enabled" type="button" class="quiet-button" :disabled="busy" @click="configure('enable')">
        {{ t("在本机启用审批通知") }}
      </button>
    </template>
    <p v-if="errorText || enabled" class="notice" :class="{error: errorText}" role="status">
      {{ errorText || t("已订阅这台设备的审批通知。") }}
    </p>
    <ApprovalDevicePairing />
  </section>
</template>
<style scoped>
.approval-push button { min-height: 44px; }
.push-key { display: grid; gap: 6px; color: var(--text-2); }
.push-key input { width: 100%; box-sizing: border-box; font-family: var(--mono); font-size: 14px; }
</style>
