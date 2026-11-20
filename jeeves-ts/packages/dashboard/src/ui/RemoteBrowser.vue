<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { X, RefreshCw, Maximize2, ArrowDown, ArrowUp } from "lucide-vue-next";
import type { BrowserSessionView, BrowserHumanInputParams } from "../protocol";
import { useUiText } from "./translate";
import type { BrowserCenter } from "./useBrowsers";
import BrowserDownloads from "./BrowserDownloads.vue";

const props = defineProps<{ center: BrowserCenter; row: BrowserSessionView }>();
const emit = defineEmits<{ close: [] }>();
const t = useUiText();
const dialog = ref<HTMLDialogElement>();
const loadedId = ref("");
const fullSize = ref(false);
const draft = ref("");
const frame = computed(() => props.center.state.frames[props.row.taskId]);
const busy = computed(() => !!props.center.state.busy[props.row.taskId]);
const interactive = computed(
  () =>
    props.row.status === "human" &&
    frame.value?.frameId &&
    frame.value.frameId === loadedId.value,
);
watch(
  [() => props.row.id, () => props.row.generation, () => props.row.status],
  () => {
    draft.value = "";
    loadedId.value = "";
  },
);
watch(() => frame.value?.frameId, () => { draft.value = ""; });
onMounted(() => {
  dialog.value?.showModal();
  void props.center.frame(props.row.taskId);
});

function send(input: BrowserHumanInputParams["input"]) {
  if (interactive.value && !busy.value)
    void props.center.input(props.row, loadedId.value, input);
}
function clickPage(event: MouseEvent) {
  const img = event.currentTarget as HTMLImageElement;
  const box = img.getBoundingClientRect();
  const x = Math.floor(
    ((event.clientX - box.left) * img.naturalWidth) / box.width,
  );
  const y = Math.floor(
    ((event.clientY - box.top) * img.naturalHeight) / box.height,
  );
  if (x >= 0 && y >= 0 && x < img.naturalWidth && y < img.naturalHeight)
    send({ kind: "click", x, y });
}
function sendText() {
  if (!draft.value || !interactive.value || busy.value) return;
  const text = draft.value;
  draft.value = "";
  send({ kind: "text", text });
}
</script>
<template>
  <dialog
    ref="dialog"
    class="remote-browser"
    :aria-label="t('远程浏览器')"
    @close="emit('close')"
  >
    <header>
      <div>
        <h2>{{ t("远程浏览器") }}</h2>
        <p>{{ row.title || row.url }}</p>
      </div>
      <slot name="executor" />
      <button
        type="button"
        class="remote-icon"
        :aria-label="t('关闭远程面板')"
        @click="dialog?.close()"
      >
        <X :size="20" />
      </button>
    </header>
    <div class="remote-content">
      <div class="remote-viewbar">
        <strong>{{
          row.status === "human" ? t("由你控制") : t("浏览器画面")
        }}</strong>
        <div>
          <button
            type="button"
            class="remote-icon"
            :aria-label="t('按原始尺寸查看')"
            :aria-pressed="fullSize"
            @click="fullSize = !fullSize"
          >
            <Maximize2 :size="17" />
          </button>
          <button
            type="button"
            class="remote-icon"
            :disabled="busy"
            :aria-label="t('刷新画面')"
            @click="center.frame(row.taskId)"
          >
            <RefreshCw :size="17" />
          </button>
        </div>
      </div>
      <div
        v-if="frame"
        class="remote-frame"
        :class="{ 'full-size': fullSize, 'can-click': interactive && !busy }"
        :tabindex="fullSize ? 0 : undefined"
        :aria-label="t('浏览器当前画面')"
      >
        <img
          :key="frame.frameId || frame.observedAt"
          :src="frame.dataUrl"
          :alt="t('浏览器当前画面')"
          :data-frame="frame.frameId || ''"
          @load="
            loadedId =
              ($event.currentTarget as HTMLImageElement).dataset.frame || ''
          "
          @click="clickPage"
        />
      </div>
      <p v-if="!frame && busy" role="status">{{ t("正在读取画面…") }}</p>
      <p
        v-if="center.state.inputFailed[row.taskId]"
        class="remote-error"
        role="alert"
      >
        {{ t("输入结果待核对。请刷新画面后再操作。") }}
      </p>
      <p
        v-if="center.state.errors[row.taskId]"
        class="remote-error"
        role="alert"
      >
        {{ center.state.errors[row.taskId] }}
      </p>
      <template v-if="row.status === 'human' && frame?.frameId">
        <p class="remote-hint">
          {{ t("点画面选择位置，或用 Tab 切换焦点、Enter 激活。") }}
        </p>
        <form @submit.prevent="sendText">
          <label
            >{{ t("输入到网页")
            }}<input
              v-model="draft"
              type="password"
              autocomplete="off"
              maxlength="4000"
              autocapitalize="off"
              :spellcheck="false"
              :disabled="busy || !interactive"
          /></label>
          <button type="submit" class="quiet-button" :disabled="busy || !interactive || !draft">
            {{ t("输入文字") }}
          </button>
        </form>
        <div class="remote-keys" :aria-label="t('网页按键')" role="group">
          <button
            v-for="key in ['tab', 'enter', 'backspace', 'escape'] as const"
            :key="key"
            type="button"
            :disabled="busy || !interactive"
            @click="send({ kind: 'key', key })"
          >
            {{
              {
                tab: "Tab",
                enter: "Enter",
                backspace: "Backspace",
                escape: "Esc",
              }[key]
            }}
          </button>
          <button
            type="button"
            :disabled="busy || !interactive"
            :aria-label="t('向上滚动网页')"
            @click="send({ kind: 'scroll', direction: 'up' })"
          >
            <ArrowUp :size="16" />
          </button>
          <button
            type="button"
            :disabled="busy || !interactive"
            :aria-label="t('向下滚动网页')"
            @click="send({ kind: 'scroll', direction: 'down' })"
          >
            <ArrowDown :size="16" />
          </button>
        </div>
      </template>
      <p v-else-if="frame && !busy && !frame.frameId" class="remote-hint">
        {{ t("当前画面只读。接管后刷新画面即可操作。") }}
      </p>
      <BrowserDownloads :center="center" :row="row" />
      <footer>
        <button
          type="button"
          class="quiet-button"
          :disabled="busy"
          @click="center.control(row, 'return')"
        >
          {{ t("交还给 Jeeves") }}
        </button>
      </footer>
    </div>
  </dialog>
