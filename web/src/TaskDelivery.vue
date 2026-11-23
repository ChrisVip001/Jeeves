<script setup lang="ts">
import { computed } from "vue";
import { Check, ChevronRight, FileText } from "lucide-vue-next";
import { taskReceiptBody } from "./render";
import type { TaskDeliveryView } from "./protocol";
import { t } from "./i18n";
import { renderMarkdown, stripToolFold } from "./markdown";
import type { AgentTaskView, TaskReviewFile } from "./protocol";
import type { TaskCenter } from "./ui/useTasks";

const props = defineProps<{ task: AgentTaskView; center: TaskCenter; selected?: string }>();
const emit = defineEmits<{open: [TaskReviewFile, HTMLElement]}>();
const delivery = computed(() => props.center.state.deliveries[props.task.id]);
const review = computed(() => props.center.state.reviews[props.task.id]);
const files = computed(() => {
  const reviewed = review.value?.files ?? [];
  if (review.value) return reviewed;
  return (props.center.state.artifacts[props.task.id] ?? []).map(file => ({
    path: file.path, change: "recorded", content: file.writtenText ?? null, patch: null,
  }));
});
function statusLabel(change: string) {
  if (change === "added") return t("新增");
  if (change === "deleted") return t("删除");
  if (change === "modified") return t("修改");
  return "";
}
const reply = computed(() => delivery.value ? delivery.value.summary : props.center.state.results[props.task.id]);
const live = computed(() => ["pending", "running", "suspended", "needs_attention", "needsAttention"].includes(props.task.status));
const status = computed(() => {
  const task = props.task;
  if (props.center.approvalFor(task)) return t("确认后继续");
  if (task.inputCondition?.state === "pending") return t("等{what}", {what: task.inputCondition.description});
  if (task.status === "running") return t("正在处理这件事。");
  if (task.status === "pending") return t("等待任务开始");
  if (live.value) return task.suspendReason || task.lastStopReason || t("已暂停");
  return "";
});
function receiptStatus(value: string) {
  switch (value) {
    case "done": return t("验收通过");
    case "failed": return t("执行失败");
    case "cancelled": return t("已取消");
    default: return t("已生成，待验收");
  }
}
const paintReceipt = (el: HTMLElement, delivery: TaskDeliveryView) =>
  el.replaceChildren(taskReceiptBody(delivery.approvalDecisions, delivery.summary ?? ""));
const vReceipt = {
  mounted(el: HTMLElement, { value }: { value: TaskDeliveryView }) { paintReceipt(el, value); },
  updated(el: HTMLElement, { value, oldValue }: { value: TaskDeliveryView; oldValue: TaskDeliveryView }) {
    if (value.summary !== oldValue.summary || value.runId !== oldValue.runId ||
        JSON.stringify(value.approvalDecisions) !== JSON.stringify(oldValue.approvalDecisions)) paintReceipt(el, value);
  },
};
const paint = (el: HTMLElement, value: string) => el.replaceChildren(renderMarkdown(stripToolFold(value)));
const vMarkdown = {
  mounted(el: HTMLElement, binding: { value: string }) { paint(el, binding.value); },
  updated(el: HTMLElement, binding: { value: string; oldValue: string }) { if (binding.value !== binding.oldValue) paint(el, binding.value); },
};
</script>
<template>
  <article class="delivery">
    <p v-if="status" class="live-status" role="status">{{ status }}</p>
    <div v-if="delivery || reply" class="delivery-answer">
      <h3>{{ delivery ? live ? t("上次执行结果") : t("本次交付") : t("最近的回复") }}</h3>
      <p v-if="delivery" class="delivery-verdict">{{ receiptStatus(delivery.status) }}</p>
      <div v-if="delivery" v-receipt="delivery"></div>
      <div v-else-if="reply" v-markdown="reply" class="bubble md"></div>
      <p v-if="!reply && !live && task.lastStopReason">{{ task.lastStopReason }}</p>
    </div>
    <section v-if="files.length || review?.omittedFiles" class="work-files">
      <h3>{{ t("已记录的文件") }}</h3>
      <button v-for="file in files" :key="file.path" type="button" class="work-file"
        :aria-expanded="selected === file.path" @click="emit('open', file, $event.currentTarget as HTMLElement)">
        <FileText :size="17" /><span>{{ file.path }}</span><small v-if="statusLabel(file.change)">{{ statusLabel(file.change) }}</small><ChevronRight :size="16" />
      </button>
      <p v-if="review?.omittedFiles" class="review-empty">{{ t("另有{count}个文件超出审阅上限。", {count: review.omittedFiles}) }}</p>
    </section>
    <details v-if="task.checks.length" class="work-checks">
      <summary>{{ t("验收结果") }}</summary>
      <p v-for="check in task.checks" :key="check.label"><Check v-if="check.passed" :size="16" />{{ check.passed ? t("通过") : t("未通过") }} · {{ check.label }}<template v-if="check.detail"><br>{{ check.detail }}</template></p>
    </details>
  </article>
</template>
