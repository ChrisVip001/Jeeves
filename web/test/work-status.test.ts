import assert from "node:assert/strict";
import { test } from "node:test";
import type { AgentTaskView, ApprovalRequestedNotification } from "../src/protocol.ts";
import { useTasks } from "../src/ui/useTasks.ts";
import { waitingCount, workSections, workStatus } from "../src/work-status.ts";

test("an owner's unconfirmed goal keeps its completed task in decisions and counts once", async () => {
  const owner = "main";
  const draft = {
    id: "brief", title: "Weekly brief", sessionId: owner, status: "done",
    goalDraft: "Cite every source", currentTurnId: "turn", acceptance: [], checks: [],
  } as AgentTaskView;
  const completed = { ...draft, id: "other", currentTurnId: null, goalDraft: null };
  const unrelated = { ...draft, id: "not-mine", sessionId: "another-owner" };
  let tasks = [draft, completed, unrelated];
  const center = useTasks({
    list: async () => ({ tasks }),
    pending: async () => ({ approvals: [] }),
    messages: async () => ({ data: [], nextCursor: null }),
    artifacts: async () => ({ artifacts: [] }),
    action: async () => { throw Error("Grouping must not change tasks"); },
    approve: async () => { throw Error("Grouping must not approve anything"); },
  });
  center.state.owner = owner;
  await center.refresh();

  assert.deepEqual(workSections(center).waiting.map(task => task.id), [draft.id]);
  assert.deepEqual(workSections(center).past.map(task => task.id), [completed.id]);
  assert.equal(workStatus(center, draft), "完成标准待你确认");
  assert.equal(waitingCount(center), 1);

  center.state.approvals.turn = { turnId: "turn", requestId: "fixture", version: 1 } as ApprovalRequestedNotification;
  assert.equal(waitingCount(center), 1, "an additional decision does not count the same task twice");
  assert.equal(workStatus(center, draft), "需要你确认", "tool approval retains priority");

  tasks = [{ ...draft, goalDraft: null }, completed, unrelated];
  await center.refresh();
  assert.equal(waitingCount(center), 0);
  assert.deepEqual(workSections(center).past.map(task => task.id), [draft.id, completed.id]);
});
