<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { GitBranch } from "lucide-vue-next";
import type { AgentTaskView, TaskGitCheckpointParams, TaskGitCheckpointResponse, TaskGitCheckpointPolicyParams, TaskGitCheckpointPolicyResponse } from "./types";
import { useUiText } from "./translate";
import GitCheckpointPolicyForm from "./GitCheckpointPolicyForm.vue";
import { checkpointPaths, invalidCheckpointPaths } from "./git-checkpoint-paths";

const t = useUiText();
const props = defineProps<{
  task: AgentTaskView;
  busy: boolean;
  submit?: (params: TaskGitCheckpointParams) => Promise<TaskGitCheckpointResponse>;
  readPolicy?: (taskId: string) => Promise<TaskGitCheckpointPolicyResponse>;
  writePolicy?: (params: TaskGitCheckpointPolicyParams) => Promise<TaskGitCheckpointPolicyResponse>;
}>();
const opened = ref(false);
const policyOpened = ref(false);
const editor = ref<HTMLTextAreaElement>();
const opener = ref<HTMLButtonElement>();
const paths = ref("");
const confirmed = ref(false);
const saving = ref(false);
const error = ref(false);
const result = ref<TaskGitCheckpointResponse | null>(null);
const linked = computed(() => Boolean(props.task.worktreePath && props.task.branch));
const idle = computed(() => linked.value
  && ["pending", "suspended", "needs_attention", "needsAttention", "done", "failed", "cancelled"].includes(props.task.status)
  && !props.task.currentTurnId && !["pending", "claimed", "queued"].includes(props.task.dispatchState ?? ""));
const eligible = computed(() => Boolean(props.submit) && idle.value);
const canReadPolicy = computed(() => linked.value && Boolean(props.readPolicy && props.writePolicy));
const files = computed(() => checkpointPaths(paths.value));
const invalid = computed(() => invalidCheckpointPaths(files.value));
watch(paths, () => { confirmed.value = false; error.value = false; });
watch(() => props.task.id, () => { opened.value = false; paths.value = ""; confirmed.value = false; error.value = false; result.value = null; });
async function open() {
  if (!eligible.value || props.busy) return;
  opened.value = true;
  result.value = null;
  await nextTick();
  editor.value?.focus();
}
async function close() {
  opened.value = false;
  paths.value = "";
  confirmed.value = false;
  error.value = false;
  await nextTick();
  opener.value?.focus();
}
async function save() {
  if (!idle.value || props.busy || saving.value || !props.submit || !files.value.length || invalid.value || !confirmed.value) return;
  saving.value = true;
  error.value = false;
  try {
    result.value = await props.submit({ taskId: props.task.id, paths: files.value, confirmed: true });
    await close();
  } catch {
    // Retain the explicitly selected files, but require confirmation again before any retry.
    confirmed.value = false;
    error.value = true;
  } finally { saving.value = false; }
}
</script>

<template>
  <section v-if="eligible || opened || result || canReadPolicy" class="td-checkpoint" :aria-label="t('Git 检查点')">
    <div class="checkpoint-head">
      <GitBranch :size="16" aria-hidden="true" />
      <div><h3>{{ t('Git 检查点') }}</h3><p class="meta checkpoint-branch">{{ task.branch }}</p></div>
      <button v-if="!opened && !policyOpened && eligible" ref="opener" type="button" class="today-btn" :disabled="busy" @click="open">{{ t('保存检查点') }}</button>
    </div>
    <p v-if="result && !opened" class="checkpoint-result" role="status">{{ t('已保存本地提交：') }}<code>{{ result.commit }}</code></p>
    <form v-if="opened" class="checkpoint-form" @submit.prevent="save">
      <p class="meta">{{ t('只提交明确列出的文件到此任务分支，不会推送或合并。') }}</p>
      <label>{{ t('文件路径（每行一个，基于任务工作区）') }}<textarea ref="editor" v-model="paths" name="paths" rows="3" required spellcheck="false" placeholder="src/main.rs" :disabled="saving" /></label>
      <p v-if="invalid" class="meta" role="status">{{ t('请输入工作区内的明确相对文件路径。') }}</p>
      <label class="checkpoint-confirm"><input v-model="confirmed" type="checkbox" name="confirmed" :disabled="saving" />{{ t('确认提交以上文件') }}</label>
      <p v-if="!eligible" class="meta" role="status">{{ t('任务正在运行或等待调度，暂不能保存检查点。') }}</p>
      <p v-if="error" class="checkpoint-error" role="alert">{{ t('检查点未保存，请核对工作区和文件后重试。') }}</p>
      <div class="checkpoint-footer"><button type="button" class="today-btn" :disabled="saving" @click="close">{{ t('取消') }}</button><button type="submit" class="today-btn primary" :disabled="busy || saving || !eligible || !confirmed || !files.length || invalid">{{ saving ? t('保存中…') : t('保存检查点') }}</button></div>
    </form>
    <GitCheckpointPolicyForm v-if="linked && readPolicy && writePolicy" v-model="policyOpened" :task-id="task.id" :eligible="idle" :busy="busy" :manual-opened="opened" :read="readPolicy" :write="writePolicy" />
  </section>
</template>
