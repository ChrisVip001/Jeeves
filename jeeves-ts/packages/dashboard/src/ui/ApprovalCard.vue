<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useId } from "vue";
import { useUiText } from "./translate";
import { minutesLeft } from "./today";
import type { AgentTaskView, ApprovalRequestedNotification } from "./types";
const t = useUiText();
const heading = useId();
const props = defineProps<{
  approval: ApprovalRequestedNotification;
  task?: AgentTaskView;
  restored?: boolean;
  busy?: boolean;
}>();
const emit = defineEmits<{ answer: [approved: boolean] }>();

// The wire carries "how long from now"; pin it to a deadline when the card first sees it.
const deadline = Date.now() + props.approval.expiresInMs;
const now = ref(Date.now());
let tick: ReturnType<typeof setInterval> | undefined;
onMounted(() => (tick = setInterval(() => (now.value = Date.now()), 15000)));
onUnmounted(() => clearInterval(tick));
const left = computed(() => minutesLeft(deadline, now.value));

const tool = computed(() => props.approval.tool);
const sends = computed(() => /feishu|lark|telegram|notify|send/.test(tool.value));
const action = computed(() => {
  switch (tool.value) {
    case "run_command":
      return t("在本机运行一条命令");
    case "write_file":
      return t("写入一个文件");
    case "edit":
    case "apply_patch":
      return t("修改文件");
    case "fetch_url":
      return t("打开一个网页");
    default:
      return sends.value
        ? t("发出一条消息，发出后不能撤回")
        : t("调用工具 {tool}", { tool: tool.value });
  }
});
// Neither field is on the wire yet (see the M1-07 report); these are read from the task.
const destination = computed(() => {
  if (sends.value) return t("见下方预览");
  const where = props.task?.worktreePath ?? props.task?.cwd;
  return where ?? t("主会话的工作目录");
});
const identity = computed(() =>
  sends.value ? t("你配置的机器人") : t("Jeeves，用你在这台电脑上的权限"),
);
const title = computed(() => props.task?.title ?? t("主会话里的一步"));
</script>
<template>
  <article class="today-card confirm" :aria-labelledby="heading">
    <div class="approval-head">
      <h3 :id="heading">{{ title }}</h3>
      <span class="meta tnum">{{
        left === null
          ? t("已过期，按拒绝处理")
          : t("{minutes} 分钟后过期", { minutes: left })
      }}</span>
    </div>
    <dl class="facts">
      <dt>{{ t("动作") }}</dt>
      <dd>{{ action }}</dd>
      <dt>{{ t("目标") }}</dt>
      <dd class="dest" :title="destination"><bdi>{{ destination }}</bdi></dd>
      <dt>{{ t("身份") }}</dt>
      <dd>{{ identity }}</dd>
      <dt>{{ t("预览") }}</dt>
      <dd><pre class="preview-box">{{ approval.argsPreview }}</pre></dd>
    </dl>
    <p class="meta why">
      {{ t("为什么问你：{reason}", { reason: approval.reason }) }}
      <template v-if="restored"> {{ t("这是重启前问的，允许后只放行同一个动作一次。") }}</template>
    </p>
    <div class="actions">
      <button type="button" class="today-btn" :disabled="busy" @click="emit('answer', false)">
        {{ t("拒绝") }}
      </button>
      <button
        type="button"
        class="today-btn primary"
        :disabled="busy || left === null"
        @click="emit('answer', true)"
      >
        {{ t("允许一次") }}
      </button>
    </div>
  </article>
</template>
