<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import type { TaskGitCheckpointPolicyParams, TaskGitCheckpointPolicyResponse } from "./types";
import { checkpointPaths, invalidCheckpointPaths } from "./git-checkpoint-paths";
import { useUiText } from "./translate";

const t = useUiText();
const props = defineProps<{
  taskId: string;
  eligible: boolean;
  busy: boolean;
  manualOpened: boolean;
  read: (taskId: string) => Promise<TaskGitCheckpointPolicyResponse>;
  write: (params: TaskGitCheckpointPolicyParams) => Promise<TaskGitCheckpointPolicyResponse>;
}>();
const opened = defineModel<boolean>({ required: true });
const policy = ref<TaskGitCheckpointPolicyResponse | null>(null);
const loading = ref(false);
const loadError = ref(false);
const saveError = ref(false);
const saving = ref(false);
const desired = ref(false);
const paths = ref("");
const confirmed = ref(false);
const files = computed(() => checkpointPaths(paths.value));
const invalid = computed(() => invalidCheckpointPaths(files.value));
const controls = ref<HTMLElement>();
const editor = ref<HTMLTextAreaElement>();
const confirmation = ref<HTMLInputElement>();
watch(paths, () => { confirmed.value = false; saveError.value = false; });
async function load() {
  const taskId = props.taskId;
  policy.value = null;
  loadError.value = false;
  loading.value = true;
  try {
    const value = await props.read(taskId);
    if (taskId === props.taskId) policy.value = value;
  } catch { if (taskId === props.taskId) loadError.value = true; }
  finally { if (taskId === props.taskId) loading.value = false; }
}
watch(() => props.taskId, () => { opened.value = false; void load(); }, { immediate: true });
async function edit(enabled: boolean) {
  if (!props.eligible || props.busy || props.manualOpened || !policy.value) return;
  desired.value = enabled;
  paths.value = enabled ? policy.value.paths.join("\n") : "";
  confirmed.value = false;
  saveError.value = false;
  opened.value = true;
  await nextTick();
  if (enabled) editor.value?.focus(); else confirmation.value?.focus();
}
async function close() {
  opened.value = false;
  confirmed.value = false;
  saveError.value = false;
  paths.value = "";
  await nextTick();
  controls.value?.querySelector<HTMLButtonElement>("button")?.focus();
}
async function save() {
  if (!props.eligible || props.busy || saving.value || !policy.value || !confirmed.value || (desired.value && (!files.value.length || invalid.value))) return;
  saving.value = true;
  saveError.value = false;
  const taskId = props.taskId;
  try {
    const value = await props.write({ taskId, paths: desired.value ? files.value : [], enabled: desired.value, confirmed: true });
    if (taskId === props.taskId) { policy.value = value; await close(); }
  } catch {
    if (taskId === props.taskId) { confirmed.value = false; saveError.value = true; }
  } finally { saving.value = false; }
}
</script>

<template>
  <div class="checkpoint-policy">
    <h4 v-if="!policy">{{ t('自动检查点') }}</h4>
    <p v-if="loading" class="meta" role="status">{{ t('正在读取自动检查点设置…') }}</p>
    <template v-else-if="loadError">
      <p role="alert" class="checkpoint-error">{{ t('无法读取自动检查点设置。') }}</p>
      <button type="button" class="today-btn" @click="load">{{ t('重新检查') }}</button>
    </template>
    <template v-else-if="policy">
      <h4 class="checkpoint-policy-status">{{ policy.enabled ? t('自动检查点：已开启') : t('自动检查点：已关闭') }}</h4>
      <p class="meta">{{ t('默认关闭。启用后，任务在持久化执行边界仅提交你批准的文件，不会推送或合并。') }}</p>
      <ul v-if="policy.enabled && policy.paths.length" class="checkpoint-policy-paths" :aria-label="t('已批准的文件')"><li v-for="path in policy.paths" :key="path"><code>{{ path }}</code></li></ul>
      <div v-if="eligible && !manualOpened && !opened" ref="controls" class="checkpoint-footer checkpoint-policy-actions">
        <button v-if="policy.enabled" type="button" class="today-btn" data-policy="disable" :disabled="busy" @click="edit(false)">{{ t('关闭自动检查点') }}</button>
        <button type="button" class="today-btn" data-policy="enable" :disabled="busy" @click="edit(true)">{{ policy.enabled ? t('更改批准文件') : t('开启自动检查点') }}</button>
      </div>
      <form v-if="opened" class="checkpoint-form checkpoint-policy-form" @submit.prevent="save">
        <template v-if="desired">
          <label>{{ t('文件路径（每行一个，基于任务工作区）') }}<textarea ref="editor" v-model="paths" name="paths" rows="3" required spellcheck="false" placeholder="src/main.rs" :disabled="saving" /></label>
          <p v-if="invalid" role="status" class="meta">{{ t('请输入工作区内的明确相对文件路径。') }}</p>
          <p class="meta">{{ t('文件内容会进入本地 Git 历史。不要选择包含密钥、令牌或隐私数据的文件。') }}</p>
        </template>
        <p v-else class="meta">{{ t('关闭后不再自动提交，已有本地提交不会被删除。') }}</p>
        <label class="checkpoint-confirm"><input ref="confirmation" v-model="confirmed" type="checkbox" name="confirmed" :disabled="saving" />{{ desired ? t('确认以上文件可在后续执行边界自动提交') : t('确认关闭自动检查点') }}</label>
        <p v-if="!eligible" class="meta" role="status">{{ t('任务正在运行或等待调度，暂不能保存检查点。') }}</p>
        <p v-if="saveError" role="alert" class="checkpoint-error">{{ t('自动检查点设置未保存，请核对配置后重试。') }}</p>
        <div class="checkpoint-footer"><button type="button" class="today-btn" :disabled="saving" @click="close">{{ t('取消') }}</button><button type="submit" class="today-btn primary" :disabled="busy || saving || !eligible || !confirmed || (desired && (!files.length || invalid))">{{ saving ? t('保存中…') : t('保存设置') }}</button></div>
      </form>
    </template>
  </div>
</template>
