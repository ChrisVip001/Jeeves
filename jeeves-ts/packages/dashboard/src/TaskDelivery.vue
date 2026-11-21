<script setup lang="ts">
import { computed } from "vue";
import { Check, Download, FileText } from "lucide-vue-next";
import { t } from "./i18n";
import { renderMarkdown, stripToolFold } from "./markdown";
import type { AgentTaskView } from "./protocol";
import type { TaskCenter } from "./ui/useTasks";

const props = defineProps<{ task: AgentTaskView; center: TaskCenter }>();
const delivery = computed(() => props.center.state.deliveries[props.task.id]);
const files = computed(() => props.center.state.artifacts[props.task.id] ?? []);
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
const paint = (el: HTMLElement, value: string) => el.replaceChildren(renderMarkdown(stripToolFold(value)));
const vMarkdown = {
  mounted(el: HTMLElement, binding: { value: string }) { paint(el, binding.value); },
  updated(el: HTMLElement, binding: { value: string; oldValue: string }) { if (binding.value !== binding.oldValue) paint(el, binding.value); },
};
const filename = (path: string) => path.split(/[\\/]/).at(-1) || path;
function saveText(path: string, text: string) {
  const link = document.createElement("a");
  const url = URL.createObjectURL(new Blob([text], {type:"text/plain;charset=utf-8"}));
  link.href = url;
  link.download = /\.(txt|md|csv|json|log)$/i.test(path) ? filename(path) : filename(path) + ".txt";
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
</script>
<template>
  <article class="delivery">
    <p v-if="status" class="live-status" role="status">{{ status }}</p>
    <div v-if="delivery || reply" class="delivery-answer">
      <h3>{{ delivery ? live ? t("上次执行结果") : t("本次交付") : t("最近的回复") }}</h3>
      <p v-if="delivery" class="delivery-verdict">{{ receiptStatus(delivery.status) }}</p>
      <div v-if="reply" v-markdown="reply" class="bubble md"></div>
      <p v-else-if="!live && task.lastStopReason">{{ task.lastStopReason }}</p>
    </div>
    <section v-if="files.length" class="work-files">
      <h3>{{ t("已记录的文件") }}</h3>
      <div v-for="file in files" :key="file.path">
        <details v-if="file.writtenText != null" class="work-file">
          <summary><FileText :size="18" /><span>{{ filename(file.path) }}</span><small>{{ t("写入时的文本") }}</small></summary>
          <pre>{{ file.writtenText }}</pre>
          <button type="button" class="text-action" @click="saveText(file.path, file.writtenText)"><Download :size="15" />{{ t("保存文本副本") }}</button>
        </details>
        <p v-else class="file-record" :title="file.path"><FileText :size="18" /><span>{{ filename(file.path) }}</span><small>{{ t("仅有文件记录") }}</small></p>
      </div>
    </section>
    <details v-if="task.checks.length" class="work-checks">
      <summary>{{ t("验收结果") }}</summary>
      <p v-for="check in task.checks" :key="check.label"><Check v-if="check.passed" :size="16" />{{ check.passed ? t("通过") : t("未通过") }} · {{ check.label }}<br>{{ check.detail }}</p>
    </details>
  </article>
</template>
