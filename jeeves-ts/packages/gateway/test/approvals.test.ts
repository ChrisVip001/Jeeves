import assert from "node:assert/strict";
import test from "node:test";

import type { AgentTaskView, ApprovalRequestedNotification } from "@jeeves/client";

import { answerLine, approvalActionOf, approvalCard, approvalRelay } from "../src/approvals.ts";
import type { ApprovalOrigin } from "../src/approvals.ts";

const request: ApprovalRequestedNotification = {
  turnId: "t1",
  requestId: "ap-1",
  tool: "run_command",
  argsPreview: "git push origin main",
  reason: "not on the allow list",
  expiresInMs: 120_000,
  version: 1,
};
const lark = { channel: "lark", chatId: "oc_g", messageId: "om_1", senderId: "ou_owner" };
const tick = () => new Promise((resolve) => setImmediate(resolve));

test("the card says what, why, which task and when silence means no; buttons carry the ids", () => {
  const card = approvalCard(request, { title: "ship the site" } as AgentTaskView);
  const text = JSON.stringify(card);
  for (const part of ["ship the site", "run_command", "git push origin main", "not on the allow list", "2 min"]) {
    assert.ok(text.includes(part), `the card should mention ${part}`);
  }
  const actions = (card["elements"] as Array<Record<string, unknown>>)[1]!["actions"] as Array<{ value: unknown }>;
  assert.deepEqual(actions.map((a) => approvalActionOf(a.value)), [
    { jeeves: "approval", turnId: "t1", requestId: "ap-1", approved: true },
    { jeeves: "approval", turnId: "t1", requestId: "ap-1", approved: false },
  ]);
});

test("a button value that isn't ours is not read as an answer", () => {
  assert.equal(approvalActionOf(null), null);
  assert.equal(approvalActionOf({ jeeves: "approval", turnId: "t1", requestId: "ap-1" }), null);
  assert.equal(approvalActionOf({ jeeves: "other", turnId: "t1", requestId: "ap-1", approved: true }), null);
});

test("a request from a group-dispatched task is asked in that thread, once", async () => {
  const cards: Array<[string, string]> = [];
  const relay = approvalRelay({
    locate: async () => ({ source: lark, task: null }),
    sendCard: async (chatId, messageId) => void cards.push([chatId, messageId]),
  });
  relay(request);
  relay(request);
  await tick();
  await tick();
  assert.deepEqual(cards, [["oc_g", "om_1"]]);
});

test("local requests and requests no longer waiting are left alone", async () => {
  const origins: Array<ApprovalOrigin | null> = [null, { source: null, task: null }];
  let sent = 0;
  const relay = approvalRelay({
    locate: async () => origins.shift() ?? null,
    sendCard: async () => void (sent += 1),
  });
  relay({ ...request, requestId: "a" });
  relay({ ...request, requestId: "b" });
  await tick();
  await tick();
  assert.equal(sent, 0);
});

test("a private origin uses the text approval sender once per revision", async () => {
  const deliveries: number[] = [];
  const relay = approvalRelay({
    locate: async () => ({ source: lark, task: null, replyWithText: true }),
    sendCard: async () => assert.fail("private approval used the group card sender"),
    sendTextApproval: async (source, approval) => {
      assert.deepEqual(source, lark);
      deliveries.push(approval.version);
    },
  });
  relay(request);
  relay(request);
  relay({ ...request, version: 2 });
  await tick();
  assert.deepEqual(deliveries, [1, 2]);
});

test("a failed lookup is logged, not thrown", async () => {
  const logs: string[] = [];
  const relay = approvalRelay({
    locate: async () => {
      throw new Error("gone");
    },
    sendCard: async () => {},
    onLog: (text) => logs.push(text),
  });
  relay(request);
  await tick();
  await tick();
  assert.match(logs[0] ?? "", /ap-1.*gone/);
});

test("the line under the card says who answered, or that it came too late", () => {
  const yes = { jeeves: "approval", turnId: "t1", requestId: "ap-1", approved: true } as const;
  assert.equal(answerLine(yes, true, "Ann"), "Approved once by Ann.");
  assert.equal(answerLine({ ...yes, approved: false }, true, "Ann"), "Denied by Ann.");
  assert.match(answerLine(yes, false, "Ann"), /already answered or has expired/);
});

test("answerLine names who answered first when the Runtime says so", () => {
  const yes = { jeeves: "approval", turnId: "t", requestId: "r", approved: true } as const;
  assert.equal(
    answerLine(yes, { accepted: false, answeredBy: "telegram:7" }, "Ann"),
    "Already answered by telegram:7; nothing changed.",
  );
  assert.match(answerLine(yes, { accepted: false }, "Ann"), /already answered or has expired/);
});
