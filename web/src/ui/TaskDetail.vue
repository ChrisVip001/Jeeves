<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import { X } from "lucide-vue-next";
import TaskTranscript from "./TaskTranscript.vue";
import ApprovalCard from "./ApprovalCard.vue";
import GitCheckpointForm from "./GitCheckpointForm.vue";
import { useUiText } from "./translate";
import { parseAt, progressOf, sameDay, timelineOf } from "./today";
import { taskEventText } from "../task-event-text";
import type { TaskCenter } from "./useTasks";
import type { AgentTaskView, TaskEventView } from "./types";
const t = useUiText();
const props = defineProps<{
  task: AgentTaskView;
  events?: TaskEventView[];
  center: TaskCenter;
  readOnly?: boolean;
}>();
const emit = defineEmits<{ close: []; follow: [id: string] }>();

const dialog = ref<HTMLDialogElement>();
// A modal <dialog>: the browser traps focus, Esc closes it, focus returns to the opener.
onMounted(async () => {
  await nextTick();
  dialog.value?.showModal();
});

const timeline = computed(() => timelineOf(props.events).map(event => ({ ...event, ...taskEventText(event, t) })));
const approval = computed(() => props.center.approvalFor(props.task));
const progress = computed(() => progressOf(props.task));

function statusLabel(status: string, verified: boolean) {
  const labels: Record<string, string> = {
    pending: t("等待执行"),
    running: t("正在执行"),
    suspended: t("已暂停"),
    needs_attention: t("需要你处理"),
    needsAttention: t("需要你处理"),
    done: verified ? t("已完成") : t("已生成，待验收"),
    failed: t("执行失败"),
    cancelled: t("已取消"),
  };
  return labels[status] ?? status;
}
function stamp(at: string) {
  const date = parseAt(at);
  if (!date) return at;
  const time = date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false });
  return sameDay(date, new Date())
    ? time
    : `${date.getMonth() + 1}/${date.getDate()} ${time}`;
}

const where = computed(() => {
  const task = props.task;
  const parts = [statusLabel(task.status, !!task.acceptance.length)];
  if (progress.value.rounds)
    parts.push(
      t("第 {used}/{max} 轮", {
        used: progress.value.rounds.used,
        max: progress.value.rounds.max,
      }),
    );
  return parts.join(" · ");
});
const basis = computed(() => {
  const checks = progress.value.checks;
  if (checks)
    return t("验收 {passed}/{total} 通过", { passed: checks.passed, total: checks.total });
  return t("没有设验收条件，以回复为准");
});
const next = computed(() => {
  const task = props.task;
  if (task.inputCondition?.state === "pending")
    return t("等{what}", { what: task.inputCondition.description });
  switch (task.status) {
    case "running":
      return t("正在做");
    case "pending":
      if (task.waitingForTaskIds?.length) return t("等待前置事项验收");
      return t("排队中，轮到就开始");
    case "suspended":
      return t("已暂停，你说继续才动");
    case "done":
    case "failed":
    case "cancelled":
      return t("不会再动");
    default:
      return t("等你处理后继续");
  }
});
const prerequisites = computed(() => (props.task.waitingForTaskIds ?? []).map(id =>
  props.center.state.tasks.find(task => task.id === id)?.title ?? id));
