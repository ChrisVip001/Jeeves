import { useBrowsers, type BrowserActions } from "./useBrowsers.ts";
import { reactive } from "vue";
import type {
  AgentTaskView,
  TaskListResponse,
  SessionMessagesResponse,
  TaskArtifactsResponse,
  TaskEnvelope,
  ApprovalRespondParams,
  ApprovalRespondResponse,
  ApprovalRequestedNotification,
  ApprovalPendingResponse,
  ServerNotification,
  TaskGitCheckpointParams,
  TaskGitCheckpointResponse,
  TaskGitCheckpointPolicyParams,
  TaskGitCheckpointPolicyResponse,
} from "./types";
export interface TaskActions {
  browser?: BrowserActions;
  gitCheckpoint?(params: TaskGitCheckpointParams): Promise<TaskGitCheckpointResponse>;
  gitCheckpointPolicy?(taskId: string): Promise<TaskGitCheckpointPolicyResponse>;
  saveGitCheckpointPolicy?(params: TaskGitCheckpointPolicyParams): Promise<TaskGitCheckpointPolicyResponse>;
  list(): Promise<TaskListResponse>;
  pending(sessionId: string): Promise<ApprovalPendingResponse>;
  messages(sessionId: string, cursor?: string | null): Promise<SessionMessagesResponse>;
  artifacts(taskId: string): Promise<TaskArtifactsResponse>;
  action(
    action: "suspend" | "resume" | "cancel",
    taskId: string,
  ): Promise<TaskEnvelope>;
  approve(params: ApprovalRespondParams): Promise<ApprovalRespondResponse>;
}
/** Presentation state only. The runtime owns routing, policy, workspaces and execution. */
export function useTasks(actions: TaskActions) {
  const browser = actions.browser ? useBrowsers(actions.browser) : undefined;
  const state = reactive({
    owner: "",
    loadedOwner: "",
    open: false,
    tasks: [] as AgentTaskView[],
    selected: "",
    detailId: "",
    busy: "",
    error: "",
    results: {} as Record<string, string>,
    deliveries: {} as Record<string, TaskArtifactsResponse["latestDelivery"]>,
    artifacts: {} as Record<string, TaskArtifactsResponse["artifacts"]>,
    approvals: {} as Record<string, ApprovalRequestedNotification>,
    /** Approvals that outlived a restart, by turn: answering grants the resumed turn one yes. */
    restored: {} as Record<string, boolean>,
    /** The last answer that arrived too late: who answered first, or "" when it expired. */
    late: null as { answeredBy: string } | null,
    /** A host showing approvals in place (the Today page) turns the drawer's auto-open off. */
    autoOpen: true,
  });
  const turns = new Map<string, string>();
  let refreshing: Promise<void> | undefined;
  let refreshAgain = false;
  let approvalVersion = 0;
  function taskForTurn(id: string) {
    const execution = turns.get(id);
    return state.tasks.find(
      (task) =>
        task.currentTurnId === id ||
        (execution !== undefined && task.executionSessionId === execution),
    );
  }
  const ownTurn = (id: string) => !!taskForTurn(id);
  function reveal() {
    if (state.autoOpen) state.open = true;
  }
  function retry() {
    state.error = "";
    return refresh();
  }
  async function refresh() {
    if (!state.owner) return;
    if (refreshing) {
      refreshAgain = true;
      return refreshing;
    }
    const owner = state.owner;
    void browser?.refresh(owner);
    const version = approvalVersion;
    refreshing = (async () => {
      try {
        const [page, snapshot] = await Promise.all([
          actions.list(),
          actions.pending(owner),
        ]);
        if (owner !== state.owner) return;
        const tasks = page.tasks.filter((task) => task.sessionId === owner);
        if (version === approvalVersion) {
          const approvals: Record<string, ApprovalRequestedNotification> = {};
          for (const entry of snapshot.approvals) {
            if (
              entry.ownerSessionId !== owner ||
              entry.request.expiresInMs <= 0
            )
              continue;
            if (
              state.approvals[entry.request.turnId]?.requestId !==
              entry.request.requestId
            )
              reveal();
            approvals[entry.request.turnId] = entry.request;
            state.restored[entry.request.turnId] = entry.restored;
            turns.set(entry.request.turnId, entry.sessionId);
            if (
              entry.task?.sessionId === owner &&
              !tasks.some((task) => task.id === entry.task!.id)
            )
              tasks.unshift(entry.task);
          }
          state.approvals = approvals;
        } else refreshAgain = true;
        state.tasks = tasks;
        state.loadedOwner = owner;
      } catch (error) {
        state.error = String(error);
      } finally {
        refreshing = undefined;
        if (refreshAgain) {
          refreshAgain = false;
          void refresh();
        }
      }
    })();
    return refreshing;
  }
  async function openReceipt(taskId: string, destination: "detail" | "reply" | "workspace" = "detail"): Promise<boolean> {
    const owner = state.owner;
    if (!owner) return false;
    const findTask = () => state.tasks.find(task => task.id === taskId && task.sessionId === owner);
    if (!findTask()) await refresh();
    const task = findTask();
    if (state.owner !== owner || !task) return false;
    if (destination === "reply") {
      if (!["pending", "running", "done"].includes(task.status)) return false;
      state.selected = taskId;
    } else if (destination === "detail") state.detailId = taskId;
    return true;
  }
  const inspections = new Map<string, symbol>();
  async function inspect(task: AgentTaskView) {
    const version = Symbol();
    inspections.set(task.id, version);
    const owner = state.owner;
    try {
      const [messages, artifacts] = await Promise.all([
        task.executionSessionId
          ? actions.messages(task.executionSessionId)
          : Promise.resolve(null),
        actions.artifacts(task.id),
      ]);
      if (owner !== state.owner || inspections.get(task.id) !== version) return;
      const reply =
        messages?.data.find((message) => message.role === "assistant")
          ?.content ?? "";
      state.results[task.id] = reply.replace(
        /\n\[tools used:[\s\S]*\]\s*$/,
        "",
      );
      state.artifacts[task.id] = artifacts.artifacts;
      state.deliveries[task.id] = artifacts.latestDelivery;
    } catch (error) {
      if (owner === state.owner && inspections.get(task.id) === version) state.error = String(error);
    } finally {
      if (inspections.get(task.id) === version) inspections.delete(task.id);
    }
  }
  async function action(kind: "suspend" | "resume" | "cancel", id: string) {
    if (state.busy) return;
    state.busy = id;
    state.error = "";
    try {
      await actions.action(kind, id);
      await refresh();
    } catch (error) {
      state.error = String(error);
    } finally {
      state.busy = "";
    }
  }
  const sendCheckpoint = actions.gitCheckpoint;
  const gitCheckpoint = sendCheckpoint && (async (params: TaskGitCheckpointParams): Promise<TaskGitCheckpointResponse> => {
    if (state.busy) throw new Error("Checkpoint unavailable");
    state.busy = params.taskId;
    try {
      const result = await sendCheckpoint(params);
      void refresh();
      return result;
    } finally {
      state.busy = "";
    }
  });
  const sendPolicy = actions.saveGitCheckpointPolicy;
  const saveGitCheckpointPolicy = sendPolicy && (async (params: TaskGitCheckpointPolicyParams): Promise<TaskGitCheckpointPolicyResponse> => {
    if (state.busy) throw new Error("Checkpoint policy unavailable");
    state.busy = params.taskId;
    try { return await sendPolicy(params); }
    finally { state.busy = ""; }
  });
  async function approve(approval: ApprovalRequestedNotification, approved: boolean) {
    const { turnId, requestId, version } = approval;
    const matches = () => state.approvals[turnId]?.requestId === requestId && state.approvals[turnId]?.version === version;
    if (state.busy) return;
    if (!matches()) {
      state.late = { answeredBy: "" };
      await refresh();
      return;
    }
    state.busy = turnId;
    state.error = "";
    approvalVersion++;
    try {
      const result = await actions.approve({
        turnId,
        requestId: approval.requestId,
        approved,
        remember: "once",
        version: approval.version,
      });
      approvalVersion++;
      if (matches()) delete state.approvals[turnId];
      state.late = result.accepted
        ? null
        : { answeredBy: result.answeredBy ?? "" };
    } catch (error) {
      state.error = String(error);
    } finally {
      state.busy = "";
      void refresh();
    }
  }
  function onNotification(event: ServerNotification) {
    if (event.type === "turnStarted") {
      turns.set(event.turnId, event.sessionId);
      if (turns.size > 256) turns.delete(turns.keys().next().value!);
    }
    if (event.type === "taskUpdated") {
      if (
        event.sessionId === state.owner &&
        !state.tasks.some((task) => task.id === event.taskId)
      )
        reveal();
      void refresh();
    }
    if (event.type === "approvalRequested") {
      approvalVersion++;
      if (ownTurn(event.turnId)) {
        state.approvals[event.turnId] = event;
        reveal();
      } else void refresh();
    }
    if (event.type === "toolCompleted" && ownTurn(event.turnId)) {
      approvalVersion++;
      delete state.approvals[event.turnId];
    }
    if (event.type === "permissionChanged" && event.sessionId === state.owner) {
      approvalVersion++;
      for (const id of Object.keys(state.approvals)) delete state.approvals[id];
      void refresh();
    }
    if (event.type === "turnCompleted") {
      if (ownTurn(event.turnId)) approvalVersion++;
      delete state.approvals[event.turnId];
      const task = taskForTurn(event.turnId);
      if (task) {
        state.results[task.id] = event.reply;
        void inspect(task);
      }
      turns.delete(event.turnId);
      void refresh();
    }
  }
  function approvalFor(task: AgentTaskView) {
    return Object.values(state.approvals).find(
      (approval) => taskForTurn(approval.turnId)?.id === task.id,
    );
  }
  /** The task an approval belongs to, when it belongs to one. */
  function taskOf(approval: ApprovalRequestedNotification) {
    return taskForTurn(approval.turnId);
  }
  return {
    messages: actions.messages,
    taskOf,
    browser,
    state,
    refresh,
    retry,
    openReceipt,
    inspect,
    action,
    gitCheckpoint,
    gitCheckpointPolicy: actions.gitCheckpointPolicy,
    saveGitCheckpointPolicy,
    approve,
    onNotification,
    approvalFor,
  };
}
export type TaskCenter = ReturnType<typeof useTasks>;
