<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, useId } from "vue";
import { useUiText } from "./translate";
import { minutesLeft } from "./today";
import type { AgentTaskView, ApprovalRequestedNotification } from "./types";
import TrustSwitch from "./TrustSwitch.vue";
import type { SessionPermission } from "./usePermission";
const t = useUiText();
const heading = useId();
const props = defineProps<{
  approval: ApprovalRequestedNotification;
  task?: AgentTaskView;
  restored?: boolean;
  busy?: boolean;
  allowId?: string;
  /** When given, the card also offers to stop asking for the rest of this session. */
  permission?: SessionPermission;
}>();
const emit = defineEmits<{
  answer: [approved: boolean, reviewed: ApprovalRequestedNotification];
  trust: [reviewed: ApprovalRequestedNotification];
}>();

// The wire carries "how long from now"; pin it to a deadline when the card first sees it.
const deadline = Date.now() + props.approval.expiresInMs;
const now = ref(Date.now());
let tick: ReturnType<typeof setInterval> | undefined;
onMounted(() => (tick = setInterval(() => (now.value = Date.now()), 15000)));
onUnmounted(() => clearInterval(tick));
const left = computed(() => minutesLeft(deadline, now.value));

const tool = computed(() => props.approval.tool);
const action = computed(() => {
  switch (tool.value) {
    case "run_command":
      return t("在任务执行环境运行一条命令");
    case "write_file":
      return t("写入一个文件");
    case "edit":
    case "apply_patch":
      return t("修改文件");
    case "fetch_url":
      return t("打开一个网页");
    case "feishu_send":
    case "email_send":
      return t("发出一条消息，发出后不能撤回");
    default:
      return t("调用工具 {tool}", { tool: tool.value });
  }
});
/** Full access still asks before a send that cannot be taken back, so a send card never offers it. */
const outward = computed(() => ["feishu_send", "email_send"].includes(tool.value));
const workspace = computed(() => props.task?.worktreePath ?? props.task?.cwd);
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
      <template v-if="workspace && ['run_command', 'write_file', 'edit', 'apply_patch'].includes(tool)">
        <dt>{{ t("工作目录") }}</dt>
        <dd class="dest" :title="workspace"><bdi>{{ workspace }}</bdi></dd>
      </template>
      <dt>{{ t("预览") }}</dt>
      <dd><pre class="preview-box">{{ approval.argsPreview }}</pre></dd>
    </dl>
    <p class="meta why">
      {{ t("为什么问你：{reason}", { reason: approval.reason }) }}
      <template v-if="restored"> {{ t("这是重启前问的，允许后只放行同一个动作一次。") }}</template>
    </p>
    <div class="actions">
      <button type="button" class="today-btn" :disabled="busy" @click="emit('answer', false, approval)">
        {{ t("拒绝") }}
      </button>
      <button
        type="button"
        class="today-btn primary"
        :id="allowId"
        :disabled="busy || left === null"
        @click="emit('answer', true, approval)"
      >
        {{ t("允许一次") }}
      </button>
    </div>
    <TrustSwitch v-if="permission && !outward && left !== null" :permission="permission"
      :busy="busy || permission.state.busy" @trust="emit('trust', approval)" />
  </article>
</template>
