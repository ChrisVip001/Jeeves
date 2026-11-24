<script setup lang="ts">
import { t } from "./i18n";
import type { AgentTaskView } from "./ui/types";
import type { TaskCenter } from "./ui/useTasks";
const props = defineProps<{ task: AgentTaskView; center: TaskCenter }>();
</script>

<template>
  <section v-if="task.goalDraft && center.confirmGoal" class="goal-draft" aria-live="polite">
    <h3>{{ t("完成标准待你确认") }}</h3>
    <p class="goal-draft-text">{{ task.goalDraft }}</p>
    <p class="goal-draft-note">{{ t("按这个标准判定完成") }}</p>
    <div class="td-actions">
      <button type="button" class="today-btn primary" :disabled="!!center.state.busy"
        @click="center.confirmGoal?.(task.id, true)">{{ t("确认标准") }}</button>
      <button type="button" class="today-btn" :disabled="!!center.state.busy"
        @click="center.confirmGoal?.(task.id, false)">{{ t("不用标准") }}</button>
    </div>
  </section>
</template>
