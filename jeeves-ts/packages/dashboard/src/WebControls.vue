<script setup lang="ts">
import ApprovalPushSettings from "./ApprovalPushSettings.vue";
import CoworkConversation from "./CoworkConversation.vue";
import { MessageCircle, PanelsTopLeft } from "lucide-vue-next";
import ApprovalNotice from "./ApprovalNotice.vue";
import DesktopPanel from "./ui/DesktopPanel.vue";
import ExecutorStop from "./ui/ExecutorStop.vue";
import { executor, refreshExecutor, changeExecutor } from "./executor-control.ts";
import { computed, provide, ref, watch, onMounted, onUnmounted } from "vue";
import { Settings, CircleAlert } from "lucide-vue-next";
import {
  AttachmentPicker,
  TaskCenter,
  SettingsPanel,
  SetupWizard,
  ModelPicker,
  PermissionPicker,
  WorkspacePicker,
  TodayPage,
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
    state.view = "chat";
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
const recent=computed(()=>[...webTasks.state.tasks].sort((a,b)=>{
  const last=(task:AgentTaskView)=>taskTimelines[task.id]?.at(-1)?.at || task.finishedAt || task.createdAt || "";
  return last(b).localeCompare(last(a));
}).slice(0,5));
function showCowork(id?:string) {
  if (id) coworkId.value=id;
  state.view="cowork";
  webTasks.state.open=false;
  closeMobileNavigation();
}
async function showAssistant(){
  state.view="chat";
  webTasks.state.open=false;
  if (state.mainSessionId && !state.busy) await workspaceActions.main();
}
function taskStatus(task:AgentTaskView) {
  if (webTasks.approvalFor(task) || ["needsAttention","needs_attention"].includes(task.status)) return t("等你决定");
  switch(task.status) {
    case "pending":return t("等待执行");
    case "running":return t("正在执行");
    case "suspended":return t("已暂停");
    case "done":return task.acceptance.length?t("已完成"):t("已生成，待验收");
    case "cancelled":return t("已取消");
    case "failed":return t("执行失败");
    default:return task.status;
  }
}
const detailId = ref("");
const detailTask = computed(() =>
  webTasks.state.tasks.find((task) => task.id === detailId.value),
);
let opener: HTMLElement | null = null;
function openDetail(id: string) {
  opener = document.activeElement as HTMLElement | null;
  detailId.value = id;
}
function closeDetail() {
  detailId.value = "";
  opener?.focus();
}
function showToday() {
  closeMobileNavigation();
  state.view = "today";
}
const needsYou = computed(() => Object.keys(webTasks.state.approvals).length);
watch(
  () => [state.view,phone.value] as const,
  ([view,isPhone]) => {
    document.body.classList.toggle("view-today", view === "today");
    // On Today approvals sit in the page, so the side drawer must not pop over them.
    webTasks.state.autoOpen = false;
    document.body.classList.toggle("view-cowork", !isPhone && view === "cowork");
    document.body.classList.toggle("view-assistant", !isPhone && view === "chat");
    if (view === "today") webTasks.state.open = false;
  },
  { immediate: true },
);
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
const setupDismissed = ref(localStorage.getItem(SETUP_KEY) === "1");
const needsSetup = computed(
  () => state.connected && !state.profiles.length && !setupDismissed.value,
);
function endSetup() {
  setupDismissed.value = true;
  localStorage.setItem(SETUP_KEY, "1");
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
  <Teleport to="#mac-mode-switch">
    <nav v-if="!phone" class="mode-switch" :aria-label="t('工作模式')">
      <button id="mode-assistant" type="button" :aria-pressed="state.view !== 'cowork'" @click="showAssistant"><MessageCircle :size="15" />{{ t("助理") }}</button>
      <button id="mode-cowork" type="button" :aria-pressed="state.view === 'cowork'" @click="showCowork()"><PanelsTopLeft :size="15" />Cowork</button>
    </nav>
  </Teleport>
  <Teleport to="#mode-task-rail">
    <section v-if="!phone && state.view !== 'today'" class="mode-task-list">
      <h2 class="rail-label">{{ state.view === 'cowork' ? t("任务会话") : t("近期活动") }}</h2>
      <p v-if="webTasks.state.error" class="cw-meta" role="status">{{ webTasks.state.error }}</p>
      <p v-else-if="!webTasks.state.tasks.length" class="cw-meta">{{ t("把要做的事告诉 Jeeves，任务会出现在这里。") }}</p>
      <button type="button" v-for="task in state.view === 'cowork' ? webTasks.state.tasks : recent" :key="task.id"
        :id="state.view==='cowork'?'cowork-task-'+task.id:undefined" :data-cowork-task="state.view==='cowork'?task.id:undefined" :aria-current="state.view==='cowork' && coworkId===task.id?'page':undefined"
        @click="showCowork(task.id)">
        <span>{{ task.title }}</span><small>{{ taskStatus(task) }}</small>
      </button>
    </section>
  </Teleport>
  <Teleport to="#cowork-view">
    <template v-if="!phone && state.view==='cowork'">
      <CoworkConversation v-if="coworkTask" :key="coworkTask.id" :task="coworkTask" @details="openDetail(coworkTask.id)" />
      <div v-else class="cw-empty"><PanelsTopLeft :size="28"/><h2>Cowork</h2><p>{{ webTasks.state.tasks.length?t("选择任务会话，查看进展或继续交流。"):t("在助理中交办的任务会显示在这里。") }}</p></div>
    </template>
  </Teleport>
  <Teleport to="#today-entry"
    ><button
      type="button"
      class="settings-entry today-entry"
      :aria-current="state.view === 'today' ? 'page' : undefined"
      @click="showToday"
    >
      {{ t("今天")
      }}<span v-if="needsYou" class="today-entry-count tnum">{{
        needsYou
      }}</span>
    </button></Teleport
  >
  <Teleport to="#phone-tasks">
    <button v-if="phone" type="button" class="ghost" @click="webTasks.state.open = true">{{ t("任务") }}</button>
  </Teleport>
  <Teleport to="#today-view">
    <TodayPage
      v-if="!phone && state.view === 'today'"
      :center="webTasks"
      :timelines="taskTimelines"
      @open="openDetail"
    />
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
