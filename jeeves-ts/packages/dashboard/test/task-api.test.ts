import assert from "node:assert/strict";
import { test } from "node:test";
import { taskAction, taskOperations, reconcileTaskOperation, turnOperations, reconcileTurnOperation } from "../src/api.ts";
import { catchUpTaskEvents, taskTimelines } from "../src/tasks.ts";

test("a task refusal carried by HTTP 200 is still an error", async () => {
  const originalFetch = globalThis.fetch;
  const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: () => null },
  });
  globalThis.fetch = async () => new Response(JSON.stringify({
    code: -32602,
    message: "Task execution stopped: input deadline has expired",
  }), { status: 200, headers: { "content-type": "application/json" } });
  try {
    await assert.rejects(
      taskAction({ action: "start", taskId: "task", message: "Prepare the report" }),
      /input deadline has expired/,
    );
    await assert.rejects(taskOperations("task"), /input deadline has expired/);
    await assert.rejects(turnOperations({sessionId:"chat"}), /input deadline has expired/);
    await assert.rejects(reconcileTurnOperation({sessionId:"chat",turnId:"turn",operationId:"op",checkpointDigest:"digest",resolution:"not_executed",evidence:"Human checked"}), /input deadline has expired/);
    await assert.rejects(reconcileTaskOperation({ taskId: "task", operationId: "op",
      checkpointDigest: "digest", resolution: "not_executed", evidence: "Checked external state" }), /input deadline has expired/);
  } finally {
    globalThis.fetch = originalFetch;
    if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage);
    else Reflect.deleteProperty(globalThis, "localStorage");
  }
});

test("an immediate checkpoint refresh waits for the event poll and then pulls its new cursor", async () => {
  const originalFetch = globalThis.fetch;
  const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => null } });
  const cursors: string[] = [];
  let release!: (response: Response) => void;
  const event = (cursor: number, kind: string, detail: string | null) => ({ cursor, taskId: "checkpoint-fixture", runId: null, seq: cursor, kind, detail, at: "2026-11-19T08:00:00Z" });
  globalThis.fetch = async url => {
    const cursor = new URL(String(url), "http://localhost").searchParams.get("cursor")!;
    cursors.push(cursor);
    if (cursors.length === 1) return new Promise<Response>(resolve => { release = resolve; });
    return Response.json({ events: [event(2, "git_checkpoint", "fixture-commit")], nextCursor: 2, latestCursor: 2, hasMore: false });
  };
  try {
    const poll = catchUpTaskEvents();
    const immediate = catchUpTaskEvents();
    assert.deepEqual(cursors, ["0"]);
    release(Response.json({ events: [event(1, "run_finished", "completed")], nextCursor: 1, latestCursor: 1, hasMore: false }));
    await Promise.all([poll, immediate]);
    assert.deepEqual(cursors, ["0", "1"]);
    assert.deepEqual(taskTimelines["checkpoint-fixture"]!.map(event => event.kind), ["run_finished", "git_checkpoint"]);
  } finally {
    globalThis.fetch = originalFetch;
    if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage);
    else Reflect.deleteProperty(globalThis, "localStorage");
  }
});
