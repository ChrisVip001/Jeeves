import assert from "node:assert/strict";
import { test } from "node:test";
import { minutesLeft, parseAt, timelineOf } from "../src/ui/today.ts";
import type { TaskEventView } from "../src/ui/types.ts";
import { taskEventText } from "../src/task-event-text.ts";
import { UI_EN } from "../src/ui/en.ts";

test("SQLite timestamps are read as UTC", () => {
  assert.equal(parseAt("2026-11-18 09:30:00")?.toISOString(), "2026-11-18T09:30:00.000Z");
  assert.equal(parseAt(null), null);
  assert.equal(parseAt("not a date"), null);
});

test("the timeline dedupes by cursor and keeps task order", () => {
  const event = (cursor: number, seq: number) => ({ cursor, seq, kind: "verify" }) as TaskEventView;
  assert.deepEqual(
    timelineOf([event(3, 2), event(1, 1), event(3, 2), event(5, 3)]).map((e) => e.cursor),
    [1, 3, 5],
  );
  assert.deepEqual(timelineOf(undefined), []);
});

test("task event text preserves evidence and distinguishes confirmed approval from execution success", () => {
  const event = (kind: string, detail?: string) => ({ cursor: 1, taskId: "task", seq: 1, kind, detail, at: "" }) satisfies TaskEventView;
  const en = (key: string) => UI_EN[key] ?? key;
  assert.equal(taskEventText(event("approval_decided", "tool · approved · actor · alias"), en).detail, "tool · Approved · actor · alias");
  assert.equal(taskEventText(event("approval_decided", "tool · voided · "), en).detail, "tool · Approval voided");
  assert.equal(taskEventText(event("run_finished", "waiting"), en).detail, "Waiting for human action");
  assert.equal(taskEventText(event("run_finished", "completed"), en).label, "Run ended");
  assert.deepEqual(taskEventText(event("new_event", "old evidence"), en), { label: "new_event", detail: "old evidence", known: false });
});

test("an approval's minutes round up and expire to null", () => {
  assert.equal(minutesLeft(90_000, 0), 2);
  assert.equal(minutesLeft(60_000, 0), 1);
  assert.equal(minutesLeft(0, 0), null);
  assert.equal(minutesLeft(0, 5), null);
});
