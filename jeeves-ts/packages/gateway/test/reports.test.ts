import assert from "node:assert/strict";
import test from "node:test";

import type { AgentTaskView } from "@jeeves/client";

import { reportLine, taskReporter } from "../src/reports.ts";

function task(over: Record<string, unknown>): AgentTaskView {
  return {
    title: "fix the site",
    status: "done",
    source: { channel: "lark", chatId: "oc_g", messageId: "om_1", senderId: "ou_owner" },
    ...over,
  } as unknown as AgentTaskView;
}

const tick = () => new Promise((resolve) => setImmediate(resolve));

test("a final status becomes one short line; the stop reason rides along on failures", () => {
  assert.equal(reportLine(task({})), "Done: fix the site");
  assert.equal(
    reportLine(task({ status: "failed", lastStopReason: "over budget" })),
    "Failed: fix the site (over budget)",
  );
  assert.equal(reportLine(task({ status: "cancelled" })), "Cancelled: fix the site");
});

test("a dispatched task reports once into its own thread; progress and local tasks stay quiet", async () => {
  const sent: Array<[string, string, string]> = [];
  const tasks: Record<string, AgentTaskView> = {
    t1: task({}),
    t2: task({ source: undefined }),
  };
  const handle = taskReporter({
    task: async (id) => tasks[id]!,
    send: async (chat, message, text) => void sent.push([chat, message, text]),
  });

  handle({ taskId: "t1", status: "running", roundsUsed: 1 } as never);
  await tick();
  assert.deepEqual(sent, [], "progress is not posted to the group");

  handle({ taskId: "t1", status: "done", roundsUsed: 1 } as never);
  handle({ taskId: "t1", status: "done", roundsUsed: 1 } as never);
  await tick();
  assert.deepEqual(sent, [["oc_g", "om_1", "Done: fix the site"]], "once, as a reply to the dispatching message");

  handle({ taskId: "t2", status: "done", roundsUsed: 1 } as never);
  await tick();
  assert.equal(sent.length, 1, "a task from a local view has no group to report to");
});

test("a failed lookup is logged and retried on the next update, not lost", async () => {
  const logs: string[] = [];
  const sent: string[] = [];
  let fail = true;
  const handle = taskReporter({
    task: async () => {
      if (fail) throw new Error("runtime busy");
      return task({});
    },
    send: async (_c, _m, text) => void sent.push(text),
    onLog: (text) => logs.push(text),
  });
  handle({ taskId: "t1", status: "done", roundsUsed: 1 } as never);
  await tick();
  assert.equal(logs.length, 1);
  fail = false;
  handle({ taskId: "t1", status: "done", roundsUsed: 1 } as never);
  await tick();
  assert.deepEqual(sent, ["Done: fix the site"]);
});