const need = computed(() => {
  if (approval.value) return t("确认一个动作，见下方");
  const task = props.task;
  if (["needs_attention", "needsAttention"].includes(task.status))
    return task.lastStopReason || task.suspendReason || t("看一下卡在哪里");
  if (task.status === "suspended") return t("想继续时点“继续”");
  return t("现在不用你做什么");
});
const busy = computed(() => !!props.center.state.busy);
</script>
<template>
  <dialog
    ref="dialog"
    class="task-detail"
    :aria-labelledby="`td-${task.id}`"
    @close="emit('close')"
    @click.self="dialog?.close()"
  >
    <div class="td-inner">
      <header class="td-head">
        <div>
          <p class="meta">{{ where }}</p>
          <h2 :id="`td-${task.id}`">{{ task.title }}</h2>
        </div>
        <button
          type="button"
          class="today-icon"
          :aria-label="t('关闭')"
          @click="dialog?.close()"
        >
          <X :size="17" aria-hidden="true" />
        </button>
      </header>
      <div v-if="!readOnly && $slots.executor" class="executor-inline"><slot name="executor" /></div>

      <p v-if="center.state.error" role="alert">{{ center.state.error }}</p>
      <p v-if="center.state.late" role="status">{{ center.state.late.answeredBy
        ? t("晚了一步：这条已由 {who} 回答。", {who: center.state.late.answeredBy}) : t("这条请求已变更或结束，请核对当前待确认事项。") }}</p>
      <ApprovalCard
        v-if="approval"
        :key="`${approval.requestId}-${approval.version}`"
        :approval="approval"
        :task="task"
        :restored="center.state.restored[approval.turnId]"
        :busy="busy"
        @answer="(ok, reviewed) => center.approve(reviewed, ok)"
      />

      <p v-if="task.inputCondition?.state === 'pending'" class="meta">{{ t("等{what}", {what: task.inputCondition.description}) }}</p>
      <p v-else-if="['failed','suspended','needs_attention','needsAttention'].includes(task.status)" class="meta">{{ task.lastStopReason || task.suspendReason }}</p>
      <p v-if="prerequisites.length" class="meta">{{ t("等待这些事项验收通过：{tasks}", { tasks: prerequisites.join(" · ") }) }}</p>
      <TaskTranscript :center="center" :task="task" />
      <details class="execution-details">
        <summary>{{ t("执行记录") }}</summary>
        <dl class="facts four">
          <dt>{{ t("做到哪了") }}</dt>
          <dd>{{ where }}</dd>
          <dt>{{ t("凭什么这么说") }}</dt>
          <dd>
            {{ basis }}
            <ul v-if="task.checks.length" class="checks">
              <li v-for="(check, index) in task.checks" :key="index" :class="{ failed: !check.passed }">
                <span class="check-mark" aria-hidden="true">{{ check.passed ? "✓" : "×" }}</span>
                <span class="sr">{{ check.passed ? t("通过") : t("未通过") }}</span>
                {{ check.label }}<span v-if="check.detail" class="meta"> · {{ check.detail }}</span>
              </li>
            </ul>
          </dd>
          <dt>{{ t("什么时候再动") }}</dt>
          <dd>{{ next }}</dd>
          <dt>{{ t("需要你做什么") }}</dt>
          <dd>{{ need }}</dd>
        </dl>

        <slot v-if="!readOnly" name="desktop" :task="task" />

        <section class="td-timeline" :aria-labelledby="`tl-${task.id}`">
          <h3 :id="`tl-${task.id}`" class="block-head">
            {{ t("经过") }}<span class="count tnum">{{ timeline.length }}</span>
          </h3>
          <p v-if="!timeline.length" class="meta">
            {{ t("还没有记录。任务每推进一步，这里多一行。") }}
          </p>
          <ol v-else class="tl">
            <li v-for="event in timeline" :key="event.cursor">
              <time class="tnum" :datetime="event.at">{{ stamp(event.at) }}</time>
              <span class="tl-kind">{{ event.label }}</span>
              <span v-if="event.detail" class="tl-text">{{ event.detail }}</span>
            </li>
          </ol>
        </section>

        <GitCheckpointForm v-if="!readOnly" :task="task" :busy="busy" :submit="center.gitCheckpoint" :read-policy="center.gitCheckpointPolicy" :write-policy="center.saveGitCheckpointPolicy" />
      </details>
      <footer v-if="!readOnly || ['pending', 'running', 'done'].includes(task.status)" class="td-actions">
        <button
          v-if="['pending', 'running', 'done'].includes(task.status)"
          type="button"
          class="today-btn"
          @click="emit('follow', task.id)"
        >
          {{ readOnly ? t("在主聊天跟进") : t("补充要求") }}
        </button>
        <button
          v-if="!readOnly && ['pending', 'running'].includes(task.status)"
          type="button"
          class="today-btn"
          :disabled="busy"
          @click="center.action('suspend', task.id)"
        >
          {{ t("暂停任务") }}
        </button>
        <button
          v-if="!readOnly && ['suspended', 'needs_attention', 'needsAttention'].includes(task.status)"
          type="button"
          class="today-btn primary"
          :disabled="busy"
          @click="center.action('resume', task.id)"
        >
          {{ t("继续任务") }}
        </button>
        <button
          v-if="!readOnly && !['done', 'failed', 'cancelled'].includes(task.status)"
          type="button"
          class="today-btn quiet"
          :disabled="busy"
          @click="center.action('cancel', task.id)"
        >
          {{ t("取消任务") }}
        </button>
      </footer>
    </div>
  </dialog>
</template>
