<script setup lang="ts">
import { OctagonPause } from "lucide-vue-next";
import { useUiText } from "./translate.ts";
import type { ExecutorControlView } from "../protocol.ts";

defineProps<{
  inline?: boolean;
  view: ExecutorControlView | null;
  busy: boolean;
  readFailed: boolean;
  message: string;
}>();
defineEmits<{ stop: []; release: []; refresh: [] }>();
const t = useUiText();
</script>

<template>
  <Teleport to="#executor-entry" :disabled="inline">
    <button
      v-if="!view?.stopped"
      type="button"
      class="executor-stop ghost"
      :disabled="busy"
      :title="t('立即停止此 Runtime 的所有浏览器和桌面操作')"
      :aria-label="t('全局急停')"
      @click="$emit('stop')"
    >
      <OctagonPause :size="16" aria-hidden="true" /><span class="executor-stop-label">{{ t("全局急停") }}</span>
    </button>
  </Teleport>
  <Teleport to="#executor-notice" :disabled="inline">
    <section
      v-if="view?.stopped || readFailed"
      class="executor-banner"
      :aria-label="t('执行器控制')"
    >
      <OctagonPause :size="19" aria-hidden="true" />
      <div role="status">
        <strong>{{
          view?.stopped ? t("全局急停已开启") : t("无法读取急停状态")
        }}</strong>
        <p v-if="view?.stopped">
          {{ t("此 Runtime 的浏览器和桌面操作已停止。解除后需要重新观察。") }}
        </p>
      </div>
      <button
        v-if="view?.stopped"
        type="button"
        class="quiet-button"
        :disabled="busy"
        @click="$emit('release')"
      >
        {{ t("解除急停") }}
      </button>
      <button
        v-else
        type="button"
        class="quiet-button"
        :disabled="busy"
        @click="$emit('refresh')"
      >
        {{ t("重新读取") }}
      </button>
    </section>
    <p v-if="message" class="executor-error task-error" role="alert">{{ message }}</p>
  </Teleport>
</template>
