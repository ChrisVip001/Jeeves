/** Date, progress and timeline helpers for tasks and approvals. */
import type { AgentTaskView, TaskEventView } from "./types";

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
