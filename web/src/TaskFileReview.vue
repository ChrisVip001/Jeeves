<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import { Download, X } from "lucide-vue-next";
import type { TaskFileResponse } from "./protocol";
import { diffRows, type WorkFile } from "./fileReview";
import { errorText, preferenceRequest } from "./api";
import { decodeBase64, saveDownload } from "./fileDownload";
import { t } from "./i18n";
import { renderMarkdown } from "./markdown";

const props = defineProps<{file: WorkFile; taskId: string; base?: string; result?: string | null}>();
const emit = defineEmits<{close:[]}>();
const tab = ref<"preview" | "diff">("preview");
const heading = ref<HTMLElement | null>(null);
const lines = computed(() => diffRows(props.file.patch ?? ""));
const markdown = computed(() => /\.md$/i.test(props.file.path));
const downloadLabel = computed(() => props.file.workspace ? t("下载原文件") : t("保存文本副本"));
const preview = ref(props.file.workspace && markdown.value ? null : props.file.content);
const pending = ref(false);
const failure = ref("");
const vDocument = {
  mounted(el: HTMLElement, binding: {value: string}) { el.replaceChildren(renderMarkdown(binding.value)); },
  updated(el: HTMLElement, binding: {value: string}) { el.replaceChildren(renderMarkdown(binding.value)); },
};
onMounted(async () => {
  await nextTick();
  heading.value?.focus();
  if (props.file.workspace && markdown.value) {
    await readWorkspace(bytes => {
      preview.value = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    });
  }
});
async function readWorkspace(use: (bytes: Uint8Array<ArrayBuffer>, filename: string) => void) {
  pending.value = true;
  failure.value = "";
  try {
    const file = await preferenceRequest<TaskFileResponse>("/api/task", {
      action: "file", taskId: props.taskId, path: props.file.path,
    });
    use(decodeBase64(file.data), file.filename);
  } catch (error) {
    failure.value = errorText(error);
  } finally {
    pending.value = false;
  }
}
function download() {
  if (props.file.workspace) {
    return readWorkspace((bytes, name) => saveDownload(new Blob([bytes], { type: "application/octet-stream" }), name));
  }
  if (props.file.content != null) {
    saveDownload(new Blob([props.file.content], { type: "text/plain;charset=utf-8" }),
      props.file.path.split(/[\\/]/).at(-1) || "file.txt");
  }
}
</script>

<template>
  <aside class="file-review" aria-labelledby="file-review-title" @keydown.esc.stop="emit('close')">
    <header class="file-review-head">
      <div>
        <h3 id="file-review-title" ref="heading" tabindex="-1">{{ file.path }}</h3>
        <p v-if="base" class="review-revisions">
          <code :title="base">{{ base.slice(0, 7) }}</code><span aria-hidden="true"> → </span><code v-if="result" :title="result">{{ result.slice(0, 7) }}</code><span v-else>{{ t("交付快照") }}</span>
        </p>
        <p v-else-if="file.workspace" class="review-revisions">{{ t("当前工作区文件") }}</p>
      </div>
      <button type="button" class="icon-action" :title="t('关闭文件审阅')" :aria-label="t('关闭文件审阅')" @click="emit('close')"><X :size="17" /></button>
    </header>
    <div class="file-review-toolbar">
      <div class="file-review-modes" role="group" :aria-label="t('文件视图')">
        <button type="button" :aria-pressed="tab === 'preview'" @click="tab = 'preview'">{{ t("预览") }}</button>
        <button type="button" :aria-pressed="tab === 'diff'" @click="tab = 'diff'">{{ t("逐行差异") }}</button>
      </div>
      <button v-if="file.workspace || file.content != null" type="button" class="icon-action"
        :disabled="pending" :title="downloadLabel"
        :aria-label="downloadLabel" @click="download"><Download :size="16" /></button>
    </div>
    <p v-if="failure" class="review-empty" role="alert">{{ failure }}</p>
    <div v-if="tab === 'preview'" class="file-review-body">
      <p v-if="pending && preview == null" class="review-empty" role="status">{{ t("正在读取文件…") }}</p>
      <p v-else-if="preview == null" class="review-empty">{{ file.change === 'deleted' ? t("交付时此文件已删除。") : t("此文件没有可用的文本预览。") }}</p>
      <div v-else-if="markdown" v-document="preview" class="bubble md review-document"></div>
      <pre v-else class="review-source">{{ preview }}</pre>
    </div>
    <div v-else class="file-review-body">
      <p v-if="file.patch == null" class="review-empty">{{ t("此文件没有可比较的文本快照。") }}</p>
      <p v-else-if="!lines.length" class="review-empty">{{ t("没有文本行变化。") }}</p>
      <table v-else class="review-diff">
        <thead><tr><th>{{ t("原行") }}</th><th>{{ t("新行") }}</th><th>{{ t("内容") }}</th></tr></thead>
        <tbody>
          <tr v-for="(line, index) in lines" :key="index" :class="line.kind">
            <td>{{ line.before }}</td><td>{{ line.after }}</td><td>{{ line.text }}</td>
          </tr>
        </tbody>
      </table>
    </div>
  </aside>
</template>
