<script setup lang="ts">
import { computed } from "vue";
import { useUiText } from "./translate";
import { latestEvent, parseAt, progressOf } from "./today";
import { taskEventText } from "../task-event-text";
import type { AgentTaskView, TaskEventView } from "./types";
const t = useUiText();
const props = defineProps<{
  task: AgentTaskView;
  events?: TaskEventView[];
  /** Shown as the trailing action when the row needs the human. */
  action?: string;
  tone?: "warn" | "idle" | "active";
}>();
const emit = defineEmits<{ open: [id: string] }>();

/** Where it is, then what it rests on: one grey line, measured facts before adjectives. */
const phrase = computed(() => {
  const task = props.task;
  const parts: string[] = [];
  const reason = task.lastStopReason || task.suspendReason;
  if (props.tone === "warn" && reason) parts.push(reason);
  const progress = progressOf(task);
  if (progress.checks)
    parts.push(
      t("验收 {passed}/{total} 通过", {
        passed: progress.checks.passed,
        total: progress.checks.total,
      }),
    );
  if (progress.rounds)
    parts.push(t("第 {used}/{max} 轮", { used: progress.rounds.used, max: progress.rounds.max }));
  const event = latestEvent(props.events);
  if (event && parts.length < 2) {
    const { label, detail, known } = taskEventText(event, t);
    if (detail) parts.push(detail);
    else if (known) parts.push(label);
  }
  if (!parts.length) parts.push(t("还没有进展记录"));
  return parts.join(" · ");
});

/** When it moves next. */
const when = computed(() => {
  const task = props.task;
  if (task.inputCondition?.state === "pending")
    return t("等{what}", { what: task.inputCondition.description });
  switch (task.status) {
    case "running":
      return t("正在做");
    case "pending":
      return t("排队中");
    case "suspended":
      return t("已暂停，你说继续才动");
    case "done":
    case "failed":
    case "cancelled": {
      const at = parseAt(task.finishedAt);
      return at
        ? at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false })
        : "";
    }
    default:
      return t("等你");
  }
});
const dot = computed(() => props.tone ?? (props.task.status === "suspended" ? "idle" : "active"));
</script>
<template>
  <div class="today-row" :class="{ 'has-action': action }">
    <i class="dot" :class="dot" aria-hidden="true"></i>
    <button type="button" class="row-open" @click="emit('open', task.id)">
      <span class="title">{{ task.title }}</span>
      <span class="phrase">{{ phrase }}</span>
    </button>
    <span class="when tnum">{{ when }}</span>
    <button v-if="action" type="button" class="today-btn" @click="emit('open', task.id)">
      {{ action }}
    </button>
  </div>
</template>
