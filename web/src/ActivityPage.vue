<script setup lang="ts">
import { computed, ref } from "vue";
import { ChevronRight, Search } from "lucide-vue-next";
import { t } from "./i18n";
import ApprovalCard from "./ui/ApprovalCard.vue";
import type { TaskCenter } from "./ui/useTasks";
import { webPermission, trustAndAllow } from "./tasks";
import type { AgentTaskView } from "./protocol";
import { formatWhen } from "./render";
import { workSections, workStatus } from "./work-status";
const props = defineProps<{ center: TaskCenter; selected?: string }>();
defineEmits<{ inspect: [id: string]; chat: [] }>();
/** Finished work stays short: the latest few, the rest one click away. */
const RECENT = 5;
const search = ref("");
const showAllPast = ref(false);
const query = computed(() => search.value.trim().toLocaleLowerCase());
const chatRequests = computed(() => Object.values(props.center.state.approvals).filter(request => !props.center.taskOf(request)));
const loaded = computed(() => props.center.state.loadedOwner === props.center.state.owner);
const total = computed(() => props.center.state.tasks.length + chatRequests.value.length);

const status = (task: AgentTaskView) => workStatus(props.center, task);
/** Why it waits, in the task's own words when it has them. */
const reason = (task: AgentTaskView) =>
  task.lastStopReason || task.suspendReason || (task.inputCondition?.state === "pending" ? task.inputCondition.description : "") || status(task);
const when = (task: AgentTaskView) => formatWhen(task.finishedAt ?? task.createdAt);
const tone = (task: AgentTaskView) =>
  task.status === "running" ? "live" : task.status === "failed" ? "bad" : ["done", "cancelled"].includes(task.status) ? "done" : "idle";

const sections = computed(() => workSections(props.center, query.value));
const requests = computed(() => chatRequests.value.filter(request => !query.value || request.tool.toLocaleLowerCase().includes(query.value)));
const past = computed(() => {
  const all = sections.value.past;
  if (showAllPast.value || query.value) return all;
  const recent = all.slice(0, RECENT);
  const open = all.find(task => task.id === props.selected);
  return open && !recent.includes(open) ? [...recent, open] : recent;
});
const waitingCount = computed(() => sections.value.waiting.length + requests.value.length);
const summary = computed(() => {
  const parts: string[] = [];
  if (waitingCount.value) parts.push(t("{n} 件等你决定", { n: waitingCount.value }));
  if (sections.value.working.length) parts.push(t("{n} 件进行中", { n: sections.value.working.length }));
  return parts.length ? parts.join(" · ") : t("都处理完了，没有等你的事。");
});
const nothingFound = computed(() => query.value && !waitingCount.value && !sections.value.working.length && !sections.value.past.length);
</script>

<template>
  <section class="activity-page" aria-labelledby="activity-title">
    <header class="work-list-head">
      <div class="work-list-title">
        <h1 id="activity-title">{{ t("工作") }}</h1>
        <p v-if="loaded && total" class="work-summary" role="status">{{ summary }}</p>
      </div>
      <label v-if="total" class="work-search"><Search :size="16" /><input v-model="search" type="search" :aria-label="t('搜索事项')" :placeholder="t('搜索事项')" /></label>
    </header>
    <!-- Recurring-work reviews wait on you, so they sit under the title; the 长期 list goes last. -->
    <slot />
    <p v-if="nothingFound" class="activity-note" role="status">{{ t("没有找到匹配的事项。") }}</p>
    <p v-if="center.state.error" role="alert">{{ center.state.error }} <button type="button" class="today-btn" @click="center.retry">{{ t("重试") }}</button></p>
    <p v-else-if="!loaded" class="activity-note" role="status">{{ t("正在读取任务…") }}</p>
    <div v-else-if="!total" class="activity-empty">
      <h2>{{ t("还没有交办的事") }}</h2>
      <p>{{ t("交办的事，结果会出现在这里。") }}</p>
      <button type="button" class="today-btn primary" @click="$emit('chat')">{{ t("回到对话") }}</button>
    </div>
    <template v-if="loaded">
      <section v-if="waitingCount" class="activity-group waiting">
        <h2>{{ t("等你决定") }}</h2>
        <ApprovalCard v-for="request in requests" :key="request.requestId + ':' + request.version"
          :approval="request" :restored="center.state.restored[request.turnId]" :busy="!!center.state.busy"
          @answer="(ok, reviewed) => center.approve(reviewed, ok)"
          :permission="webPermission" @trust="trustAndAllow" />
        <button v-for="task in sections.waiting" :key="task.id" :id="'cowork-task-'+task.id" :data-cowork-task="task.id"
          class="activity-task waiting-card" type="button" :aria-current="task.id === selected ? 'page' : undefined" @click="$emit('inspect', task.id)">
          <span class="waiting-text"><strong>{{ task.title }}</strong><small>{{ reason(task) }}</small></span>
          <span class="waiting-go">{{ t("去处理") }}<ChevronRight :size="15" aria-hidden="true" /></span>
        </button>
      </section>
      <section v-if="sections.working.length" class="activity-group">
        <h2>{{ t("进行中") }}</h2>
        <button v-for="task in sections.working" :key="task.id" :id="'cowork-task-'+task.id" :data-cowork-task="task.id"
          class="activity-task" type="button" :aria-current="task.id === selected ? 'page' : undefined" @click="$emit('inspect', task.id)">
          <i :class="['work-dot', tone(task)]" aria-hidden="true"></i>
          <span class="work-text"><strong>{{ task.title }}</strong><small>{{ status(task) }}</small></span>
          <time>{{ when(task) }}</time>
        </button>
      </section>
      <section v-if="sections.past.length" class="activity-group">
        <h2>{{ t("已结束") }}</h2>
        <button v-for="task in past" :key="task.id" :id="'cowork-task-'+task.id" :data-cowork-task="task.id"
          class="activity-task" type="button" :aria-current="task.id === selected ? 'page' : undefined" @click="$emit('inspect', task.id)">
          <i :class="['work-dot', tone(task)]" aria-hidden="true"></i>
          <span class="work-text"><strong>{{ task.title }}</strong><small>{{ task.status === "failed" && task.lastStopReason ? task.lastStopReason : status(task) }}</small></span>
          <time>{{ when(task) }}</time>
        </button>
        <button v-if="!query && sections.past.length > RECENT" type="button" class="text-action work-more" @click="showAllPast = !showAllPast">
          {{ showAllPast ? t("收起") : t("显示全部（{n}）", { n: sections.past.length }) }}
        </button>
      </section>
    </template>
    <slot name="after" />
  </section>
</template>
