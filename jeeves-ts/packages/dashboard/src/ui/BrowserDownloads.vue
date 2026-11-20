<script setup lang="ts">
import { computed } from "vue";
import { Download, RefreshCw } from "lucide-vue-next";
import { useUiText } from "./translate";
import type { BrowserCenter } from "./useBrowsers";
import type { BrowserSessionView } from "../protocol";

const props = defineProps<{ center: BrowserCenter; row: BrowserSessionView }>();
const t = useUiText();
const files = computed(() => props.center.state.downloads[props.row.taskId]);
const busy = computed(() => !!props.center.state.busy[props.row.taskId]);
function status(value: string) {
  const labels: Record<string, string> = {
    receiving: t("正在接收"),
    pending: t("待保存"),
    saved: t("已归档"),
    cancelled: t("已取消"),
    failed: t("下载失败"),
  };
  return labels[value] ?? value;
}
function downloadDetail(value: string) {
  switch (value) {
    case "Download requires human control and an allowed website":
      return t("下载需要人工接管，且来源网站必须已获准。");
    case "Download exceeds 768 KiB":
      return t("下载文件超过 768 KiB 上限。");
    case "Browser download interrupted":
      return t("浏览器下载已中断。");
    case "Download limit reached; open a fresh task browser":
      return t("下载数量已达上限，请打开新的任务浏览器。");
    case "Download cancellation could not be confirmed":
      return t("无法确认下载是否已取消。");
    case "Accepted file awaits artifact delivery; retry save":
      return t("已接收文件，等待归档到任务。请重新保存。");
    default:
      return value;
  }
}
function onToggle(event: Event) {
  if ((event.currentTarget as HTMLDetailsElement).open)
    void props.center.downloads(props.row.taskId);
}
</script>

<template>
  <details class="browser-downloads" @toggle="onToggle">
    <summary>
      <Download :size="16" aria-hidden="true" />{{ t("任务下载")
      }}<span v-if="files?.length">{{ files.length }}</span>
    </summary>
    <div class="downloads-head">
      <p>{{ t("保存后可在此任务的“结果与文件”中查看。") }}</p>
      <button
        type="button"
        class="quiet-button"
        :disabled="busy"
        @click="center.downloads(row.taskId)"
      >
        <RefreshCw :size="14" aria-hidden="true" />{{ t("刷新下载") }}
      </button>
    </div>
    <p v-if="files && !files.length" class="task-empty">
      {{ t("这个浏览器还没有下载。") }}
    </p>
    <ul v-else-if="files?.length">
      <li v-for="file in files" :key="file.id">
        <div>
          <strong>{{ file.name }}</strong
          ><small>{{ status(file.status) }}</small>
          <p v-if="file.detail">{{ downloadDetail(file.detail) }}</p>
        </div>
        <div
          v-if="
            row.status === 'human' &&
            ['receiving', 'pending'].includes(file.status)
          "
          class="download-actions"
        >
          <button
            v-if="file.status === 'pending'"
            type="button"
            class="quiet-button"
            :disabled="busy"
            @click="center.download(row, file.id, 'save')"
          >
            {{ t("保存到任务") }}
          </button>
          <button
            type="button"
            class="text-button"
            :disabled="busy"
            @click="center.download(row, file.id, 'cancel')"
          >
            {{ t("取消下载") }}
          </button>
        </div>
      </li>
    </ul>
  </details>
</template>

<style scoped>
.browser-downloads {
  border-top: 1px solid var(--line);
  margin-top: 12px;
  padding-top: 4px;
}
.browser-downloads summary,
.downloads-head button {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.browser-downloads summary,
.browser-downloads button {
  min-height: 44px;
  cursor: pointer;
}
.browser-downloads summary {
  font-weight: 500;
}
.browser-downloads summary span,
.browser-downloads small,
.downloads-head p {
  color: var(--text-2);
}
.downloads-head {
  display: flex;
  align-items: center;
  gap: 8px;
  justify-content: space-between;
}
.downloads-head p {
  margin: 0;
}
.downloads-head button {
  flex: none;
}
.browser-downloads ul {
  list-style: none;
  padding: 0;
  margin: 8px 0;
}
.browser-downloads li {
  border-top: 1px solid var(--line-soft);
  padding: 8px 0;
}
.browser-downloads small {
  display: block;
  font-size: 12px;
}
.browser-downloads strong,
.browser-downloads li p {
  overflow-wrap: anywhere;
}
.browser-downloads strong {
  font-weight: 500;
}
.browser-downloads li p {
  margin: 4px 0;
}
.download-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
</style>
