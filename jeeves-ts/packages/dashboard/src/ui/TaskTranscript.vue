<script setup lang="ts">
import { onMounted, ref, watch } from "vue";
import type { AgentTaskView, SessionMessagesResponse } from "./types";
import type { TaskCenter } from "./useTasks";
import { stripToolFold } from "../markdown";
import { useUiText } from "./translate";
const t = useUiText();
const props = defineProps<{ task: AgentTaskView; center: TaskCenter; revision?: number }>();
const messages = ref<SessionMessagesResponse["data"]>([]);
const activeInputs = ref<string[]>([]);
const cursor = ref<string | null>(null);
const error = ref("");
const loading = ref(false);
let refreshPending = false;
async function read(older: boolean) {
  const session = props.task.executionSessionId;
  if (!session) return;
  if (loading.value) {
    if (!older) refreshPending = true;
    return;
  }
  loading.value = true;
  error.value = "";
  try {
    const newest = messages.value.at(-1)?.id;
    let page = await props.center.messages(session, older ? cursor.value : null);
    if (!older) activeInputs.value = page.activeInputs ?? [];
    const rows = [...page.data];
    // Catch up across every missed page before joining the existing transcript.
    while (!older && newest !== undefined && page.nextCursor && (page.data.at(-1)?.id ?? 0) > newest) {
      page = await props.center.messages(session, page.nextCursor);
      rows.push(...page.data);
    }
    if (older || newest === undefined) cursor.value = page.nextCursor;
    messages.value = [...new Map([...messages.value, ...rows].map(m => [m.id, m])).values()]
      .sort((a, b) => a.id - b.id);
  } catch (e) { error.value = String(e); }
  finally {
    loading.value = false;
    if (refreshPending) { refreshPending = false; void read(false); }
  }
}
onMounted(() => { void read(false); void props.center.inspect(props.task); });
watch(() => [props.task, props.revision], () => {
  void read(false); void props.center.inspect(props.task);
});
</script>
<template>
  <section class="task-transcript">
    <h3 class="block-head">{{ t("任务对话") }}</h3>
    <button v-if="cursor" class="today-btn" type="button" :disabled="loading" @click="read(true)">{{ t("更早的 ↑") }}</button>
    <p v-if="error" role="status">{{ error }} <button class="today-btn" @click="read(!!cursor)">{{ t("重试") }}</button></p>
    <ol v-if="messages.length || activeInputs.length">
      <li v-for="m in messages" :key="m.id">
        <strong>{{ m.role === 'user' ? t("你") : 'Jeeves' }}</strong>
        <div>{{ m.role === 'assistant' ? stripToolFold(m.content) : m.content }}</div>
      </li>
      <li v-for="(input, index) in activeInputs" :key="'active-' + index">
        <strong>{{ t("你") }}</strong><div>{{ input }}</div>
      </li>
    </ol>
    <p v-else-if="!loading && !error" class="meta">{{ t("还没有任务消息。") }}</p>
    <template v-if="center.state.artifacts[task.id]?.length">
      <h3 class="block-head">{{ t("产物") }}</h3>
      <ul><li v-for="file in center.state.artifacts[task.id]" :key="file.path"><code>{{ file.path }}</code></li></ul>
    </template>
  </section>
</template>
