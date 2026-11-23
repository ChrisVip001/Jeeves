<script setup lang="ts">
import { accountKey, accountStorage } from "./hosted.ts";
import { computed, ref, watch } from "vue";
import ApprovalCard from "./ui/ApprovalCard.vue";
import { ArrowUp, Ellipsis } from "lucide-vue-next";
import { interact } from "./api";
import { workspaceActions } from "./controls";
import { t } from "./i18n";
import { webTasks } from "./tasks";
import { workStatus } from "./work-status";
import { submissionTracker } from "./ui/submission";
import TaskDelivery from "./TaskDelivery.vue";
import TaskFileReview from "./TaskFileReview.vue";
import type { TaskReviewFile } from "./protocol";
import TaskTranscript from "./ui/TaskTranscript.vue";
import type { AgentTaskView } from "./ui/types";
const props=defineProps<{task:AgentTaskView}>();
const emit=defineEmits<{details:[]}>();
const approval = computed(() => webTasks.approvalFor(props.task));
/** The task's acceptance conditions as a step list: passed, failing, or not measured yet. */
const steps = computed(() => props.task.acceptance.map((condition, index) => {
  const check = props.task.checks[index];
  return {
    label: check?.label || condition.label || condition.path || condition.kind,
    state: check === undefined ? "todo" : check.passed ? "done" : props.task.status === "running" ? "doing" : "failed",
    detail: check?.detail ?? "",
  };
}));
const tracker=submissionTracker(accountStorage);
const draftKey = accountKey("jeeves.task-draft." + props.task.id);
const text = ref(sessionStorage.getItem(draftKey) ?? "");
const historyOpen = ref(false);
const reviewFile = ref<TaskReviewFile | null>(null);
const review = computed(() => webTasks.state.reviews[props.task.id]);
let reviewOpener: HTMLElement | undefined;
function openReview(file: TaskReviewFile, opener: HTMLElement) {
  reviewOpener = opener;
  reviewFile.value = file;
}
function closeReview() {
  reviewFile.value = null;
  reviewOpener?.focus();
}
watch(text, value => {
  if (value) sessionStorage.setItem(draftKey, value);
  else sessionStorage.removeItem(draftKey);
}, { flush: "sync" });
const sending=ref(false);
const feedback=ref("");
const revision=ref(0);
async function send() {
  const message=text.value.trim();
  if(!message || sending.value) return;
  sending.value=true;
  feedback.value="";
  try {
    const submission=await tracker.prepare({
      sessionId:webTasks.state.owner, taskId:props.task.id, message, cwd:null, attachments:null,
    });
    for await (const event of interact(submission)) {
      if (event.type==="error") throw new Error(event.message);
      if (event.type==="turnCompleted") {
        tracker.complete(submission);
        if (sessionStorage.getItem(draftKey)?.trim() === message) sessionStorage.removeItem(draftKey);
        text.value="";
        feedback.value=event.reply;
        revision.value++;
        await webTasks.refresh();
        return;
      }
    }
    feedback.value=t("未收到任务回执，可重试这条补充。");
  } catch(error) { feedback.value = error instanceof Error ? error.message : String(error); }
  finally { sending.value=false; }
}
watch(() => [props.task.status, props.task.currentTurnId, props.task.roundsUsed, props.task.runCount, revision.value],
  () => void webTasks.inspect(props.task), { immediate: true });
</script>
<template>
  <div class="cowork-layout" :class="{ 'with-file-review': reviewFile }">
  <section class="cowork-detail" :aria-label="task.title" :data-task-id="task.id">
    <header class="cowork-head">
      <div class="work-title"><h2>{{ task.title }}</h2><small class="cowork-status">{{ workStatus(webTasks, task) }}</small></div>
      <button id="cowork-details" type="button" class="icon-action" :aria-label="t('任务详情')" :title="t('任务详情')" @click="emit('details')"><Ellipsis :size="21" /></button>
    </header>
    <div class="cowork-thread">
      <ol v-if="steps.length" class="work-steps" :aria-label="t('步骤')">
        <li v-for="(step, index) in steps" :key="index" :class="step.state" :title="step.detail">
          <span class="work-step-mark" aria-hidden="true">{{ step.state === "done" ? "✓" : step.state === "failed" ? "✕" : step.state === "doing" ? "◐" : "○" }}</span>
          <span class="work-step-text">{{ step.label }}<small v-if="step.detail && step.state === 'failed'" class="work-step-detail">{{ step.detail }}</small></span>
        </li>
      </ol>
      <ApprovalCard v-if="approval" :key="approval.requestId + ':' + approval.version"
        :approval="approval" :task="task" :restored="webTasks.state.restored[approval.turnId]"
        :busy="!!webTasks.state.busy" @answer="(ok, reviewed) => webTasks.approve(reviewed, ok)" />
      <p v-if="webTasks.state.error" role="alert">{{ webTasks.state.error }}</p>
      <TaskDelivery :task="task" :center="webTasks" :selected="reviewFile?.path" @open="openReview" />
      <div v-if="!approval && ['suspended','needs_attention','needsAttention'].includes(task.status)" class="td-actions">
        <button v-if="task.runCount" type="button" class="today-btn" @click="workspaceActions.reviewOperations(task)">{{ t("核对操作") }}</button>
        <button type="button" class="today-btn primary" :disabled="!!webTasks.state.busy" @click="webTasks.action('resume', task.id)">{{ t("继续任务") }}</button>
      </div>
      <details class="work-history" @toggle="historyOpen = ($event.currentTarget as HTMLDetailsElement).open">
        <summary id="work-history-toggle">{{ t("查看往来记录") }}</summary>
        <TaskTranscript v-if="historyOpen" :task="task" :center="webTasks" :revision="revision" history-only />
      </details>
    </div>
    <form class="cowork-compose" v-if="['pending','running','done'].includes(task.status)" @submit.prevent="send">
      <label class="sr" for="cowork-input">{{ t("补充要求") }}</label>
      <textarea id="cowork-input" v-model="text" :disabled="sending" :placeholder="t('给这个任务补充要求…')" rows="2" @keydown.meta.enter.prevent="send" />
      <button id="cowork-send" type="submit" class="primary-button" :disabled="sending || !text.trim()"><ArrowUp :size="16" />{{ t("发送") }}</button>
    </form>
    <p v-if="feedback" class="cw-feedback" role="status">{{ feedback }}</p>
  </section>
  <TaskFileReview v-if="reviewFile" :key="reviewFile.path" :file="reviewFile"
    :base="review?.baseCommit" :result="review?.resultCommit" @close="closeReview" />
  </div>
</template>
