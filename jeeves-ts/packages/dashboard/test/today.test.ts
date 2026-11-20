import assert from "node:assert/strict";
import { test } from "node:test";
import { buildToday, dayPart, minutesLeft, parseAt, timelineOf } from "../src/ui/today.ts";
import type { AgentTaskView, ApprovalRequestedNotification, TaskEventView } from "../src/ui/types.ts";
import { taskEventText } from "../src/task-event-text.ts";
import { UI_EN } from "../src/ui/en.ts";

/** SQLite `datetime('now')`: UTC, space-separated, no zone suffix. */
const sqlite = (date: Date) => date.toISOString().slice(0, 19).replace("T", " ");
const task = (fields: Partial<AgentTaskView>) =>
  ({ acceptance: [], checks: [], roundsUsed: 0, maxRounds: 0, finishedAt: null, ...fields }) as AgentTaskView;

test("SQLite timestamps are read as UTC", () => {
  assert.equal(parseAt("2026-11-18 09:30:00")?.toISOString(), "2026-11-18T09:30:00.000Z");
  assert.equal(parseAt(null), null);
  assert.equal(parseAt("not a date"), null);
});

test("the greeting follows the local hour", () => {
  const at = (hour: number) => new Date(2026, 10, 18, hour, 0);
  assert.equal(dayPart(at(3)), "night");
  assert.equal(dayPart(at(8)), "morning");
  assert.equal(dayPart(at(12)), "noon");
  assert.equal(dayPart(at(15)), "afternoon");
  assert.equal(dayPart(at(20)), "evening");
  assert.equal(dayPart(at(23)), "night");
});

test("the board puts what needs you first and never repeats an asked task under in progress", () => {
  const now = new Date(2026, 10, 18, 15, 0);
  const earlier = sqlite(new Date(2026, 10, 18, 9, 0));
  const later = sqlite(new Date(2026, 10, 18, 11, 0));
  const yesterday = sqlite(new Date(2026, 10, 17, 11, 0));
  const tasks = [
    task({ id: "asked", status: "running", currentTurnId: "t1" }),
    task({ id: "moving", status: "running" }),
    task({ id: "stuck", status: "needsAttention" }),
    task({ id: "failed", status: "failed", finishedAt: earlier }),
    task({ id: "old-failure", status: "failed", finishedAt: yesterday }),
    task({ id: "done-early", status: "done", finishedAt: earlier }),
    task({ id: "done-late", status: "done", finishedAt: later }),
    task({ id: "done-yesterday", status: "done", finishedAt: yesterday }),
  ];
  const approvals = [
    { turnId: "t2", requestId: "b", expiresInMs: 90000 },
    { turnId: "t1", requestId: "a", expiresInMs: 30000 },
  ] as ApprovalRequestedNotification[];
  const model = buildToday(tasks, approvals, (a) => tasks.find((t) => t.currentTurnId === a.turnId), now);
  assert.deepEqual(
    model.needs.map((item) => item.approval?.requestId ?? item.task?.id),
    ["failed", "a", "b", "stuck"],
  );
  assert.deepEqual(model.moving.map((t) => t.id), ["moving"]);
  assert.deepEqual(model.finished.map((t) => t.id), ["done-late", "done-early"]);
  assert.equal(model.mark, "waiting");
});

test("the mark works while something runs and folds in when nothing is left", () => {
  const now = new Date(2026, 10, 18, 15, 0);
  const none = () => undefined;
  assert.equal(buildToday([task({ id: "x", status: "pending" })], [], none, now).mark, "working");
  const quiet = buildToday([], [], none, now);
  assert.equal(quiet.mark, "done");
  assert.deepEqual([quiet.needs, quiet.moving, quiet.finished], [[], [], []]);
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
