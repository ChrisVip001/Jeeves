<script setup lang="ts">
import { accountStorage } from "./hosted.ts";
import { computed, ref } from "vue";
import ApprovalCard from "./ui/ApprovalCard.vue";
import { ArrowUp } from "lucide-vue-next";
import { interact } from "./api";
import { t } from "./i18n";
import { webTasks } from "./tasks";
import { submissionTracker } from "./ui/submission";
import TaskTranscript from "./ui/TaskTranscript.vue";
import type { AgentTaskView } from "./ui/types";
const props=defineProps<{task:AgentTaskView}>();
const emit=defineEmits<{details:[]; back:[]}>();
const approval = computed(() => webTasks.approvalFor(props.task));
const tracker=submissionTracker(accountStorage);
const text=ref("");
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
        text.value="";
        feedback.value=event.reply;
        revision.value++;
        await webTasks.refresh();
        return;
      }
    }
    feedback.value=t("未收到任务回执，可重试这条补充。");
  } catch(error) { feedback.value=String(error); }
  finally { sending.value=false; }
}
</script>
<template>
  <section class="cowork-detail" :aria-label="task.title" :data-task-id="task.id">
    <header class="cowork-head">
      <div><button type="button" class="activity-back" @click="emit('back')">‹ {{ t("动态") }}</button><h2>{{ task.title }}</h2></div>
      <button id="cowork-details" type="button" class="today-btn" @click="emit('details')">{{ t("任务详情") }}</button>
    </header>
    <div class="cowork-thread">
      <ApprovalCard v-if="approval" :key="approval.requestId + ':' + approval.version"
        :approval="approval" :task="task" :restored="webTasks.state.restored[approval.turnId]"
        :busy="!!webTasks.state.busy" @answer="(ok, reviewed) => webTasks.approve(reviewed, ok)" />
      <p v-if="webTasks.state.error" role="alert">{{ webTasks.state.error }}</p>
      <TaskTranscript :task="task" :center="webTasks" :revision="revision" />
    </div>
    <form class="cowork-compose" v-if="['pending','running','done'].includes(task.status)" @submit.prevent="send">
      <label for="cowork-input">{{ t("补充要求") }}</label>
      <textarea id="cowork-input" v-model="text" :disabled="sending" :placeholder="t('给这个任务补充要求…')" rows="2" @keydown.meta.enter.prevent="send" />
      <button id="cowork-send" type="submit" class="primary-button" :disabled="sending || !text.trim()"><ArrowUp :size="16" />{{ t("发送") }}</button>
    </form>
    <p v-if="feedback" class="cw-feedback" role="status">{{ feedback }}</p>
  </section>
</template>
