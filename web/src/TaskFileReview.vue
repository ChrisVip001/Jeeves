<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import { Download, X } from "lucide-vue-next";
import type { TaskReviewFile } from "./protocol";
import { diffRows } from "./fileReview";
import { t } from "./i18n";
import { renderMarkdown } from "./markdown";

const props = defineProps<{file: TaskReviewFile; base?: string; result?: string | null}>();
const emit = defineEmits<{close:[]}>();
const tab = ref<"preview" | "diff">("preview");
const heading = ref<HTMLElement | null>(null);
const lines = computed(() => diffRows(props.file.patch ?? ""));
const markdown = computed(() => /\.md$/i.test(props.file.path));
const vDocument = {
  mounted(el: HTMLElement, binding: {value: string}) { el.replaceChildren(renderMarkdown(binding.value)); },
  updated(el: HTMLElement, binding: {value: string}) { el.replaceChildren(renderMarkdown(binding.value)); },
};
onMounted(async () => {
  await nextTick();
  heading.value?.focus();
});
function download() {
  if (props.file.content == null) return;
  const link = document.createElement("a");
  const url = URL.createObjectURL(new Blob([props.file.content], {type: "text/plain;charset=utf-8"}));
  link.href = url;
  link.download = props.file.path.split(/[\\/]/).at(-1) || "file.txt";
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
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
      </div>
      <button type="button" class="icon-action" :title="t('关闭文件审阅')" :aria-label="t('关闭文件审阅')" @click="emit('close')"><X :size="17" /></button>
    </header>
    <div class="file-review-toolbar">
      <div class="file-review-modes" role="group" :aria-label="t('文件视图')">
        <button type="button" :aria-pressed="tab === 'preview'" @click="tab = 'preview'">{{ t("预览") }}</button>
        <button type="button" :aria-pressed="tab === 'diff'" @click="tab = 'diff'">{{ t("逐行差异") }}</button>
      </div>
      <button v-if="file.content != null" type="button" class="icon-action" :title="t('保存文本副本')" :aria-label="t('保存文本副本')" @click="download"><Download :size="16" /></button>
    </div>
    <div v-if="tab === 'preview'" class="file-review-body">
      <p v-if="file.content == null" class="review-empty">{{ file.change === 'deleted' ? t("交付时此文件已删除。") : t("此文件没有可用的文本预览。") }}</p>
      <div v-else-if="markdown" v-document="file.content" class="bubble md review-document"></div>
      <pre v-else class="review-source">{{ file.content }}</pre>
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
