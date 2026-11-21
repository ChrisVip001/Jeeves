<script setup lang="ts">
import { hostedBase, accountKey } from "./hosted.ts";
import ApprovalPushSettings from "./ApprovalPushSettings.vue";
import CoworkConversation from "./CoworkConversation.vue";
import ActivityPage from "./ActivityPage.vue";
import ChatDecisions from "./ChatDecisions.vue";
import ApprovalNotice from "./ApprovalNotice.vue";
import DesktopPanel from "./ui/DesktopPanel.vue";
import ExecutorStop from "./ui/ExecutorStop.vue";
import { executor, refreshExecutor, changeExecutor } from "./executor-control.ts";
import { computed, provide, ref, toRef, watch, onMounted, onUnmounted } from "vue";
import { Settings, CircleAlert } from "lucide-vue-next";
import {
  AttachmentPicker,
  TaskCenter,
  SettingsPanel,
  SetupWizard,
  ModelPicker,
  PermissionPicker,
  WorkspacePicker,
  TaskDetail,
} from "./ui/index.ts";
import { uiTranslatorKey } from "./ui/index.ts";
import type { AgentTaskView, PermissionMode } from "./ui/types.ts";
import { t } from "./i18n";
import { phoneLayout } from "./phone";
import { closeMobileNavigation } from "./navigation";
import {
  webState as state,
  workspaceActions,
  refreshConnections,
  refreshPolicy,
  setPermission,
  activateModel,
  saveWorkspace,
  settingsActions,
  remember,
} from "./controls";
import { webTasks, connectTasks, taskTimelines } from "./tasks";
provide(uiTranslatorKey, t);
const hosted = hostedBase();
const executorProps = computed(() => ({
  view: executor.view,
  busy: executor.busy,
  readFailed: executor.readFailed,
  message: executor.message,
}));
function desktopProps(task: AgentTaskView) {
  return {
    taskId: task.id,
    blocked: ["suspended", "cancelled", "failed", "done"].includes(task.status) ||
      executor.busy || !executor.view || executor.view.stopped,
    generation: executor.view?.generation,
  };
}
const executorActions = {
  stop: () => changeExecutor("stop"),
  release: () => changeExecutor("release"),
  refresh: refreshExecutor,
};
const followingTask = computed(() =>
  webTasks.state.tasks.find((task) => task.id === webTasks.state.selected),
);
const media = matchMedia(phoneLayout);
const phone = ref(media.matches);
function layout() {
  phone.value = media.matches;
  if (phone.value) {
    if (state.view === "cowork") state.view = "activity";
    webTasks.state.autoOpen = false;
    if (state.mainSessionId && state.sessionId !== state.mainSessionId) void workspaceActions.main();
  }
}
let disconnect: (() => void) | undefined;
onMounted(() => {
  layout();
  media.addEventListener("change", layout);
  disconnect = connectTasks();
});
onUnmounted(() => {
  media.removeEventListener("change", layout);
  disconnect?.();
});
async function followTask(id: string) {
  detailId.value = "";
  state.view = "chat";
  if (state.sessionId !== state.mainSessionId) await workspaceActions.main();
  if (!state.mainSessionId || state.sessionId !== state.mainSessionId) return;
  document.querySelector<HTMLTextAreaElement>("#input")?.focus();
  webTasks.state.selected = id;
  webTasks.state.open = false;
}
const coworkId=ref("");
const coworkTask=computed(()=>webTasks.state.tasks.find(task=>task.id===coworkId.value));
function inspectActivity(id: string) {
  if (phone.value) openDetail(id);
  else {
    coworkId.value = id;
    state.view = "cowork";
  }
}
async function showAssistant() {
  state.view = "chat";
  webTasks.state.open = false;
  closeMobileNavigation();
  if (state.mainSessionId && state.sessionId !== state.mainSessionId && !state.busy) await workspaceActions.main();
}
const detailId = toRef(webTasks.state, "detailId");
const detailTask = computed(() =>
  webTasks.state.tasks.find((task) => task.id === detailId.value),
);
let opener: HTMLElement | null = null;
watch(detailId, (next) => {
  if (next) opener = document.activeElement as HTMLElement | null;
}, { flush: "sync" });
function openDetail(id: string) {
  detailId.value = id;
}
function closeDetail() {
  detailId.value = "";
  opener?.focus();
}
function showActivity() {
  closeMobileNavigation();
  webTasks.state.open = false;
  state.view = "activity";
}
const needsYou = computed(() => Object.keys(webTasks.state.approvals).length);
watch(() => state.view, view => {
  document.body.classList.toggle("view-activity", view === "activity");
  document.body.classList.toggle("view-cowork", view === "cowork");
  webTasks.state.autoOpen = false;
}, { immediate: true });
watch(
  () => webTasks.state.open,
  (value) => document.body.classList.toggle("jeeves-with-task-center", value),
);
const noticeOpen = ref(new URLSearchParams(location.search).has("approvalRequestId"));
async function followNotice(id: string) {
  await followTask(id);
  if (state.sessionId === state.mainSessionId && webTasks.state.selected === id) {
    closeNotice();
    document.querySelector<HTMLTextAreaElement>("#input")?.focus();
  }
}
function closeNotice() {
  noticeOpen.value = false;
  const url = new URL(location.href);
  for (const key of ["deviceId", "grantId", "generation", "taskId", "turnId", "approvalRequestId", "approvalVersion"]) {
    url.searchParams.delete(key);
  }
  history.replaceState(history.state, "", url);
}
const open = ref(false);
const tab = ref("general");
function settings(value = "general") {
  closeMobileNavigation();
  tab.value = value;
  open.value = true;
}
function manageMcp() {
  open.value = false;
  const trigger = document.querySelector<HTMLButtonElement>("#cap-mcp");
  trigger?.click();
  trigger?.focus();
}
const SETUP_KEY = "jeeves.setup.dismissed";
const setupDismissed = ref(localStorage.getItem(accountKey(SETUP_KEY)) === "1");
const needsSetup = computed(
  () => state.connected && !state.profiles.length && !setupDismissed.value,
);
function endSetup() {
  setupDismissed.value = true;
  localStorage.setItem(accountKey(SETUP_KEY), "1");
  void changed();
}
function chooseMode(mode: PermissionMode) {
  if (state.policy && state.policy.mode !== mode) void setPermission(mode, false);
}
const unavailable = computed(() => state.busy || !state.connected);
watch(
  () => state.connected,
  (ready) => {
    if (ready)
      void refreshConnections().catch(
        (error: unknown) => (state.message = String(error)),
      );
  },
);
watch(
  () => state.sessionId,
  () => {
    state.policy = undefined;
    state.policyError = "";
    void refreshPolicy();
  },
);
async function changed() {
  await refreshPolicy();
  await refreshConnections();
  await workspaceActions.refresh();
}
</script>
<template>
  <Teleport to="#primary-navigation">
    <nav class="primary-navigation" :aria-label="t('主导航')">
      <button id="mode-assistant" type="button" :aria-current="state.view === 'chat' ? 'page' : undefined" @click="showAssistant">{{ t("对话") }}</button>
      <button id="mode-cowork" type="button" :aria-current="state.view !== 'chat' ? 'page' : undefined" @click="showActivity">{{ t("动态") }}<span v-if="needsYou" class="decision-count" :aria-label="t('待确认（{count}）', {count: needsYou})">{{ needsYou }}</span></button>
    </nav>
  </Teleport>
  <Teleport to="#activity-view">
    <ActivityPage v-if="state.view === 'activity'" :center="webTasks" @inspect="inspectActivity" @chat="showAssistant" />
  </Teleport>
  <Teleport to="#chat-decisions">
    <ChatDecisions v-if="state.view === 'chat' && state.sessionId === state.mainSessionId" :center="webTasks" :main-busy="state.busy" />
  </Teleport>
  <Teleport to="#cowork-view">
    <CoworkConversation v-if="state.view === 'cowork' && coworkTask" :key="coworkTask.id" :task="coworkTask" @details="openDetail(coworkTask.id)" @back="showActivity" />
  </Teleport>
  <Teleport to="#task-progress-entry"
    ><button
      type="button"
      class="settings-entry"
      @click="closeMobileNavigation(); webTasks.state.open = !webTasks.state.open"
    >
      {{ t("任务进展")
      }}<span style="margin-left: auto">{{
        webTasks.state.tasks.filter(
          (task) => !["done", "failed", "cancelled"].includes(task.status),
        ).length
      }}</span>
    </button></Teleport
  >
  <Teleport to="#composer-attachments">
    <AttachmentPicker @files="workspaceActions.upload()" />
  </Teleport>
  <Teleport to="#connections-entry">
    <a v-if="hosted" href="/account/" class="cap-entry">{{ t("个人中心") }}</a>
    <button class="cap-entry" type="button" @click="settings('connections')"><span class="cap-entry-name">{{ t('连接管理') }}</span></button>
  </Teleport>
  <Teleport to="#settings-entry"
    ><button class="settings-entry" type="button" @click="settings()">
      <Settings :size="16" />{{ t("设置")
      }}<span style="margin-left: auto; font-size: 10px">⌘ ,</span>
    </button></Teleport
  >
  <Teleport to="#composer-context"
    ><WorkspacePicker
      :cwd="followingTask?.cwd || state.cwd"
      :automatic="state.sessionId === state.mainSessionId"
      :busy="unavailable || !!followingTask"
      can-browse
      @save="saveWorkspace"
      @browse="workspaceActions.browse()" /><PermissionPicker
      :policy="state.policy"
      :disabled="!state.connected"
      :loading="state.policyLoading"
      :error="state.policyError"
      :saving="state.policySaving"
      @change="setPermission"
      @retry="refreshPolicy()"
      @manage="settings('permissions')"
  /></Teleport>
  <Teleport to="#composer-model"
    ><ModelPicker
      :profiles="state.profiles"
      :fallback="state.config?.model"
      :busy="unavailable"
      @select="activateModel"
      @manage="settings('models')"
  /></Teleport>
  <Teleport to="#composer-target">
    <div v-if="followingTask" class="task-following">
      {{ t("补充要求") }}：{{ followingTask.title
      }}<button
        type="button"
        :aria-label="t('取消任务归属')"
        @click="webTasks.state.selected = ''"
      >
        ×
      </button>
    </div>
  </Teleport>
  <Teleport to="body">
    <ApprovalNotice v-if="noticeOpen" :main-session-id="state.mainSessionId" @close="closeNotice" @follow="followNotice" />
    <SetupWizard
      v-if="needsSetup && !noticeOpen"
      :models="state.models"
      :has-model="false"
      :actions="settingsActions"
      :remember="remember"
      @permission="chooseMode"
      @skip="endSetup"
      @done="endSetup"
    />
    <TaskCenter :center="webTasks" :read-only="phone" @inspect="openDetail" @follow="followTask">
      <template #desktop="{ task }">
        <DesktopPanel :key="task.id" v-bind="desktopProps(task)" />
      </template>
      <template #executor><ExecutorStop v-bind="executorProps" v-on="executorActions" inline /></template>
    </TaskCenter>
    <TaskDetail
      v-if="detailTask"
      :key="detailTask.id"
      :task="detailTask"
      :read-only="phone"
      :events="taskTimelines[detailTask.id]"
      :center="webTasks"
      @close="closeDetail"
      @follow="followTask"
    >
      <template #desktop="{ task }">
        <DesktopPanel :key="task.id" v-bind="desktopProps(task)" />
      </template>
      <template #executor><ExecutorStop v-bind="executorProps" v-on="executorActions" inline /></template>
    </TaskDetail>
    <ExecutorStop v-if="!webTasks.state.open && !detailTask" v-bind="executorProps" v-on="executorActions" />
    <SettingsPanel
      v-if="open"
      :profiles="state.profiles"
      :models="state.models"
      :config="state.config"
      :policy="state.policy"
      :busy="unavailable"
      :actions="settingsActions"
      :initial-tab="tab"
      @close="open = false"
      @changed="changed"
      @mcp="manageMcp"
    >
      <template #approval-notifications><ApprovalPushSettings /></template>
    </SettingsPanel>
    <div v-if="state.message" class="web-notice" role="status">
      <CircleAlert :size="16" /><span>{{ state.message }}</span
      ><button
        v-if="!state.connected"
        type="button"
        @click="workspaceActions.refresh()"
      >
        {{ t("重新连接") }}</button
      ><button
        type="button"
        :aria-label="t('关闭')"
        @click="state.message = ''"
      >
        ×
      </button>
    </div>
  </Teleport>
</template>
