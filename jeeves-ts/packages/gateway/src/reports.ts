import type { AgentTaskView, TaskUpdatedNotification } from "@jeeves/client";

/** Statuses after which a task has something to report; the rest are progress, kept off the group. */
const REPORTABLE = new Set(["done", "failed", "cancelled", "needs_attention"]);

/**
 * One line for the group, from a task that has just reached a reportable status.
 *
 * The group hears three kinds of thing only: the receipt, this report, and a confirmation card.
 * So progress never comes here, and the line stays short.
 */
export function reportLine(task: AgentTaskView): string {
  const why = task.lastStopReason === undefined || task.lastStopReason === null ? "" : ` (${task.lastStopReason})`;
  switch (task.status) {
    case "done":
      return `Done: ${task.title}`;
    case "needs_attention":
      return `Needs your call: ${task.title}${why}`;
    case "cancelled":
      return `Cancelled: ${task.title}`;
    default:
      return `Failed: ${task.title}${why}`;
  }
}

export interface ReportDeps {
  /** Load the task, to learn its status text and where it came from. */
  task(taskId: string): Promise<AgentTaskView>;
  /** Post into a chat as a reply to a message. */
  send(chatId: string, messageId: string, text: string): Promise<void>;
  onLog?: (text: string) => void;
}

/**
 * Returns the handler for `taskUpdated`: report a dispatched task back to its own group thread,
 * once per final status. Tasks without a Lark source (started from a local view) are not
 * touched; their report is the task card.
 */
export function taskReporter(deps: ReportDeps): (update: TaskUpdatedNotification) => void {
  const reported = new Set<string>();
  return (update) => {
    if (!REPORTABLE.has(update.status)) return;
    const key = `${update.taskId}:${update.status}`;
    if (reported.has(key)) return;
    reported.add(key);
    void (async () => {
      try {
        const task = await deps.task(update.taskId);
        const source = task.source;
        if (source === undefined || source === null || source.channel !== "lark") return;
        await deps.send(source.chatId, source.messageId, reportLine(task));
      } catch (error) {
        reported.delete(key);
        deps.onLog?.(`could not report task ${update.taskId}: ${error instanceof Error ? error.message : String(error)}`);
      }
    })();
  };
}
