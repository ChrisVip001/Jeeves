import assert from "node:assert/strict";
import { test } from "node:test";
import { queuedTasks } from "../src/task-queued.ts";

test("queued task lines become cards; other output does not", () => {
  const output = "• 调研空气净化器 (task1732400000000000-3): queued\n• Weekly (brief) (task17-4): queued\nPassed the follow-up to \"x\".";
  assert.deepEqual(queuedTasks(output), [
    { title: "调研空气净化器", id: "task1732400000000000-3" },
    { title: "Weekly (brief)", id: "task17-4" },
  ]);
  assert.deepEqual(queuedTasks("Cancelled \"x\"."), []);
  assert.deepEqual(queuedTasks("• x (task1-2): failed"), []);
});