</template>
<style scoped>
.remote-browser {
  width: min(980px, 94vw);
  max-width: 100vw;
  max-height: 92dvh;
  padding: 0;
  border: 1px solid var(--line);
  border-radius: 12px;
  background: var(--bg);
  color: var(--text);
  font: 14px/1.55 var(--font);
  overflow: auto;
}
.remote-browser::backdrop {
  background: var(--scrim);
}
.remote-browser header {
  display: flex;
  gap: 10px;
  align-items: center;
  padding: 8px 18px;
  border-bottom: 1px solid var(--line);
  background: var(--bg);
  position: sticky;
  top: 0;
  z-index: 1;
  flex-wrap: wrap;
}
.remote-browser header > div:first-child {
  flex: 1;
  min-width: 6em;
}
.remote-browser h2 {
  margin: 0;
  font-size: 18px;
  font-weight: 600;
}
.remote-browser header p {
  margin: 2px 0;
  font-size: 13px;
  color: var(--text-2);
  overflow-wrap: anywhere;
}
.remote-content {
  padding: 10px 18px 16px;
}
.remote-content strong {
  font-weight: 500;
}
.remote-viewbar,
.remote-viewbar > div,
.remote-keys {
  display: flex;
  align-items: center;
  gap: 4px;
}
.remote-viewbar {
  justify-content: space-between;
}
.remote-browser button {
  cursor: pointer;
  min-height: 44px;
  font: inherit;
  color: inherit;
}
.remote-browser button:disabled {
  cursor: default;
  opacity: 0.5;
}
.remote-icon,
.remote-keys button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: transparent;
  border: 1px solid transparent;
  border-radius: 6px;
  min-width: 44px;
}
.remote-icon:hover,
.remote-icon[aria-pressed="true"],
.remote-keys button:hover {
  background: var(--surface-hover);
}
.remote-frame {
  border: 1px solid var(--line);
  border-radius: 7px;
  max-height: 55dvh;
  overflow: auto;
  background: var(--surface);
}
.remote-frame img {
  display: block;
  width: 100%;
  height: auto;
  max-width: none;
}
.remote-frame.full-size img {
  width: auto;
}
.remote-frame.can-click img {
  cursor: crosshair;
}
.remote-hint,
.remote-error {
  font-size: 13px;
  margin: 8px 0;
  color: var(--text-2);
}
.remote-error {
  color: var(--bad);
}
.remote-browser form {
  display: flex;
  align-items: flex-end;
  gap: 8px;
}
.remote-browser label {
  display: grid;
  flex: 1;
  min-width: 0;
  font-size: 13px;
  gap: 4px;
}
.remote-browser input {
  box-sizing: border-box;
  min-height: 44px;
  width: 100%;
  padding: 6px 10px;
  border: 1px solid var(--line);
  border-radius: 6px;
  background: var(--surface);
  color: var(--text);
  font: inherit;
}
.remote-browser footer {
  display: flex;
  justify-content: flex-end;
  padding-top: 10px;
  border-top: 1px solid var(--line);
}
.remote-browser form button {
  min-width: fit-content;
}
.remote-keys {
  flex-wrap: wrap;
  margin-top: 4px;
}
.remote-keys button {
  font-family: var(--mono);
  font-size: 13px;
}
@media (max-width: 640px) {
  .remote-browser {
    width: 100%;
    max-width: 100%;
    max-height: 100dvh;
    height: 100dvh;
    border: 0;
    border-radius: 0;
  }
  .remote-content {
    padding: 8px 14px;
  }
  .remote-browser header {
    padding: 8px 14px;
  }
}
</style>
