/**
 * The Today page as data: which tasks need the human, which are moving, what finished today,
 * and what the mark should say. Pure functions, no strings — the components own the wording
 * (every word goes through `t()` in a .vue file), so this stays testable without a DOM.
 */
import type {
  AgentTaskView,
  ApprovalRequestedNotification,
  TaskEventView,
} from "./types";

export type DayPart = "night" | "morning" | "noon" | "afternoon" | "evening";
export type MarkState = "working" | "waiting" | "done";

const MOVING = new Set(["pending", "running", "suspended"]);
const ATTENTION = new Set(["needs_attention", "needsAttention"]);
const FINISHED = new Set(["done", "failed", "cancelled"]);

/** SQLite's `datetime('now')` is UTC with no zone marker; everything else is ISO already. */
export function parseAt(value: string | null | undefined): Date | null {
  if (!value) return null;
  const plain = /^\d{4}-\d\d-\d\d \d\d:\d\d(:\d\d)?$/.test(value);
  const date = new Date(plain ? `${value.replace(" ", "T")}Z` : value);
  return Number.isNaN(date.getTime()) ? null : date;
}

export function sameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

export function dayPart(now: Date): DayPart {
  const hour = now.getHours();
  if (hour < 5 || hour >= 23) return "night";
  if (hour < 11) return "morning";
  if (hour < 13) return "noon";
  if (hour < 18) return "afternoon";
  return "evening";
}

export interface NeedItem {
  kind: "approval" | "attention" | "failed";
  task?: AgentTaskView;
  approval?: ApprovalRequestedNotification;
}

export interface TodayModel {
  needs: NeedItem[];
  moving: AgentTaskView[];
  finished: AgentTaskView[];
  mark: MarkState;
}

/**
 * Sort the board. An approval names its task, so that task is not repeated under "In progress";
 * a failure from today is bad news and goes first, older failures are history.
 */
export function buildToday(
  tasks: AgentTaskView[],
  approvals: ApprovalRequestedNotification[],
  taskOf: (approval: ApprovalRequestedNotification) => AgentTaskView | undefined,
  now: Date,
): TodayModel {
  const needs: NeedItem[] = [];
  const asked = new Set<string>();
  for (const approval of [...approvals].sort((a, b) => a.expiresInMs - b.expiresInMs)) {
    const task = taskOf(approval);
    if (task) asked.add(task.id);
    needs.push({ kind: "approval", approval, task });
  }
  const finishedToday = (task: AgentTaskView) => {
    const at = parseAt(task.finishedAt);
    return FINISHED.has(task.status) && at !== null && sameDay(at, now);
  };
  for (const task of tasks) {
    if (asked.has(task.id)) continue;
    if (ATTENTION.has(task.status)) needs.push({ kind: "attention", task });
    else if (task.status === "failed" && finishedToday(task))
      needs.unshift({ kind: "failed", task });
  }
  const moving = tasks.filter((task) => MOVING.has(task.status) && !asked.has(task.id));
  const finished = tasks
    .filter((task) => task.status !== "failed" && finishedToday(task))
    .sort((a, b) => (b.finishedAt ?? "").localeCompare(a.finishedAt ?? ""));
  const running = tasks.some((task) => task.status === "running" || task.status === "pending");
  return {
    needs,
    moving,
    finished,
    mark: needs.length ? "waiting" : running ? "working" : "done",
  };
}

export interface Progress {
  checks?: { passed: number; total: number };
  rounds?: { used: number; max: number };
}

/** "Where is it": the measured part first (checks), then effort spent (rounds). */
export function progressOf(task: AgentTaskView): Progress {
  const progress: Progress = {};
  if (task.acceptance.length)
    progress.checks = {
      passed: task.checks.filter((check) => check.passed).length,
      total: task.acceptance.length,
    };
  if (task.maxRounds > 0 && task.roundsUsed > 0)
    progress.rounds = { used: task.roundsUsed, max: task.maxRounds };
  return progress;
}

/** Timeline in task order. Keyed by cursor, so a reconnect only ever appends. */
export function timelineOf(events: TaskEventView[] | undefined): TaskEventView[] {
  const seen = new Set<number>();
  return (events ?? [])
    .filter((event) => !seen.has(event.cursor) && seen.add(event.cursor))
    .sort((a, b) => a.seq - b.seq);
}

/** The newest event, which is what the card quotes as its evidence. */
export function latestEvent(events: TaskEventView[] | undefined): TaskEventView | undefined {
  return timelineOf(events).at(-1);
}

/** Whole minutes left, never negative; `null` once it has expired. */
export function minutesLeft(deadline: number, now: number): number | null {
  const ms = deadline - now;
  return ms <= 0 ? null : Math.ceil(ms / 60000);
}
