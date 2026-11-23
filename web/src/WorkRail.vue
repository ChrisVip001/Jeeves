<script setup lang="ts">
import { computed } from "vue";
import { t } from "./i18n";
import type { TaskCenter } from "./ui/useTasks";
import { workSections, workStatus } from "./work-status";
const props = defineProps<{ center: TaskCenter; selected: string }>();
defineEmits<{ inspect: [id: string]; all: [] }>();
const groups = computed(() => {
  const { waiting, working, past } = workSections(props.center);
  return [
    { label: t("等你决定"), rows: waiting },
    { label: t("进行中"), rows: working },
    { label: t("已结束"), rows: past.slice(0, 8) },
  ].filter(group => group.rows.length);
});
</script>
<template>
  <nav class="work-rail" :aria-label="t('工作列表')">
    <button type="button" class="work-rail-all" @click="$emit('all')">{{ t("全部工作") }}</button>
    <section v-for="group in groups" :key="group.label">
      <h3>{{ group.label }} <span>{{ group.rows.length }}</span></h3>
      <button v-for="task in group.rows" :key="task.id" type="button" class="work-rail-row"
        :aria-current="task.id === selected ? 'page' : undefined" @click="$emit('inspect', task.id)">
        <span>{{ task.title }}</span><small>{{ workStatus(center, task) }}</small>
      </button>
    </section>
  </nav>
</template>
