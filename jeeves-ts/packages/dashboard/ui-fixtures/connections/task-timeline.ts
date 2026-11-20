import { createApp, h, ref } from "vue";
import TaskDetail from "../../src/ui/TaskDetail.vue";
import TaskRow from "../../src/ui/TaskRow.vue";
import { uiTranslatorKey } from "../../src/ui/translate.ts";
import { useTasks, type TaskActions } from "../../src/ui/useTasks.ts";
import { initLang, setLang, t } from "../../src/i18n.ts";
import type { AgentTaskView, TaskEventView, TaskGitCheckpointParams, TaskGitCheckpointPolicyParams, TaskGitCheckpointPolicyResponse } from "../../src/ui/types.ts";
import "../../src/ui/style.css";
import "../../src/style.css";
import "../../src/ui/today.css";

const task: AgentTaskView = {
  id: "fixture", sessionId: "fixture-owner", title: "Fixture task", goalCondition: null,
  status: "running", roundsUsed: 0, maxRounds: 0, tier: null, currentTurnId: null,
  suspendReason: null, lastStopReason: null, acceptance: [], checks: [], budget: null,
  spent: { wallMs: 0, tokens: 0, tools: 0 }, cwd: null, runCount: 2,
  createdAt: "2026-11-19T08:00:00Z", finishedAt: null,
};
const checkpoint = new URLSearchParams(location.search).get("checkpoint");
if (checkpoint) {
  task.worktreePath = "/fixture/worktrees/fixture";
  task.branch = "jeeves/fixture";
  task.cwd = task.worktreePath;
  if (checkpoint === "idle") task.status = "pending";
  if (checkpoint === "queued") { task.status = "pending"; task.dispatchState = "pending"; }
}
const event = (seq: number, kind: string, detail?: string): TaskEventView => ({ cursor: seq, taskId: task.id, runId: null, seq, kind, detail: detail ?? null, at: `2026-11-19T08:0${seq}:00Z` });
const events = ref([
  event(1, "run_started"),
  event(2, "approval_requested", "email_send · ap-fixture"),
  event(3, "approval_decided", "email_send · denied · fixture-person"),
  event(4, "approval_decided", "email_send · timeout · "),
  event(5, "run_finished", "completed"),
  event(6, "run_started"),
]);
const neverCalled = async () => { throw new Error("Fixture only; no task action should be called"); };
const fixtureCheckpoints: TaskGitCheckpointParams[] = [];
const fixturePolicyWrites: TaskGitCheckpointPolicyParams[] = [];
let fixturePolicy: TaskGitCheckpointPolicyResponse = { taskId: task.id, enabled: false, paths: [] };
Object.assign(window, { fixtureCheckpoints, fixturePolicyWrites });
const actions: TaskActions = { list: neverCalled, pending: neverCalled, messages: neverCalled, artifacts: neverCalled, action: neverCalled, approve: neverCalled,
  gitCheckpoint: async params => {
    fixtureCheckpoints.push(structuredClone(params));
    if (params.paths.includes("blocked.txt")) throw new Error("Fixture refused the file");
    const sha = "1234567890abcdef1234567890abcdef12345678";
    events.value.push(event(7, "git_checkpoint", sha));
    return { taskId: task.id, branch: "jeeves/fixture", commit: sha };
  },
  gitCheckpointPolicy: async () => {
    if (new URLSearchParams(location.search).has("failPolicy")) throw new Error("Fixture read failed");
    return fixturePolicy;
  },
  saveGitCheckpointPolicy: async params => {
    fixturePolicyWrites.push(structuredClone(params));
    if (params.paths.includes("blocked.txt")) throw new Error("Fixture refused policy");
    fixturePolicy = { taskId: params.taskId, enabled: params.enabled, paths: [...params.paths] };
    return fixturePolicy;
  },
};
const lang = new URLSearchParams(location.search).get("lang") === "en" ? "en" : "zh";
if (initLang() !== lang) setLang(lang);
const app = createApp({ setup() {
  const open = ref(false);
  const center = useTasks(actions);
  return () => h("main", { class: "today" }, [
    h("div", { class: "today-page" }, [h(TaskRow, { task, events: events.value, onOpen: () => { open.value = true; } })]),
    open.value ? h(TaskDetail, { task, events: events.value, center, onClose: () => { open.value = false; } }) : null,
  ]);
} });
app.provide(uiTranslatorKey, t);
app.mount("#app");
