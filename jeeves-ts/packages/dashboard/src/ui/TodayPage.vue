<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import ApprovalCard from "./ApprovalCard.vue";
import JeevesMark from "./JeevesMark.vue";
import TaskRow from "./TaskRow.vue";
import { useUiText } from "./translate";
import { buildToday, dayPart, parseAt } from "./today";
import type { TaskCenter } from "./useTasks";
import type { TaskEventView } from "./types";
const t = useUiText();
const props = defineProps<{
  center: TaskCenter;
  timelines: Record<string, TaskEventView[]>;
  /** Fixed clock for previews and tests; live otherwise. */
  now?: Date;
}>();
const emit = defineEmits<{ open: [id: string] }>();

const tick = ref(Date.now());
let timer: ReturnType<typeof setInterval> | undefined;
onMounted(() => (timer = setInterval(() => (tick.value = Date.now()), 60000)));
onUnmounted(() => clearInterval(timer));
const now = computed(() => props.now ?? new Date(tick.value));

const model = computed(() =>
  buildToday(
    props.center.state.tasks,
    Object.values(props.center.state.approvals),
    props.center.taskOf,
    now.value,
  ),
);
const working = computed(
  () => model.value.moving.filter((task) => task.status !== "suspended").length,
);
const empty = computed(
  () => !model.value.needs.length && !model.value.moving.length && !model.value.finished.length,
);

const hello = computed(
  () =>
    ({
      night: t("夜深了。"),
      morning: t("早上好。"),
      noon: t("中午好。"),
      afternoon: t("下午好。"),
      evening: t("晚上好。"),
    })[dayPart(now.value)],
);
/** One judgment after the greeting: what needs you first, otherwise what is moving. */
const lead = computed(() => {
  const needs = model.value.needs.length;
  if (needs === 1) return t("有一件事等你。");
  if (needs > 1) return t("有 {count} 件事等你。", { count: needs });
  if (working.value)
    return t("{count} 件事在推进，暂时不用你管。", { count: working.value });
  if (model.value.finished.length)
    return t("今天做完了 {count} 件。", { count: model.value.finished.length });
  return "";
});
const sub = computed(() => {
  if (model.value.needs.length && working.value)
    return t("另外 {count} 件在推进，不用你管。", { count: working.value });
  return "";
});

const date = computed(() => {
  const d = now.value;
  const weekdays = [
    t("周日"),
    t("周一"),
    t("周二"),
    t("周三"),
    t("周四"),
    t("周五"),
    t("周六"),
  ];
  return t("{month} 月 {day} 日 {weekday}", {
    month: d.getMonth() + 1,
    day: d.getDate(),
    weekday: weekdays[d.getDay()] ?? "",
  });
});
function finishedAt(at: string | null) {
  const date = parseAt(at);
  return date
    ? date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", hour12: false })
    : "";
}
const busy = computed(() => !!props.center.state.busy);
</script>
<template>
  <div class="today">
    <div class="today-page">
      <div class="topline">
        <JeevesMark :state="model.mark" />
        <span class="date tnum">{{ date }}</span>
      </div>

      <h1 class="greeting" tabindex="-1">
        {{ hello }}<template v-if="lead">{{ lead }}</template>
      </h1>
      <p v-if="sub" class="greeting-sub">{{ sub }}</p>

      <p v-if="center.state.late" class="today-note" role="status">
        {{
          center.state.late.answeredBy
            ? t("晚了一步：这条已由 {who} 回答。", { who: center.state.late.answeredBy })
            : t("这条请求已变更或结束，请核对当前待确认事项。")
        }}
      </p>
      <p v-if="center.state.error" class="today-note bad" role="status">
        {{ center.state.error }}
      </p>

      <p v-if="empty" class="all-set">{{ t("都安排好了。有新的事，直接在下面说。") }}</p>

      <section v-if="model.needs.length" class="today-block needs" aria-labelledby="today-needs">
        <h2 id="today-needs" class="block-head">
          {{ t("需要你的") }}<span class="count tnum">{{ model.needs.length }}</span>
        </h2>
        <template v-for="item in model.needs" :key="item.approval?.requestId ?? item.task?.id">
          <ApprovalCard
            v-if="item.approval"
            :key="`${item.approval.requestId}-${item.approval.version}`"
            :approval="item.approval"
            :task="item.task"
            :restored="center.state.restored[item.approval.turnId]"
            :busy="busy"
            @answer="(ok, reviewed) => center.approve(reviewed, ok)"
          />
          <TaskRow
            v-else-if="item.task"
            :task="item.task"
            :events="timelines[item.task.id]"
            tone="warn"
            :action="item.kind === 'failed' ? t('看原因') : t('去处理')"
            @open="emit('open', $event)"
          />
        </template>
      </section>

      <section v-if="model.moving.length" class="today-block" aria-labelledby="today-moving">
        <h2 id="today-moving" class="block-head">
          {{ t("在推进的") }}<span class="count tnum">{{ model.moving.length }}</span>
        </h2>
        <TaskRow
          v-for="task in model.moving"
          :key="task.id"
          :task="task"
          :events="timelines[task.id]"
          @open="emit('open', $event)"
        />
      </section>

      <section v-if="model.finished.length" class="today-block" aria-labelledby="today-done">
        <h2 id="today-done" class="block-head">{{ t("今天的") }}</h2>
        <ul class="today-list">
          <li v-for="task in model.finished" :key="task.id">
            <time class="tnum">{{ finishedAt(task.finishedAt) }}</time>
            <span class="title">{{ task.title }}</span>
            <button type="button" class="link" @click="emit('open', task.id)">
              {{ task.status === "cancelled" ? t("已取消") : t("看结果") }}
            </button>
          </li>
        </ul>
      </section>
    </div>
  </div>
</template>
