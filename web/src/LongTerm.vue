<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { workspaceActions } from "./controls";
import { t } from "./i18n";
import ResponsibilityCards from "./ResponsibilityCards.vue";
defineProps<{ busy: boolean }>();
/** The scheduled-jobs panel is borrowed from the workspace while 工作 shows, then handed back. */
const host = ref<HTMLElement>();
let handBack: (() => void) | null = null;
onMounted(() => { if (host.value) handBack = workspaceActions.mountPanel("schedule", host.value); });
onBeforeUnmount(() => { handBack?.(); handBack = null; });
</script>

<template>
  <section class="activity-group long-term" :aria-labelledby="'long-term-title'">
    <h2 id="long-term-title">{{ t("长期") }}</h2>
    <ResponsibilityCards :busy="busy" list />
    <div ref="host" class="long-term-schedules"></div>
  </section>
</template>
