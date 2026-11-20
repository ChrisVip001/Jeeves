import assert from "node:assert/strict";
import { test } from "node:test";
import { useTasks } from "../src/ui/useTasks.ts";
import type { AgentTaskView } from "../src/ui/types.ts";

test("task approvals associate after a missed turn-start and policy changes clear them", async () => {
  const task = {
    id: "alpha",
    sessionId: "main",
    executionSessionId: "worker",
    currentTurnId: "run",
    status: "running",
  } as AgentTaskView;
  let calls = 0;
  const center = useTasks({
    list: async () => ({ tasks: [task] }),
    pending: async () => ({ approvals: [] }),
    messages: async () => ({ data: [], nextCursor: null }),
    artifacts: async () => ({ artifacts: [] }),
    action: async () => ({ task }),
    approve: async () => ({ accepted: ++calls === 1 }),
  });
  center.state.owner = "main";
  await center.refresh();
  const approval = {
    type: "approvalRequested",
    turnId: "run",
    requestId: "ap",
    tool: "write_file",
    argsPreview: "report.txt",
    reason: "Confirm",
    expiresInMs: 120000,
  } as const;
  center.onNotification(approval);
  assert.equal(center.approvalFor(task)?.requestId, "ap");
  await center.approve("run", true);
  assert.equal(calls, 1);
  assert.equal(center.approvalFor(task), undefined);
  center.onNotification(approval);
  center.onNotification({
    type: "permissionChanged",
    sessionId: "main",
    mode: "readOnly",
    revision: 2,
    execEnabled: false,
    writeEnabled: true,
    delegateEnabled: false,
  });
  assert.equal(center.approvalFor(task), undefined);
});

test("a task update arriving during refresh triggers a repair read", async () => {
  let release: (page: { tasks: AgentTaskView[] }) => void = () => {};
  let calls = 0;
  const task = { id: "alpha", sessionId: "main" } as AgentTaskView;
  const center = useTasks({
    list: () =>
      ++calls === 1
        ? new Promise((resolve) => (release = resolve))
        : Promise.resolve({ tasks: [task] }),
    pending: async () => ({ approvals: [] }),
    messages: async () => ({ data: [], nextCursor: null }),
    artifacts: async () => ({ artifacts: [] }),
    action: async () => ({ task }),
    approve: async () => ({ accepted: true }),
  });
  center.state.owner = "main";
  const first = center.refresh();
  center.onNotification({
    type: "taskUpdated",
    taskId: "alpha",
    sessionId: "main",
    status: "pending",
    roundsUsed: 0,
    reason: "queued",
  });
  release({ tasks: [] });
  await first;
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(calls, 2);
  assert.equal(center.state.tasks[0]?.id, "alpha");
});

test("reopening the view restores only live owner approvals and merges their task", async () => {
  const task = {
    id: "alpha",
    sessionId: "main",
    executionSessionId: "worker",
    currentTurnId: "run",
    status: "running",
  } as AgentTaskView;
  const request = {
    turnId: "run",
    requestId: "pending",
    tool: "write_file",
    argsPreview: "report.txt",
    reason: "Confirm",
    expiresInMs: 50000,
  };
  let pending = [
    { sessionId: "worker", ownerSessionId: "main", task, request },
  ];
  const center = useTasks({
    list: async () => ({ tasks: [] }),
    pending: async () => ({ approvals: pending }),
    messages: async () => ({ data: [], nextCursor: null }),
    artifacts: async () => ({ artifacts: [] }),
    action: async () => ({ task }),
    approve: async () => ({ accepted: true }),
  });
  center.state.owner = "main";
  await center.refresh();
  assert.equal(center.approvalFor(task)?.requestId, "pending");
  assert.equal(center.state.tasks[0]?.id, "alpha");
  assert.equal(center.state.open, true);
  pending = [];
  await center.refresh();
  assert.equal(center.approvalFor(task), undefined);
});

