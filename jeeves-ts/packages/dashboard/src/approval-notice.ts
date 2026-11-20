import { reactive } from "vue";
import { signApprovalNotice } from "./approval-push.ts";
import { preferenceRequest } from "./api.ts";
import type {
  ApprovalNoticeParams, ApprovalNoticeView, ApprovalNoticeDecisionParams,
  ApprovalRespondResponse, PendingApprovalView,
} from "./protocol.ts";

export function approvalNoticeLink(query: URLSearchParams): ApprovalNoticeParams | null {
  const deviceId = query.get("deviceId");
  const grantId = query.get("grantId");
  const taskId = query.get("taskId");
  const turnId = query.get("turnId");
  const approvalRequestId = query.get("approvalRequestId");
  const generation = Number(query.get("generation"));
  const approvalVersion = Number(query.get("approvalVersion"));
  if (!deviceId || !grantId || !taskId || !turnId || !approvalRequestId ||
    !Number.isInteger(generation) || generation < 1 || generation > 2147483647 ||
    !Number.isInteger(approvalVersion) || approvalVersion < 1 || approvalVersion > 2147483647) return null;
  return { deviceId, grantId, generation, taskId, turnId, approvalRequestId, approvalVersion };
}

type Phase = "loading" | "pending" | "offline" | "unavailable" | "expired" | "revoked" |
  "approved" | "denied" | "unknown" | "submitting";

export function useApprovalNotice(link: ApprovalNoticeParams | null, sign = signApprovalNotice) {
  const state = reactive({ phase: (link ? "loading" : "expired") as Phase, approval: null as PendingApprovalView | null });
  let read = 0;
  let deadline = 0;
  function disconnected() {
    if (state.phase !== "loading" && state.phase !== "pending") return;
    ++read;
    state.approval = null;
    state.phase = "offline";
  }
  function expire() {
    if (state.phase !== "pending") return;
    state.phase = "expired";
    state.approval = null;
  }
  async function load() {
    if (!link || state.phase === "submitting") return;
    const version = ++read;
    state.phase = "loading";
    state.approval = null;
    const query = new URLSearchParams();
    for (const [key, value] of Object.entries(link)) query.set(key, String(value));
    try {
      query.set("signature", await sign(link, null));
      if (read !== version) return;
      const result = await preferenceRequest<ApprovalNoticeView>(`/api/approval/notice?${query}`);
      if (read !== version) return;
      const approval = result.approval;
      if (result.status === "revoked") { state.phase = "revoked"; return; }
      if (result.status !== "pending" || approval?.task?.id !== link.taskId ||
        approval.request.turnId !== link.turnId || approval.request.requestId !== link.approvalRequestId ||
        approval.request.version !== link.approvalVersion || approval.request.expiresInMs <= 0) {
        state.phase = "expired";
        return;
      }
      deadline = Date.now() + approval.request.expiresInMs;
      state.approval = approval;
      state.phase = "pending";
    } catch (error) {
      if (version === read) {
        if (error instanceof Error && error.message === "revoked") state.phase = "revoked";
        else state.phase = error instanceof TypeError ? "offline" : "unavailable";
      }
    }
  }
  async function answer(approved: boolean) {
    if (!link || state.phase !== "pending") return;
    if (Date.now() >= deadline) { expire(); return; }
    state.phase = "submitting";
    let signature: string;
    try { signature = await sign(link, approved); }
    catch (error) {
      state.phase = error instanceof Error && error.message === "revoked" ? "revoked" : "unavailable";
      state.approval = null;
      return;
    }
    try {
      const result = await preferenceRequest<ApprovalRespondResponse>("/api/approval/notice",
        { ...link, approved, signature } satisfies ApprovalNoticeDecisionParams);
      if (!result.accepted) {
        state.phase = "expired";
        await load();
        return;
      }
      state.approval = null;
      state.phase = approved ? "approved" : "denied";
    } catch {
      state.phase = "unknown";
      state.approval = null;
    }
  }
  return { state, load, answer, disconnected, expire };
}
