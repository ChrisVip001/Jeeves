<script setup lang="ts">
import { computed, ref } from "vue";
import { ChevronRight, Search } from "lucide-vue-next";
import { t } from "./i18n";
import ApprovalCard from "./ui/ApprovalCard.vue";
import type { TaskCenter } from "./ui/useTasks";
import type { AgentTaskView } from "./protocol";
import { workSections, workStatus } from "./work-status";
const props = defineProps<{ center: TaskCenter }>();
defineEmits<{ inspect: [id: string]; chat: [] }>();
const search = ref("");
const query = computed(() => search.value.trim().toLocaleLowerCase());
const chatRequests = computed(() => Object.values(props.center.state.approvals).filter(request => !props.center.taskOf(request)));

const status = (task: AgentTaskView) => workStatus(props.center, task);

const sections = computed(() => {
  const { waiting, working, past } = workSections(props.center, query.value);
  return [
    { label: t("等你决定"), rows: waiting, requests: chatRequests.value.filter(request => !query.value || request.tool.toLocaleLowerCase().includes(query.value)) },
    { label: t("进行中"), rows: working, requests: [] },
    { label: t("已结束"), rows: past, requests: [] },
  ];
});
</script>

<template>
  <section class="activity-page" aria-labelledby="activity-title">
    <header class="work-list-head"><h1 id="activity-title">{{ t("我的事项") }}</h1>
      <label v-if="center.state.tasks.length || chatRequests.length" class="work-search"><Search :size="18" /><input v-model="search" type="search" :aria-label="t('搜索事项')" :placeholder="t('搜索事项')" /></label>
    </header>
    <!-- Recurring-work reviews wait on you, so they sit under the title; the 长期 list goes last. -->
    <slot />
    <p v-if="query && sections.every(s => !s.rows.length && !s.requests.length)" role="status">{{ t("没有找到匹配的事项。") }}</p>
    <p v-if="center.state.error" role="alert">{{ center.state.error }} <button type="button" class="today-btn" @click="center.retry">{{ t("重试") }}</button></p>
    <p v-else-if="!center.state.loadedOwner || center.state.loadedOwner !== center.state.owner" role="status">{{ t("正在读取任务…") }}</p>
    <div v-else-if="!center.state.tasks.length && !chatRequests.length" class="activity-empty">
      <p>{{ t("交办的事，结果会出现在这里。") }}</p>
      <button type="button" class="today-btn" @click="$emit('chat')">{{ t("回到对话") }}</button>
    </div>
    <template v-for="section in sections" :key="section.label">
      <section v-if="center.state.loadedOwner === center.state.owner && (section.rows.length || section.requests.length)" class="activity-group">
        <h2>{{ section.label }} <span>{{ section.rows.length + section.requests.length }}</span></h2>
        <ApprovalCard v-for="request in section.requests" :key="request.requestId + ':' + request.version"
          :approval="request" :restored="center.state.restored[request.turnId]" :busy="!!center.state.busy"
          @answer="(ok, reviewed) => center.approve(reviewed, ok)" />
        <button v-for="task in section.rows" :key="task.id" :id="'cowork-task-'+task.id" :data-cowork-task="task.id"
          class="activity-task" type="button" @click="$emit('inspect', task.id)">
          <strong>{{ task.title }}</strong><small>{{ status(task) }}</small><ChevronRight :size="16" aria-hidden="true" />
        </button>
      </section>
    </template>
    <slot name="after" />
  </section>
</template>
