<script setup lang="ts">
import { useUiText } from "./translate";
const t = useUiText();
import { onMounted, reactive, ref } from "vue";
import JeevesLogo from "./JeevesLogo.vue";
import type { ModelInfo, SettingsActions } from "./types";
const props = defineProps<{
  models: ModelInfo[];
  actions: SettingsActions;
  remember: (subject: string, content: string) => Promise<unknown>;
}>();
const emit = defineEmits<{
  done: [];
  skip: [];
}>();
const dialog = ref<HTMLDialogElement>();
const step = ref(1);
const saving = ref(false);
const error = ref("");
const form = reactive({
  provider: props.models[0]?.provider || "",
  model: "",
  apiKey: "",
  baseUrl: "",
});
const about = reactive({ name: "", cares: "" });
async function saveModel() {
  saving.value = true;
  error.value = "";
  try {
    await props.actions.saveModel({
      id: form.provider,
      provider: form.provider,
      model: form.model.trim() || null,
      smallModel: null,
      baseUrl: form.baseUrl.trim() || null,
      keyEnv: null,
      apiKey: form.apiKey || null,
      vision: null,
      makeActive: true,
    });
    step.value = 2;
  } catch (e) {
    error.value = String(e);
  } finally {
    saving.value = false;
    form.apiKey = "";
  }
}
async function finish() {
  saving.value = true;
  error.value = "";
  try {
    if (about.name.trim())
      await props.remember(t("用户的称呼"), about.name.trim());
    if (about.cares.trim())
      await props.remember(t("用户关心的事"), about.cares.trim());
    emit("done");
  } catch (e) {
    error.value = String(e);
  } finally {
    saving.value = false;
  }
}
onMounted(() => dialog.value?.showModal());
</script>
<template>
  <dialog ref="dialog" class="preferences setup-wizard" @cancel.prevent>
    <div class="preferences-content">
      <JeevesLogo :size="28" />
      <h2>{{ t("欢迎使用 Jeeves") }}</h2>
      <form v-if="step === 1" @submit.prevent="saveModel">
        <h3>{{ t("连接一个模型") }}</h3>
        <label
          >{{ t("服务商")
          }}<select v-model="form.provider" required>
            <option v-for="m in models" :key="m.provider" :value="m.provider">
              {{ m.label || m.provider }}
            </option>
          </select></label
        ><label
          >{{ t("模型名称")
          }}<input
            v-model="form.model"
            :placeholder="t('留空使用服务商默认模型')" /></label
        ><label
          >API Key<input
            v-model="form.apiKey"
            type="password"
            autocomplete="new-password"
          /><small>{{
            t("密钥保存在系统钥匙串；本地模型（如 Ollama）可留空。")
          }}</small></label
        >
        <p v-if="error" class="notice error">{{ error }}</p>
        <div class="form-footer">
          <button type="button" class="quiet-button" @click="emit('skip')">
            {{ t("稍后再说") }}</button
          ><button class="primary-button" :disabled="saving">
            {{ saving ? t("保存中…") : t("下一步") }}
          </button>
        </div>
      </form>
      <form v-else @submit.prevent="finish">
        <h3>{{ t("让 Jeeves 认识你") }}</h3>
        <label
          >{{ t("怎么称呼你")
          }}<input v-model="about.name" maxlength="80" /></label
        ><label
          >{{ t("你最近关心什么")
          }}<input
            v-model="about.cares"
            maxlength="200"
            :placeholder="t('例如：每天的工作简报、整理下载文件夹')"
        /></label>
        <p v-if="error" class="notice error">{{ error }}</p>
        <div class="form-footer">
          <button type="button" class="quiet-button" @click="emit('done')">
            {{ t("跳过") }}</button
          ><button class="primary-button" :disabled="saving">
            {{ saving ? t("保存中…") : t("开始使用") }}
          </button>
        </div>
      </form>
    </div>
  </dialog>
</template>
