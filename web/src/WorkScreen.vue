<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { Globe, Monitor } from "lucide-vue-next";
import { t } from "./i18n";
import type { AgentTaskView } from "./protocol";
import type { TaskCenter } from "./ui/useTasks";
import RemoteBrowser from "./ui/RemoteBrowser.vue";
const props = defineProps<{ task: AgentTaskView; center: TaskCenter }>();
const browser = computed(() => props.center.browser);
const row = computed(() => browser.value?.state.sessions.find(item => item.taskId === props.task.id));
/** Whether the interactive remote-control dialog is open (only meaningful while you hold control). */
const remote = ref(false);
const live = computed(() => !!row.value && !["closed", "needsAttention"].includes(row.value.status));
const frame = computed(() => browser.value?.state.frames[props.task.id]);
const busy = computed(() => !!browser.value?.state.busy[props.task.id]);
const controlError = computed(() => browser.value?.state.errors[props.task.id]);

/** How often the live screen refreshes while Jeeves is driving the browser. */
const FRAME_MS = 3000;
let timer: number | undefined;
function pull() {
  if (!browser.value || !live.value || busy.value || document.visibilityState !== "visible") return;
  void browser.value.frame(props.task.id).catch((error: unknown) => console.debug("Frame refresh failed", error));
}
// Watch each value on its own: a single getter that builds an array fires on every refresh (a
// fresh array is never Object.is-equal), and each run pulls a frame whose refresh re-runs it — a
// flood. The multi-source form only fires when `live` or the status actually changes.
watch([live, () => row.value?.status], () => {
  window.clearInterval(timer);
  timer = undefined;
  if (live.value && row.value?.status === "agent") timer = window.setInterval(pull, FRAME_MS);
  if (live.value) pull();
}, { immediate: true });
/** A hidden tab stops pulling frames; refresh at once on return so the screen is never a tick stale. */
function onReturn() {
  if (document.visibilityState === "visible") pull();
}
onMounted(() => document.addEventListener("visibilitychange", onReturn));
onUnmounted(() => {
  window.clearInterval(timer);
  document.removeEventListener("visibilitychange", onReturn);
});
/** Close the interactive dialog the moment control returns to Jeeves or the session ends. */
watch(() => row.value?.status, status => { if (status !== "human") remote.value = false; });
async function takeover() {
  if (!row.value) return;
  await browser.value?.control(row.value, "takeover");
  if (row.value?.status === "human") remote.value = true;
}
function label(status: string) {
  switch (status) {
    case "agent": return t("Jeeves 正在操作");
    case "human": return t("你正在操作");
    case "paused": return t("已暂停");
    default: return status;
  }
}
</script>
<template>
  <aside class="work-screen" :aria-label="t('Jeeves 的电脑')">
    <header><h3>{{ t("Jeeves 的电脑") }}</h3><span v-if="live && row" class="work-screen-state">{{ label(row.status) }}</span></header>
    <template v-if="live && row">
      <p class="work-screen-url" :title="row.url"><Globe :size="14" aria-hidden="true" />{{ row.title || row.url }}</p>
      <figure v-if="frame" class="work-screen-frame">
        <img :src="frame.dataUrl" :alt="t('浏览器当前画面')" />
        <figcaption>{{ t("画面更新于 {time}", { time: new Date(frame.observedAt).toLocaleTimeString() }) }}</figcaption>
      </figure>
      <p v-else class="work-screen-empty">{{ t("正在取得画面…") }}</p>
      <div class="work-screen-actions">
        <button v-if="row.status !== 'human'" type="button" class="primary-button" :disabled="busy"
          @click="takeover">{{ t("接管") }}</button>
        <button v-if="row.status === 'human'" type="button" class="primary-button" :disabled="busy"
          @click="remote = true">{{ t("继续操作") }}</button>
        <button v-if="['human', 'paused'].includes(row.status)" type="button" class="quiet-button"
          :disabled="busy || task.status === 'cancelled'" @click="browser?.control(row, 'return')">{{ t("交还给 Jeeves") }}</button>
        <button v-if="row.status !== 'agent'" type="button" class="quiet-button" :disabled="busy" @click="pull">{{ t("刷新画面") }}</button>
      </div>
      <p v-if="controlError" class="task-error" role="status">{{ controlError }}</p>
      <Teleport to="body">
        <RemoteBrowser v-if="browser && remote && row.status === 'human'" :center="browser" :row="row"
          @close="remote = false" />
      </Teleport>
    </template>
    <div v-else class="work-screen-empty">
      <Monitor :size="28" aria-hidden="true" />
      <p>{{ t("这件事还没用到浏览器或你的 Mac。Jeeves 开始操作时，画面会显示在这里。") }}</p>
    </div>
    <div class="work-screen-tools"><slot /></div>
  </aside>
</template>
