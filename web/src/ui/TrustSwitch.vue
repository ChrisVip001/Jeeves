<script setup lang="ts">
import { computed, ref } from "vue";
import { useUiText } from "./translate";
import type { SessionPermission } from "./usePermission";
const t = useUiText();
const props = defineProps<{
  permission: SessionPermission;
  busy?: boolean;
}>();
const emit = defineEmits<{ trust: [] }>();
const asking = ref(false);

/** Only a session that still asks per operation has anything to gain from the switch. */
const offered = computed(() => props.permission.state.mode !== "fullAccess");
const error = computed(() => props.permission.state.error);
function ask() {
  asking.value = true;
}
function confirm() {
  asking.value = false;
  emit("trust");
}
</script>
<template>
  <div v-if="offered" class="trust-switch">
    <button v-if="!asking" type="button" class="today-btn trust-open" :disabled="busy" @click="ask">
      {{ t("别再问了，这个会话我信任") }}
    </button>
    <div v-else class="trust-ask" role="group" :aria-label="t('确认信任这个会话')">
      <p class="meta">
        {{ t("之后读写文件、运行命令都不再问你。对外发消息这类发出去就收不回的操作，仍然会先问。") }}
      </p>
      <div class="actions">
        <button type="button" class="today-btn" :disabled="busy" @click="asking = false">
          {{ t("算了") }}
        </button>
        <button type="button" class="today-btn primary" :disabled="busy" @click="confirm">
          {{ t("确认，不再逐条问") }}
        </button>
      </div>
    </div>
    <p v-if="error" class="meta trust-error" role="alert">{{ error }}</p>
  </div>
</template>
<style scoped>
.trust-switch {
  margin-top: 12px;
  border-top: 1px solid var(--line);
  padding-top: 12px;
}
.trust-open {
  font-size: 13px;
}
.trust-ask .meta {
  margin: 0 0 10px;
  line-height: 1.6;
}
.trust-error {
  margin: 8px 0 0;
  color: var(--danger, #b4232a);
}
</style>
