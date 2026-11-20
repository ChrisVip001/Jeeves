<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { X } from "lucide-vue-next";
import { approvalNoticeLink, useApprovalNotice } from "./approval-notice";
import ApprovalCard from "./ui/ApprovalCard.vue";
import { t } from "./i18n";
defineProps<{ mainSessionId: string }>();
const emit = defineEmits<{ close: []; follow: [taskId: string] }>();
const dialog = ref<HTMLDialogElement>();
const notice = useApprovalNotice(approvalNoticeLink(new URLSearchParams(location.search)));
const state = notice.state;
const message = computed(() => {
  switch (state.phase) {
    case "loading": return t("正在核对这条审批…");
    case "revoked": return t("此通知属于另一台设备，或设备授权已撤销。");
    case "expired": return t("这条审批已过期或不再待处理。");
    case "offline": return t("无法连接 Jeeves。连上后再查看这条审批。");
    case "unavailable": return t("暂时无法读取这条审批。");
    case "unknown": return t("尚未收到提交回执，请重新核对审批状态。");
    case "approved": return t("已允许这次操作。");
    case "denied": return t("已拒绝这次操作。");
    case "submitting": return t("正在提交决定…");
    default: return "";
  }
});
let expiry: ReturnType<typeof setTimeout> | undefined;
watch(() => state.approval, approval => {
  clearTimeout(expiry);
  if (approval) expiry = setTimeout(notice.expire, approval.request.expiresInMs);
});
function read() {
  if (navigator.onLine) void notice.load();
  else notice.disconnected();
}
onMounted(() => {
  dialog.value?.showModal();
  window.addEventListener("offline", notice.disconnected);
  read();
});
onUnmounted(() => {
  clearTimeout(expiry);
  window.removeEventListener("offline", notice.disconnected);
});
</script>
<template>
  <dialog ref="dialog" class="task-detail approval-notice" aria-labelledby="notice-heading" @close="emit('close')">
    <div class="td-inner">
      <header class="td-head">
        <h2 id="notice-heading">{{ t("任务审批") }}</h2>
        <button type="button" class="today-icon" :aria-label="t('关闭')" @click="dialog?.close()">
          <X :size="18" aria-hidden="true" />
        </button>
      </header>
      <ApprovalCard v-if="state.approval?.task" :key="state.approval.request.requestId"
        :approval="state.approval.request" :task="state.approval.task" :restored="state.approval.restored"
        :busy="state.phase !== 'pending'" @answer="notice.answer" />
      <button v-if="state.phase === 'pending' && state.approval?.task?.sessionId === mainSessionId && mainSessionId" type="button" class="today-btn"
        @click="emit('follow', state.approval.task.id)">{{ t("在主聊天跟进") }}</button>
      <p role="status" class="notice-status">{{ message }}</p>
      <button v-if="['offline', 'unavailable', 'unknown'].includes(state.phase)"
        type="button" class="today-btn" @click="read">{{ t("重新核对") }}</button>
    </div>
  </dialog>
</template>
<style scoped>
.approval-notice .td-head { align-items: center; margin-bottom: 20px; }
.approval-notice .today-icon, .approval-notice :deep(.today-btn) { min-height: 44px; min-width: 44px; }
.notice-status { color: var(--text-2); line-height: 1.65; }
.notice-status:empty { margin: 0; }
</style>
