import assert from "node:assert/strict";
import { after, test } from "node:test";
import { approvalNoticeLink, useApprovalNotice } from "../src/approval-notice.ts";

const query = new URLSearchParams({
  deviceId: "fixture-public-key+/=", grantId: "grant", generation: "2",
  taskId: "task-a", turnId: "turn-a", approvalRequestId: "ask", approvalVersion: "3",
});
const locator = { ...Object.fromEntries(query), generation: 2, approvalVersion: 3 };
const sign = async (_: unknown, decision: boolean | null) => "signed-" + String(decision);
const pending = { status: "pending", approval: {
  task: { id: "task-a", title: "Fixture" },
  request: { requestId: "ask", turnId: "turn-a", version: 3, expiresInMs: 30000 },
} };
const fetchImpl = globalThis.fetch;
const storage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => null } });
after(() => {
  globalThis.fetch = fetchImpl;
  if (storage) Object.defineProperty(globalThis, "localStorage", storage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

test("opening a notice reads its exact device, generation and task request before offering a decision", async () => {
  const link = approvalNoticeLink(query);
  assert.deepEqual(link, locator);
  const notice = useApprovalNotice(link, sign);
  const calls: string[] = [];
  globalThis.fetch = async (url, init) => {
    calls.push(init?.method ?? "GET");
    if (calls.at(-1) === "GET") {
      const received = new URL(String(url), "http://fixture").searchParams;
      assert.equal(received.get("signature"), "signed-null");
      received.delete("signature");
      assert.deepEqual(received, query);
      return Response.json(pending);
    }
    assert.equal(url, "/api/approval/notice");
    assert.deepEqual(JSON.parse(String(init?.body)), { ...locator, approved: false, signature:"signed-false" });
    return Response.json({ accepted: true });
  };
  await notice.load();
  assert.deepEqual(calls, ["GET"]);
  assert.equal(notice.state.phase, "pending");
  await Promise.all([notice.answer(false), notice.answer(false)]);
  assert.deepEqual(calls, ["GET", "POST"]);
  assert.equal(notice.state.phase, "denied");
});

test("invalid and stale links never offer an action", async () => {
  const incomplete = new URLSearchParams(query);
  incomplete.delete("grantId");
  assert.equal(approvalNoticeLink(incomplete), null);
  for (const result of [{status:"expired"}, {status:"revoked"}, {
    ...pending, approval: {...pending.approval, request: {...pending.approval.request, version:4}}
  }]) {
    const notice = useApprovalNotice(approvalNoticeLink(query), sign);
    globalThis.fetch = async (_, init) => {
      assert.notEqual(init?.method, "POST");
      return Response.json(result);
    };
    await notice.load();
    assert.equal(notice.state.phase, result.status === "revoked" ? "revoked" : "expired");
    assert.equal(notice.state.approval, null);
    await notice.answer(true);
  }
});

test("uncertain submission is never retried; checking again is read-only", async () => {
  const notice = useApprovalNotice(approvalNoticeLink(query), sign);
  let posts = 0;
  globalThis.fetch = async (_, init) => {
    if (init?.method === "POST") {posts++; throw new TypeError("offline");}
    return Response.json(posts ? {status:"expired"} : pending);
  };
  await notice.load();
  await notice.answer(true);
  assert.equal(notice.state.phase, "unknown");
  await notice.answer(true);
  await notice.load();
  assert.equal(posts, 1);
  assert.equal(notice.state.phase, "expired");
});

test("going offline hides a pending read even if its response arrives late", async () => {
  const notice = useApprovalNotice(approvalNoticeLink(query), sign);
  let finish!: (r: Response) => void;
  globalThis.fetch = async () => new Promise(resolve => { finish = resolve; });
  const loading = notice.load();
  await Promise.resolve();
  notice.disconnected();
  finish(Response.json(pending));
  await loading;
  assert.equal(notice.state.phase, "offline");
  assert.equal(notice.state.approval, null);
  await notice.answer(true);
  assert.equal(notice.state.phase, "offline");
});

test("a revoked device at decision time refreshes its grant without replaying the decision", async () => {
  const notice = useApprovalNotice(approvalNoticeLink(query), sign);
  let posts = 0;
  globalThis.fetch = async (_, init) => {
    if (init?.method === "POST") { posts++; return Response.json({ accepted: false }); }
    return Response.json(posts ? { status: "revoked" } : pending);
  };
  await notice.load();
  await notice.answer(false);
  assert.equal(posts, 1);
  assert.equal(notice.state.approval, null);
  assert.equal(notice.state.phase, "revoked");
});


test("a copied link on another device cannot read or answer its approval", async () => {
  const notice = useApprovalNotice(approvalNoticeLink(query), async () => { throw Error("revoked"); });
  globalThis.fetch = async () => { throw new Error("No request should leave this device"); };
  await notice.load();
  assert.equal(notice.state.phase, "revoked");
  assert.equal(notice.state.approval, null);
  await notice.answer(true);
  assert.equal(notice.state.phase, "revoked");
});
