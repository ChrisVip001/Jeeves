<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { AppWindow } from "lucide-vue-next";
import { errorText, preferenceRequest } from "../api.ts";
import type { DesktopAction, ModelProfilesResponse } from "../protocol.ts";
import { useDesktop } from "./useDesktop.ts";
import { useUiText } from "./translate.ts";

const props = defineProps<{
  taskId: string;
  blocked: boolean;
  generation?: number;
}>();
const t = useUiText();
const panel = useDesktop(props.taskId, preferenceRequest);
const state = panel.state;
const open = ref(false);
const elementId = ref<number | null>(null);
const input = ref("");
const previewReady = ref(false);
const previewFailed = ref(false);
watch(
  () => state.frame,
  () => {
    previewReady.value = false;
    previewFailed.value = false;
  },
  { flush: "sync" },
);
const profiles = ref<ModelProfilesResponse["profiles"]>([]);
function originLabel(origin: string): string {
  if (origin === "platform") return t("平台提供");
  if (origin === "local") return t("本地模型");
  return t("自带模型");
}
const profileError = ref("");
const diagnostics = computed<Record<string, string>>(() => ({
  "Allow Accessibility for the Jeeves window helper in System Settings, then select again":
    t("请在 Mac 系统设置中允许 Jeeves 窗口助手使用辅助功能，再选择窗口。"),
  "Allow Screen Recording for the Jeeves window helper in System Settings, then preview again":
    t("请在 Mac 系统设置中允许 Jeeves 窗口助手录屏，再预览截图。"),
  "Window previews require macOS 14 or later": t(
    "窗口截图需要 macOS 14 或更新版本。",
  ),
  "Window selection cancelled": t("已取消选择窗口。"),
  "Selected window changed or became unavailable; select it again":
    t("窗口选择已更改，请重新选择。"),
  "Bring the selected window to the front and preview again":
    t("请将所选窗口置于前台，再次预览。"),
  "Window identity changed; select it again": t("窗口选择已更改，请重新选择。"),
  "Control changed; observe again": t("窗口控件已更改，请重新观察。"),
  "Preview requires a fresh observation": t("窗口观察已过期，请重新观察。"),
  "Observe the selected window before capturing it":
    t("窗口观察已过期，请重新观察。"),
  "Screen destination changed; preview again":
    t("截图目的地已更改，请重新预览。"),
  "Window preview expired": t("截图已过期，请重新预览。"),
  "Select a vision-capable screen profile": t("请选择支持图像的模型配置。"),
  "Application did not confirm the action; inspect the window":
    t("应用未确认操作结果，请检查窗口。"),
  "Window changed after dispatch; inspect the result":
    t("操作发送后窗口已更改，请检查结果。"),
  "Focus a standard window in the chosen application and try again": t(
    "请将所选应用的普通窗口置于前台，再试一次。",
  ),
  "This application cannot report window changes":
    t("此应用无法报告窗口变化。"),
  "This application cannot track its selected window":
    t("此应用无法跟踪所选窗口。"),
  "Cannot uniquely identify this window for capture":
    t("无法唯一确定要截图的窗口。"),
  "Observe the window again": t("窗口观察已过期，请重新观察。"),
  "Select a window for this task first": t("请先在 Mac 上为此任务选择窗口。"),
  "Window selection changed": t("窗口选择已更改，请重新选择。"),
  "Take over the selected window before selecting another": t(
    "请先在原任务中接手窗口，再选择其他窗口。",
  ),
  "Task is paused or ended": t("任务已暂停或结束。"),
  "Desktop helper is not installed": t("本机未启用窗口执行器。"),
}));
const element = computed(() =>
  state.observation?.elements.find((item) => item.id === elementId.value),
);
const locked = computed(() => props.blocked || state.busy);
const labels = computed<Record<DesktopAction, string>>(() => ({
  click: t("点击元素"),
  scrollUp: t("向上滚动"),
  scrollDown: t("向下滚动"),
  setText: t("填写文字"),
}));
watch(
  () => state.observation,
  (observation) => {
    elementId.value =
      observation?.elements.find((item) => item.actions.length)?.id ?? null;
    input.value = "";
  },
);
watch(elementId, () => {
  input.value = "";
});
watch(
  [() => props.blocked, () => props.generation, open],
  () => {
    panel.invalidate();
    if (open.value) void panel.refresh();
  },
  { flush: "sync" },
);
onBeforeUnmount(panel.invalidate);
function act(action: DesktopAction) {
  if (!element.value) return;
  const text = action === "setText" ? input.value : undefined;
  input.value = "";
  void panel.act(element.value.id, action, text);
}
async function loadProfiles(event: Event) {
  if (!(event.target as HTMLDetailsElement).open) return;
  profileError.value = "";
  try {
    profiles.value = (
      await preferenceRequest<ModelProfilesResponse>("/api/model/profiles")
    ).profiles;
  } catch (error) {
    profileError.value = errorText(error);
  }
}
function receipt(status: string) {
  if (status === "confirmed") return t("已完成");
  if (status === "rejected") return t("已拒绝操作");
  return t("结果不明，请检查窗口");
}
</script>
<template>
  <details
    class="desktop-panel"
    @toggle="open = ($event.target as HTMLDetailsElement).open"
  >
    <summary>
      <AppWindow :size="15" aria-hidden="true" />{{ t("任务窗口") }}
    </summary>
    <div v-if="open" class="desktop-body">
      <p v-if="state.error" class="task-error" role="alert">
        {{ diagnostics[state.error] || state.error }}
      </p>
      <button
        v-if="!state.status && !state.busy"
        type="button"
        class="quiet-button"
        @click="panel.refresh"
      >
        {{ t("重新读取") }}
      </button>
      <p v-if="state.busy" class="meta" role="status">
        {{ t("等候窗口响应…") }}
      </p>
      <template v-if="state.status">
        <p v-if="!state.status.available" class="meta">
          {{ t("本机未启用窗口执行器。") }}
        </p>
        <template v-else>
          <div class="desktop-actions">
            <button
              v-if="!state.status.selected"
              type="button"
              class="quiet-button"
              :disabled="locked"
              @click="panel.select"
            >
              {{ t("在 Mac 上选择窗口") }}
            </button>
            <button
              v-if="state.status.selected"
              type="button"
              class="quiet-button"
              :disabled="locked"
              @click="panel.observe"
            >
              {{ t("重新观察") }}
            </button>
            <button
              v-if="state.status.selected"
              type="button"
              class="text-button"
              :disabled="blocked"
              @click="panel.takeover"
            >
              {{ t("接手此窗口") }}
            </button>
          </div>
          <template v-if="state.observation">
            <strong class="window-name">{{ state.observation.window }}</strong>
            <label
              v-if="
                state.observation.elements.some((item) => item.actions.length)
              "
            >
              {{ t("可操作元素") }}
              <select v-model="elementId" :disabled="locked">
                <option
                  v-for="item in state.observation.elements.filter(
                    (item) => item.actions.length,
                  )"
                  :key="item.id"
                  :value="item.id"
                >
                  {{ item.label }}
                </option>
              </select>
            </label>
            <p v-else class="meta">{{ t("当前窗口没有可操作的元素。") }}</p>
            <label v-if="element?.actions.includes('setText')"
              >{{ t("写入此元素的文字") }}
              <input
                v-model="input"
                type="text"
                autocomplete="off"
                :disabled="locked"
              />
            </label>
            <div v-if="element" class="desktop-actions">
              <button
                v-for="action in element.actions"
                :key="action"
                type="button"
                class="quiet-button"
                :disabled="locked"
                @click="act(action)"
              >
                {{ labels[action] }}
              </button>
            </div>
          </template>
          <details @toggle="loadProfiles">
            <summary>{{ t("屏幕模型") }}</summary>
            <div class="desktop-body">
              <p v-if="profileError" class="task-error" role="alert">
                {{ profileError }}
              </p>
              <label
                >{{ t("本机窗口截图的模型配置") }}
                <select
                  :value="state.status.screenProfile ?? ''"
                  :disabled="locked || !!profileError"
                  @change="
                    panel.model(
                      ($event.target as HTMLSelectElement).value || null,
                    )
                  "
                >
                  <option value="">{{ t("关闭截图分享") }}</option>
                  <option
                    v-if="
                      state.status.screenProfile &&
                      !profiles.some(
                        (p) => p.id === state.status?.screenProfile,
                      )
                    "
                    :value="state.status.screenProfile"
                  >
                    {{ state.status.screenProfile }}
                  </option>
                  <option
                    v-for="profile in profiles"
                    :key="profile.id"
                    :value="profile.id"
                    :disabled="profile.vision === false"
                  >
                    {{ profile.id }} · {{ profile.model || profile.provider }} ·
                    {{ originLabel(profile.origin) }}
                  </option>
                </select>
              </label>
              <button
                v-if="state.status.screenProfile && state.observation"
                type="button"
                class="quiet-button"
                :disabled="locked"
                @click="panel.capture"
              >
                {{ t("在本地预览窗口截图") }}
              </button>
              <figure v-if="state.frame">
                <img
                  :key="state.frame.frameId"
                  :src="state.frame.dataUrl"
                  :alt="t('待发送的窗口截图')"
                  @load="previewReady = true"
                  @error="previewFailed = true"
                />
                <p v-if="previewFailed" class="task-error" role="alert">
                  {{ t("无法显示截图，请重新预览。") }}
                </p>
                <figcaption>
                  {{
                    t("仅此截图将发送至 {destination}", {
                      destination: state.frame.destination,
                    })
                  }}
                </figcaption>
                <div class="desktop-actions">
                  <button
                    type="button"
                    class="primary-button"
                    :disabled="locked || !previewReady || previewFailed"
                    @click="panel.share"
                  >
                    {{ t("允许发送这张截图") }}
                  </button>
                  <button
                    type="button"
                    class="text-button"
                    @click="panel.clearPreview"
                  >
                    {{ t("放弃截图") }}
                  </button>
                </div>
              </figure>
              <div v-if="state.advice" role="status">
                <strong>{{ t("模型建议") }}</strong>
                <p class="desktop-advice">{{ state.advice }}</p>
              </div>
            </div>
          </details>
        </template>
        <details v-if="state.status.receipts.length">
          <summary>
            {{ t("窗口操作回执") }} · {{ state.status.receipts.length }}
          </summary>
          <ul>
            <li v-for="row in state.status.receipts" :key="row.id">
              {{ labels[row.action] }} · {{ receipt(row.status) }}
            </li>
          </ul>
        </details>
      </template>
    </div>
  </details>
