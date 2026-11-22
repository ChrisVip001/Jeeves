<script setup lang="ts">
import {
  X,
  Folder,
  CheckCircle2,
  Circle,
  Pause,
  Play,
  Square,
  ArrowUpRight,
} from "lucide-vue-next";
import { computed } from "vue";
import BrowserPanel from "./BrowserPanel.vue";
import { useUiText } from "./translate";
import type { TaskCenter } from "./useTasks";
const t = useUiText();
const props = defineProps<{ center: TaskCenter; readOnly?: boolean }>();
const groups = computed(() => {
  const buckets = [[], [], []] as (typeof props.center.state.tasks)[];
  for (const task of props.center.state.tasks) {
    const ended = ["done", "failed", "cancelled"].includes(task.status);
    const waiting = props.center.approvalFor(task) || ["needs_attention","needsAttention","suspended"].includes(task.status);
    buckets[ended ? 2 : waiting ? 1 : 0]!.push(task);
  }
  return [t("进行中"), t("等你决定"), t("已完成")].map((title, index) => ({ title, tasks: buckets[index]! }));
});
const emit = defineEmits<{ follow: [id: string]; inspect: [id: string] }>();
function label(status: string, verified: boolean) {
  const labels: Record<string, string> = {
    pending: t("等待执行"),
    running: t("正在执行"),
    suspended: t("已暂停"),
    needs_attention: t("需要你处理"),
    needsAttention: t("需要你处理"),
    done: verified ? t("已完成") : t("已生成，待验收"),
    failed: t("执行失败"),
    cancelled: t("已取消"),
  };
  return labels[status] ?? status;
}
</script>
<template>
  <aside
    v-if="center.state.open"
    class="jeeves-task-center"
    :aria-label="t('任务进展')"
    @keydown.esc="center.state.open = false"
  >
    <header>
      <div>
        <strong>{{ t("任务进展") }}</strong><small>{{
          t("在主会话交流，任务分别推进")
        }}</small>
      </div>
      <button
        type="button"
        class="task-icon"
        :aria-label="t('关闭任务面板')"
        @click="center.state.open = false"
      >
        <X :size="17" />
      </button>
    </header>
    <div v-if="!readOnly && $slots.executor" class="executor-inline"><slot name="executor" /></div>
    <p v-if="center.state.error" class="task-error" role="status">
      {{ center.state.error }}
    </p>
    <div class="jeeves-task-list">
      <p v-if="!center.state.tasks.length" class="task-empty">
        {{ t("把要做的事告诉 Jeeves，任务会出现在这里。") }}
      </p>
      <template v-if="readOnly">
        <section v-for="group in groups" :key="group.title" class="phone-task-group">
          <h3>{{ group.title }} <small>{{ group.tasks.length }}</small></h3>
          <button type="button" v-for="task in group.tasks" :key="task.id" @click="emit('inspect', task.id)">{{ task.title }}<span>{{ label(task.status, !!task.acceptance.length) }}</span></button>
        </section>
      </template>
      <template v-else>
      <article
        v-for="task in center.state.tasks"
        :key="task.id"
        class="jeeves-task-card"
      >
        <div class="task-status">
          <CheckCircle2 v-if="task.status === 'done'" :size="14" /><Circle
            v-else
            :size="14"
          /><span>{{ label(task.status, !!task.acceptance.length) }}</span>
        </div>
        <h3>{{ task.title }}</h3>
        <p v-if="task.lastStopReason || task.suspendReason" class="task-reason">
          {{ task.lastStopReason || task.suspendReason }}
        </p>
        <div v-if="task.cwd" class="task-path" :title="task.cwd">
          <Folder :size="13" /><span>{{ task.cwd }}</span>
        </div>
        <template
          v-for="approval in [center.approvalFor(task)]"
          :key="approval?.requestId"
        >
          <div v-if="approval" class="task-approval">
            <strong>{{ t("需要你的确认") }}</strong>
            <p>{{ approval.tool }} · {{ approval.reason }}</p>
            <pre>{{ approval.argsPreview }}</pre>
            <div>
              <button
                type="button"
                class="quiet-button"
                :disabled="!!center.state.busy"
                @click="center.approve(approval, false)"
              >
                {{ t("拒绝") }}</button><button
                type="button"
                class="primary-button"
                :disabled="!!center.state.busy"
                @click="center.approve(approval, true)"
              >
                {{ t("允许一次") }}
              </button>
            </div>
          </div>
        </template>
        <button type="button" class="text-button" @click="emit('inspect', task.id)">{{ t("查看任务对话") }}</button>
        <details
          @toggle="
            ($event.target as HTMLDetailsElement).open && center.inspect(task)
          "
        >
          <summary>{{ t("查看结果与文件") }}</summary>
          <p v-if="center.state.results[task.id]" class="task-result">
            {{ center.state.results[task.id] }}
          </p>
          <p v-else class="task-empty">{{ t("结果会在执行后显示") }}</p>
          <ul v-if="center.state.artifacts[task.id]?.length">
            <li
              v-for="file in center.state.artifacts[task.id]"
              :key="file.path"
            >
              <code :title="file.path">{{
                file.path.split(/[\\/]/).pop()
              }}</code>
            </li>
          </ul>
        </details>
        <BrowserPanel
          v-if="center.browser"
          :center="center.browser"
          :task-id="task.id"
          :cancelled="task.status === 'cancelled'"
        ><template #executor><slot name="executor" /></template></BrowserPanel>
        <slot name="desktop" :task="task" />
        <footer>
          <button
            v-if="['pending', 'running', 'done'].includes(task.status)"
            type="button"
            class="text-button"
            @click="emit('follow', task.id)"
          >
            <ArrowUpRight :size="14" />{{ t("补充要求") }}
          </button>
          <button
            v-if="['pending', 'running'].includes(task.status)"
            type="button"
            class="task-icon"
            :disabled="!!center.state.busy"
            :title="t('暂停任务')"
            @click="center.action('suspend', task.id)"
          >
            <Pause :size="14" />
          </button>
          <button
            v-if="
              ['suspended', 'needs_attention', 'needsAttention'].includes(
                task.status,
              )
            "
            type="button"
            class="task-icon"
            :disabled="!!center.state.busy"
            :title="t('继续任务')"
            @click="center.action('resume', task.id)"
          >
            <Play :size="14" />
          </button>
          <button
            v-if="!['done', 'failed', 'cancelled'].includes(task.status)"
            type="button"
            class="task-icon"
            :disabled="!!center.state.busy"
            :title="t('取消任务')"
            @click="center.action('cancel', task.id)"
          >
            <Square :size="14" />
          </button>
        </footer>
      </article>
      </template>
    </div>
  </aside>
</template>
