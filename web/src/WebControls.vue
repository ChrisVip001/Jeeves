<script setup lang="ts">
import { accountKey } from "./hosted.ts";
import ApprovalPushSettings from "./ApprovalPushSettings.vue";
import CoworkConversation from "./CoworkConversation.vue";
import WorkScreen from "./WorkScreen.vue";
import { waitingCount } from "./work-status";
import ActivityPage from "./ActivityPage.vue";
import ResponsibilityCards from "./ResponsibilityCards.vue";
import LongTerm from "./LongTerm.vue";
import ChatDecisions from "./ChatDecisions.vue";
import ApprovalNotice from "./ApprovalNotice.vue";
import BrowserPanel from "./ui/BrowserPanel.vue";
import DesktopPanel from "./ui/DesktopPanel.vue";
import ExecutorStop from "./ui/ExecutorStop.vue";
import { executor, refreshExecutor, changeExecutor } from "./executor-control.ts";
import { computed, provide, ref, toRef, watch, onMounted, onUnmounted } from "vue";
import { Settings, CircleAlert, ListTodo, MessageCircle, CornerUpLeft } from "lucide-vue-next";
import {
  AttachmentPicker,
  TaskCenter,
  SettingsPanel,
  SetupWizard,
  WorkspacePicker,
  TaskDetail,
} from "./ui/index.ts";
import { uiTranslatorKey } from "./ui/index.ts";
import type { AgentTaskView } from "./ui/types.ts";
import { t } from "./i18n";
import { SETUP_EVENT } from "./render.ts";
import { phoneLayout } from "./phone";
import {
  webState as state,
  workspaceActions,
  refreshConnections,
  refreshPolicy,
  saveWorkspace,
  settingsActions,
  remember,
} from "./controls";
import { webTasks, connectTasks, taskTimelines } from "./tasks";
provide(uiTranslatorKey, t);
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
webTasks.state.autoOpen = false;
const media = matchMedia(phoneLayout);
const phone = ref(media.matches);
function layout() {
  phone.value = media.matches;
  if (phone.value) {
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
  setRoute("chat");
  if (state.sessionId !== state.mainSessionId) await workspaceActions.main();
  if (!state.mainSessionId || state.sessionId !== state.mainSessionId) return;
  document.querySelector<HTMLTextAreaElement>("#input")?.focus();
  webTasks.state.selected = id;
  webTasks.state.open = false;
}
const route = new URL(location.href);
const coworkId = ref(route.searchParams.get("work") ?? "");
const coworkTask = computed(() => webTasks.state.tasks.find(task => task.id === coworkId.value && task.sessionId === webTasks.state.owner));
function setRoute(view: "chat" | "activity" | "cowork", id = "", push = true) {
  const url = new URL(location.href);
  url.searchParams.delete("work");
  url.searchParams.delete("list");
  if (view === "cowork") url.searchParams.set("work", id);
  if (view === "activity") url.searchParams.set("list", "1");
  if (push && url.href !== location.href) history.pushState(null, "", url);
  coworkId.value = id;
  state.view = view;
  webTasks.state.open = false;
}
function restoreRoute() {
  const params = new URLSearchParams(location.search);
  const id = params.get("work");
  setRoute(id ? "cowork" : params.has("list") ? "activity" : "chat", id ?? "", false);
}
// ⌘1 对话 · ⌘2 工作 · ⌘L the message box (Ctrl on other systems).
function shortcut(event: KeyboardEvent) {
  if (!(event.metaKey || event.ctrlKey) || event.altKey || event.shiftKey || document.querySelector("dialog[open]")) return;
  if (event.key === "1") setRoute("chat");
  else if (event.key === "2") setRoute("activity");
  else if (event.key.toLowerCase() === "l") { setRoute("chat"); requestAnimationFrame(() => document.querySelector<HTMLTextAreaElement>("#input")?.focus()); }
  else return;
  event.preventDefault();
}
onMounted(() => { workspaceActions.openWork = inspectActivity; restoreRoute(); window.addEventListener("popstate", restoreRoute); window.addEventListener("keydown", shortcut); });
onUnmounted(() => { window.removeEventListener("popstate", restoreRoute); window.removeEventListener("keydown", shortcut); });
function inspectActivity(id: string) {
  sessionStorage.setItem(accountKey("jeeves.last-work"), id);
  lastWorkId.value = id;
  setRoute("cowork", id);
}
const lastWorkId = ref(sessionStorage.getItem(accountKey("jeeves.last-work")) ?? "");
const lastWork = computed(() => webTasks.state.tasks.find(task => task.id === lastWorkId.value && task.sessionId === webTasks.state.owner));
async function showAssistant() {
  setRoute("chat");
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
  setRoute("activity");
}
const needsYou = computed(() => waitingCount(webTasks));
// The tab title (and, in the Mac app, the Dock badge read from it) shows what waits for you.
watch(needsYou, count => { document.title = count ? `(${count}) Jeeves` : "Jeeves"; }, { immediate: true });
watch(() => state.view, view => {
  if (view === "chat" && (new URLSearchParams(location.search).has("work") || new URLSearchParams(location.search).has("list"))) {
    const url = new URL(location.href);
    url.searchParams.delete("work");
    url.searchParams.delete("list");
    history.replaceState(null, "", url);
  }
  document.body.classList.toggle("view-activity", view === "activity");
  document.body.classList.toggle("view-cowork", view === "cowork");
});
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
const tab = ref<"notifications" | "connections">("notifications");
function settings(value: "notifications" | "connections" = "notifications") {
  tab.value = value;
  open.value = true;
}
const SETUP_KEY = "jeeves.setup.dismissed";
const setupDismissed = ref(localStorage.getItem(accountKey(SETUP_KEY)) === "1");
const needsSetup = computed(
  () => state.connected && !state.managed && state.profiles?.length === 0 && !setupDismissed.value,
);
function endSetup() {
  setupDismissed.value = true;
  localStorage.setItem(accountKey(SETUP_KEY), "1");
  void workspaceActions.refresh();
}
function reopenSetup() {
  void refreshConnections()
    .then(() => {
      setupDismissed.value = false;
      localStorage.removeItem(accountKey(SETUP_KEY));
    })
    .catch((error: unknown) => (state.message = String(error)));
}
onMounted(() => window.addEventListener(SETUP_EVENT, reopenSetup));
onUnmounted(() => window.removeEventListener(SETUP_EVENT, reopenSetup));
const unavailable = computed(() => state.busy || !state.connected);
watch(
  () => state.connected,
  (ready) => {
    if (ready && !state.managed && !setupDismissed.value)
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
</script>
<template>
  <Teleport to="#bottom-tabs">
    <nav class="bottom-tabs" :aria-label="t('主导航')">
      <button type="button" :aria-current="state.view === 'chat' ? 'page' : undefined" @click="showAssistant"><MessageCircle :size="20" /><span>{{ t("对话") }}</span></button>
      <button type="button" :aria-current="state.view !== 'chat' ? 'page' : undefined" @click="showActivity"><ListTodo :size="20" /><span>{{ t("工作") }}</span><span v-if="needsYou" class="decision-count">{{ needsYou }}</span></button>
      <button type="button" @click="settings()"><Settings :size="20" /><span>{{ t("设置") }}</span></button>
    </nav>
  </Teleport>
  <Teleport to="#primary-navigation">
    <nav class="primary-navigation" :aria-label="t('主导航')">
      <button id="mode-assistant" type="button" :aria-label="t('对话')" :title="`${t('对话')} (⌘1)`" :aria-current="state.view === 'chat' ? 'page' : undefined" @click="showAssistant"><MessageCircle :size="17" />{{ t("对话") }}</button>
      <button id="mode-cowork" type="button" :title="`${t('工作')} (⌘2)`" :aria-current="state.view !== 'chat' ? 'page' : undefined" @click="showActivity"><ListTodo :size="17" />{{ t("工作") }}<span v-if="needsYou" class="decision-count">{{ needsYou }}</span></button>
    </nav>
  </Teleport>
  <Teleport to="#chat-decisions">
    <button v-if="lastWork && state.view === 'chat' && state.sessionId === state.mainSessionId && !followingTask" class="return-work" type="button" @click="inspectActivity(lastWork.id)"><CornerUpLeft :size="15" /><span>{{ t("继续上次的事项") }}</span><strong>{{ lastWork.title }}</strong></button>
    <ResponsibilityCards v-if="state.view === 'chat' && state.sessionId === state.mainSessionId" :busy="state.busy" />
    <ChatDecisions v-if="state.view === 'chat' && state.sessionId === state.mainSessionId" :center="webTasks" :main-busy="state.busy" />
  </Teleport>
  <Teleport to="#cowork-view">
    <div v-if="state.view !== 'chat'" class="work-split" :class="{ 'has-detail': state.view === 'cowork' }">
      <div class="work-list-pane">
        <ActivityPage :center="webTasks" :selected="state.view === 'cowork' ? coworkId : ''" @inspect="inspectActivity" @chat="showAssistant">
          <ResponsibilityCards :busy="state.busy" />
          <template #after><LongTerm :busy="state.busy" /></template>
        </ActivityPage>
      </div>
      <div class="work-detail-pane">
        <CoworkConversation v-if="state.view === 'cowork' && coworkTask" :key="coworkTask.id" :task="coworkTask" :phone="phone" @details="openDetail(coworkTask.id)" @back="showActivity" @follow="followTask">
          <template #screen><WorkScreen :key="'screen-' + coworkTask.id" :task="coworkTask" :center="webTasks" /></template>
          <template #tools>
            <BrowserPanel v-if="!phone && webTasks.browser" :center="webTasks.browser" :task-id="coworkTask.id" :cancelled="coworkTask.status === 'cancelled'" />
            <DesktopPanel v-if="!phone" v-bind="desktopProps(coworkTask)" />
          </template>
        </CoworkConversation>
        <div v-else-if="state.view === 'cowork'" class="missing-work" role="status">
          <p>{{ webTasks.state.error || (webTasks.state.loadedOwner ? t("暂时无法打开原任务，请重试。") : t("正在读取任务…")) }}</p>
          <button type="button" class="today-btn" @click="showActivity">{{ t("工作") }}</button>
        </div>
        <div v-else class="work-placeholder"><ListTodo :size="26" aria-hidden="true" /><p>{{ t("选一件事，在这里查看进展和交付。") }}</p></div>
      </div>
    </div>
  </Teleport>
  <Teleport to="#composer-attachments">
    <AttachmentPicker @files="workspaceActions.upload()" />
  </Teleport>
  <Teleport to="#settings-entry"
    ><button class="settings-entry ghost" type="button" :aria-label="t('设置')" :title="t('设置') + ' (⌘,)'" @click="settings()">
      <Settings :size="17" aria-hidden="true" />
    </button></Teleport
  >
  <Teleport :to="state.cwd && !followingTask ? '#composer-target' : '#composer-context'"
    ><WorkspacePicker
      :cwd="followingTask?.cwd || state.cwd"
      :automatic="state.sessionId === state.mainSessionId"
      :busy="unavailable || !!followingTask"
      can-browse
      @save="saveWorkspace"
      @browse="workspaceActions.browse()" /></Teleport>
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
      :actions="settingsActions"
      :remember="remember"
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
      :busy="unavailable"
      :actions="settingsActions"
      :initial-tab="tab"
      @close="open = false"
    >
      <template #approval-notifications><ApprovalPushSettings /></template>
    </SettingsPanel>
    <div v-if="state.message || state.policyError" class="web-notice" role="status">
      <CircleAlert :size="16" /><span>{{ state.message || state.policyError }}</span
      ><button
        v-if="!state.connected"
        type="button"
        @click="workspaceActions.refresh()"
      >
        {{ t("重新连接") }}</button
      ><button
        type="button"
        :aria-label="t('关闭')"
        @click="state.message ? (state.message = '') : (state.policyError = '')"
      >
        ×
      </button>
    </div>
  </Teleport>
</template>
