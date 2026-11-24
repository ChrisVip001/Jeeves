import { t } from "./i18n.ts";
import type { AgentTaskView } from "./protocol";
import type { TaskCenter } from "./ui/useTasks";

function awaitsGoalDecision(task: AgentTaskView): boolean {
  return Boolean(task.goalDraft) && task.status !== "failed" && task.status !== "cancelled";
}

/** One line on where a task stands, in the words the work list and its rail share. */
export function workStatus(center: TaskCenter, task: AgentTaskView): string {
  if (center.approvalFor(task)) return t("需要你确认");
  if (awaitsGoalDecision(task)) return t("完成标准待你确认");
  if (task.inputCondition?.state === "pending") return t("等{what}", { what: task.inputCondition.description });
  switch (task.status) {
    case "running": return t("正在执行");
    case "pending": return t("等待执行");
    case "suspended": return t("已暂停");
    case "needsAttention": case "needs_attention": return t("需要你处理");
    case "failed": return t("执行失败");
    case "cancelled": return t("已取消");
    case "done": return task.acceptance.length && task.checks.length === task.acceptance.length && task.checks.every(c => c.passed)
      ? t("验收通过") : t("已生成，待验收");
    default: return task.status;
  }
}

/** Tasks grouped by whether they need the person: waiting on you, working, finished. */
export function workSections(center: TaskCenter, query = "") {
  const waiting: AgentTaskView[] = [], working: AgentTaskView[] = [], past: AgentTaskView[] = [];
  for (const task of center.state.tasks) {
    if (query && !task.title.toLocaleLowerCase().includes(query)) continue;
    if (awaitsGoalDecision(task)) waiting.push(task);
    else if (["done", "failed", "cancelled"].includes(task.status)) past.push(task);
    else if (center.approvalFor(task) || ["needs_attention", "needsAttention"].includes(task.status)
      || (task.inputCondition?.state === "pending" && task.inputCondition.source === "manual")) waiting.push(task);
    else working.push(task);
  }
  return { waiting, working, past };
}

/** How many things wait on the person: tasks in the 等你决定 group, plus approvals not tied to a task. */
export function waitingCount(center: TaskCenter): number {
  const chatApprovals = Object.values(center.state.approvals).filter(approval => !center.taskOf(approval)).length;
  return workSections(center).waiting.length + chatApprovals;
}
