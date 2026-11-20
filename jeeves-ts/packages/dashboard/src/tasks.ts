/** Browser transport for the shared task view. No routing or policy lives here. */
import { refreshExecutor } from "./executor-control.ts";
import { reactive } from "vue";
import { TaskEventFollower } from "../../client/src/task-events.ts";
import { useTasks } from "./ui/useTasks.ts";
import {
  preferenceRequest,
  fetchMessages,
  respondApproval,
  followNotifications,
} from "./api.ts";
import type {
  TaskListResponse,
  ApprovalPendingResponse,
  TaskArtifactsResponse,
  TaskEnvelope,
  TaskEventView,
  TaskEventsResponse,
  TaskGitCheckpointResponse,
  TaskGitCheckpointPolicyResponse,
} from "./protocol";
export const webTasks = useTasks({
  browser: {
    list: (sessionId) =>
      preferenceRequest(
        `/api/browser?sessionId=${encodeURIComponent(sessionId)}`,
      ),
    start: (params) => preferenceRequest("/api/browser/start", params),
    control: (params) => preferenceRequest("/api/browser/control", params),
    input: (params) => preferenceRequest("/api/browser/input", params),
    downloads: (taskId) => preferenceRequest(`/api/browser/downloads?taskId=${encodeURIComponent(taskId)}`),
    download: (params) => preferenceRequest("/api/browser/download", params),
    frame: (taskId) =>
      preferenceRequest(
        `/api/browser/frame?taskId=${encodeURIComponent(taskId)}`,
      ),
  },
  list: async () => {
    const tasks: TaskListResponse["tasks"] = [];
    let cursor: string | undefined;
    do {
      const page: TaskListResponse & { nextCursor?: string | null } = await preferenceRequest(
        cursor ? `/api/tasks?cursor=${encodeURIComponent(cursor)}` : "/api/tasks");
      tasks.push(...page.tasks);
      cursor = page.nextCursor ?? undefined;
    } while (cursor);
    return { tasks };
  },
  pending: (sessionId) =>
    preferenceRequest<ApprovalPendingResponse>(
      `/api/approval/pending?sessionId=${encodeURIComponent(sessionId)}`,
    ),
  messages: fetchMessages,
  artifacts: (taskId) =>
    preferenceRequest<TaskArtifactsResponse>("/api/task", {
      action: "artifacts",
      taskId,
    }),
  action: (action, taskId) =>
    preferenceRequest<TaskEnvelope>("/api/task", { action, taskId }),
  gitCheckpoint: async (params) => {
    const result = await preferenceRequest<TaskGitCheckpointResponse>("/api/task/git-checkpoint", params);
    await catchUpTaskEvents();
    return result;
  },
  gitCheckpointPolicy: taskId => preferenceRequest<TaskGitCheckpointPolicyResponse>(`/api/task/git-checkpoint-policy?taskId=${encodeURIComponent(taskId)}`),
  saveGitCheckpointPolicy: params => preferenceRequest<TaskGitCheckpointPolicyResponse>("/api/task/git-checkpoint-policy", params),
  approve: respondApproval,
});
/** Per-task timelines, built only from cursor catch-ups (kept in memory per tab). */
export const taskTimelines = reactive<Record<string, TaskEventView[]>>({});
const follower = new TaskEventFollower((params) => {
  const query = new URLSearchParams();
  if (params.cursor != null) query.set("cursor", String(params.cursor));
  if (params.limit != null) query.set("limit", String(params.limit));
  return preferenceRequest<TaskEventsResponse>(`/api/task/events?${query}`);
});
/** Pull what is newer than our cursor and append it; only a reset reloads the snapshot. */
async function pullTaskEvents(): Promise<void> {
  try {
    const { events, reset } = await follower.catchUp();
    if (reset) {
      for (const id of Object.keys(taskTimelines)) delete taskTimelines[id];
      void webTasks.refresh();
    }
    for (const event of events) (taskTimelines[event.taskId] ??= []).push(event);
  } catch (error) {
    webTasks.state.error = String(error);
  }
}
let eventCatchUp: Promise<void> | undefined;
let eventCatchUpAgain = false;
/** A checkpoint can request an immediate refresh while the poll is reading. Serialize cursor
 * reads and give a concurrent caller a fresh pass, rather than racing an older page/reset. */
export function catchUpTaskEvents(): Promise<void> {
  if (eventCatchUp) {
    eventCatchUpAgain = true;
    return eventCatchUp;
  }
  eventCatchUp = (async () => {
    try {
      do {
        eventCatchUpAgain = false;
        await pullTaskEvents();
      } while (eventCatchUpAgain);
    } finally { eventCatchUp = undefined; }
  })();
  return eventCatchUp;
}
/** Polling repairs missed events; notifications carry live task approvals. */
export function connectTasks(): () => void {
  const abort = new AbortController();
  let retry: ReturnType<typeof setTimeout> | undefined;
  let wake: (() => void) | undefined;
  async function follow() {
    while (!abort.signal.aborted) {
      try {
        for await (const event of followNotifications(abort.signal))
          webTasks.onNotification(event);
      } catch (error) {
        if (!abort.signal.aborted) webTasks.state.error = String(error);
      }
      if (!abort.signal.aborted)
        await new Promise<void>((resolve) => {
          wake = resolve;
          retry = setTimeout(resolve, 2000);
        });
    }
  }
  void follow();
  void refreshExecutor();
  void catchUpTaskEvents();
  const poll = setInterval(() => {
    void refreshExecutor();
    void webTasks.refresh();
    void catchUpTaskEvents();
  }, 5000);
  return () => {
    abort.abort();
    clearInterval(poll);
    clearTimeout(retry);
    wake?.();
  };
}
