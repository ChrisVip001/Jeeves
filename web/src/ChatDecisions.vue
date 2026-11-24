<script setup lang="ts">
import { computed } from "vue";
import { t } from "./i18n";
import ApprovalCard from "./ui/ApprovalCard.vue";
import type { TaskCenter } from "./ui/useTasks";
import { webPermission, trustAndAllow } from "./tasks";
const props = defineProps<{ center: TaskCenter; mainBusy: boolean }>();
const requests = computed(() => Object.values(props.center.state.approvals)
  .filter(request => !props.mainBusy || props.center.taskOf(request))
  .sort((a, b) => a.expiresInMs - b.expiresInMs));
</script>

<template>
  <details v-if="requests.length || center.state.late || center.state.error" class="chat-decisions" :open="requests.length === 1">
    <summary id="chat-review-requests">{{ requests.length ? t("待确认（{count}）", { count: requests.length }) : t("任务通知") }}</summary>
    <p v-if="center.state.error" role="alert">{{ center.state.error }} <button type="button" class="today-btn" @click="center.retry">{{ t("重试") }}</button></p>
    <p v-if="center.state.late" role="status">{{ center.state.late.answeredBy
      ? t("晚了一步：这条已由 {who} 回答。", {who: center.state.late.answeredBy}) : t("这条请求已变更或结束，请核对当前待确认事项。") }}</p>
    <ApprovalCard v-for="request in requests" :key="request.requestId + ':' + request.version"
      :allow-id="'allow-' + request.requestId + '-' + request.version"
      :approval="request" :task="center.taskOf(request)" :restored="center.state.restored[request.turnId]"
      :busy="!!center.state.busy" @answer="(ok, reviewed) => center.approve(reviewed, ok)"
      :permission="webPermission" @trust="trustAndAllow" />
  </details>
</template>