test("an old snapshot cannot restore a request answered while the refresh was in flight", async () => {
  const task = {
    id: "alpha",
    sessionId: "main",
    executionSessionId: "worker",
    currentTurnId: "run",
    status: "running",
  } as AgentTaskView;
  const entry = {
    sessionId: "worker",
    ownerSessionId: "main",
    task,
    request: {
      turnId: "run",
      requestId: "ap",
      tool: "write_file",
      argsPreview: "report.txt",
      reason: "Confirm",
      expiresInMs: 50000,
    },
  };
  let calls = 0;
  let release: (value: { approvals: (typeof entry)[] }) => void = () => {};
  const center = useTasks({
    list: async () => ({ tasks: [task] }),
    pending: () =>
      ++calls === 1
        ? Promise.resolve({ approvals: [entry] })
        : calls === 2
          ? new Promise((resolve) => (release = resolve))
          : Promise.resolve({ approvals: [] }),
    messages: async () => ({ data: [], nextCursor: null }),
    artifacts: async () => ({ artifacts: [] }),
    action: async () => ({ task }),
    approve: async () => ({ accepted: true }),
  });
  center.state.owner = "main";
  await center.refresh();
  const stale = center.refresh();
  await center.approve("run", true);
  release({ approvals: [entry] });
  await stale;
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(center.approvalFor(task), undefined);
  assert.equal(calls, 3);
});

test("a foreign conversation's approval never opens this task panel", async () => {
  const unstarted = {
    id: "unstarted",
    sessionId: "main",
    currentTurnId: null,
  } as AgentTaskView;
  const center = useTasks({
    list: async () => ({ tasks: [unstarted] }),
    pending: async () => ({
      approvals: [
        {
          sessionId: "other",
          ownerSessionId: "other",
          task: null,
          request: {
            turnId: "foreign",
            requestId: "ap",
            tool: "write_file",
            argsPreview: "private",
            reason: "Confirm",
            expiresInMs: 50000,
          },
        },
      ],
    }),
    messages: async () => ({ data: [], nextCursor: null }),
    artifacts: async () => ({ artifacts: [] }),
    action: async () => {
      throw Error("unused");
    },
    approve: async () => ({ accepted: false }),
  });
  center.state.owner = "main";
  await center.refresh();
  assert.deepEqual(Object.keys(center.state.approvals), []);
  assert.equal(center.state.open, false);
  assert.equal(center.approvalFor(unstarted), undefined);
  center.onNotification({
    type: "approvalRequested",
    turnId: "foreign",
    requestId: "ap",
    tool: "write_file",
    argsPreview: "private",
    reason: "Confirm",
    expiresInMs: 50000,
  });
  await center.refresh();
  assert.equal(center.state.open, false);
  assert.equal(center.approvalFor(unstarted), undefined);
});

test("answering echoes the approval version and a late answer names who answered first", async () => {
  const task = {
    id: "alpha",
    sessionId: "main",
    executionSessionId: "worker",
    currentTurnId: "run",
    status: "running",
  } as AgentTaskView;
  const sent: unknown[] = [];
  const center = useTasks({
    list: async () => ({ tasks: [task] }),
    pending: async () => ({ approvals: [] }),
    messages: async () => ({ data: [], nextCursor: null }),
    artifacts: async () => ({ artifacts: [] }),
    action: async () => ({ task }),
    approve: async (params: unknown) => {
      sent.push(params);
      return { accepted: false, answeredBy: "telegram" };
    },
  });
  center.state.owner = "main";
  await center.refresh();
  center.onNotification({
    type: "approvalRequested",
    turnId: "run",
    requestId: "ap",
    tool: "write_file",
    argsPreview: "report.txt",
    reason: "Confirm",
    expiresInMs: 120000,
    version: 3,
  } as const);
  await center.approve("run", true);
  assert.equal((sent[0] as { version?: number }).version, 3);
  assert.deepEqual(center.state.late, { answeredBy: "telegram" });
  assert.equal(center.state.error, "");
});