</template>
<style scoped>
.desktop-panel {
  margin: 12px 0;
  padding-top: 10px;
  border-top: 1px solid var(--line);
  font-size: 13px;
  color: var(--text);
}
.desktop-panel > summary {
  display: flex;
  align-items: center;
  gap: 6px;
}
.desktop-panel summary {
  cursor: pointer;
}
.desktop-body,
.desktop-panel label {
  display: grid;
  gap: 8px;
  min-width: 0;
}
.desktop-body {
  margin-top: 10px;
}
.desktop-body > button {
  justify-self: start;
}
.desktop-panel p,
.desktop-panel figure {
  margin: 0;
}
.desktop-panel figcaption {
  margin: 5px 0 8px;
}
.desktop-panel img {
  display: block;
  max-width: 100%;
  max-height: 240px;
  border: 1px solid var(--line);
  border-radius: 6px;
}
.desktop-panel select,
.desktop-panel input {
  box-sizing: border-box;
  width: 100%;
  min-width: 0;
  padding: 7px;
  background: var(--surface);
  color: var(--text);
  border: 1px solid var(--line);
  border-radius: 6px;
  font: inherit;
}
.desktop-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}
.desktop-panel .meta {
  color: var(--text-2);
}
.desktop-panel .window-name,
.desktop-panel figcaption,
.desktop-panel .task-error,
.desktop-advice {
  overflow-wrap: anywhere;
}
.desktop-advice {
  white-space: pre-wrap;
}
.desktop-panel :focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
</style>
