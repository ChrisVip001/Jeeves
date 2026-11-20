import assert from "node:assert/strict";
import { test } from "node:test";
import { submissionTracker } from "../src/ui/submission.ts";
test("ambiguous submissions retain identity after reload, completed work gets a new identity", async () => {
  const records = new Map<string, string>();
  const store = {
    getItem: (key: string) => records.get(key) ?? null,
    setItem: (key: string, value: string) => {
      records.set(key, value);
    },
    removeItem: (key: string) => {
      records.delete(key);
    },
  };
  const input = {
    sessionId: "main",
    message: "private task text",
    taskId: null,
    cwd: null,
    attachments: null,
  };
  const first = await submissionTracker(store).prepare(input);
  const reload = submissionTracker(store);
  assert.equal((await reload.prepare(input)).messageId, first.messageId);
  assert.ok(![...records.values()].join("").includes(input.message));
  reload.complete(first);
  assert.notEqual((await reload.prepare(input)).messageId, first.messageId);
  assert.notEqual(
    (await reload.prepare({ ...input, cwd: "/project" })).messageId,
    first.messageId,
  );
});
