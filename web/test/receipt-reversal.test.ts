import assert from "node:assert/strict";
import { test } from "node:test";
import { reversalHint } from "../src/reversal.ts";
import type { ToolOperationView } from "../src/protocol.ts";

function operation(partial: Partial<ToolOperationView>): ToolOperationView {
  return {
    id: "1",
    runId: null,
    turnId: "turn-1",
    roundId: "1",
    toolUseId: "call-1",
    tool: "write_file",
    arguments: {},
    inputDigest: "digest",
    origin: "checkpoint",
    status: "succeeded",
    replayPolicy: "never",
    outputPreview: null,
    outputDigest: null,
    evidence: null,
    startedAt: "2026-11-25T03:00:00Z",
    settledAt: "2026-11-25T03:00:01Z",
    ...partial,
  };
}

test("a file receipt suggests a confirmed next turn, never a hidden undo", () => {
  const hint = reversalHint(operation({ arguments: { path: "/home/u/notes.md" } }));
  assert.match(hint, /notes\.md/);
  assert.match(hint, /不会自动撤销/);
});

test("an external send cannot be reversed from the journal", () => {
  const hint = reversalHint(operation({ tool: "email_send" }));
  assert.match(hint, /接收端核对/);
});

test("an operation that never ran needs no reversal", () => {
  const hint = reversalHint(operation({ status: "not_executed", arguments: { path: "/x" } }));
  assert.match(hint, /没有产生需要撤销的效果/);
});

test("an unknown tool gets the honest generic answer", () => {
  const hint = reversalHint(operation({ tool: "mystery" }));
  assert.match(hint, /没有通用逆操作/);
});
