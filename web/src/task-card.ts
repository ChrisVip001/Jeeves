import { watchEffect } from "vue";
import { t } from "./i18n";
import { el } from "./render";
import type { TaskCenter } from "./ui/useTasks";
import { workStatus } from "./work-status";
import type { QueuedTask } from "./task-queued";

/** The dot's tone: the same four the 工作 list uses. */
function tone(status: string): string {
  if (status === "running") return "live";
  if (status === "failed") return "bad";
  if (status === "done" || status === "cancelled") return "done";
  return "idle";
}

/**
 * Cards for the tasks a turn started, each with its live status, opening the task in 工作 on click.
 * Title and status follow the task center until the card leaves the page; a card redrawn from
 * history knows only the id, so the title arrives with the task list.
 */
export function taskCards(tasks: QueuedTask[], center: TaskCenter, open: (id: string) => Promise<boolean>): HTMLElement | null {
  if (!tasks.length) return null;
  const list = el("div", { class: "chat-task-cards" });
  for (const queued of tasks) {
    const dot = el("i", { class: "work-dot idle", "aria-hidden": "true" });
    const title = el("strong", { text: queued.title || t("正在读取任务…") });
    const status = el("small", { class: "chat-task-status", text: t("等待执行") });
    const note = el("span", { class: "chat-task-note", role: "status" });
    const card = el("button", { type: "button", class: "chat-task-card" },
      dot, el("span", { class: "chat-task-text" }, title, status), el("span", { class: "chat-task-go", text: t("查看工作") }));
    card.dataset.taskId = queued.id;
    card.addEventListener("click", async () => {
      note.textContent = await open(queued.id) ? "" : t("暂时无法打开原任务，请重试。");
    });
    list.append(card, note);
    let seen = false;
    const stop = watchEffect(() => {
      const task = center.state.tasks.find(row => row.id === queued.id);
      if (card.isConnected) seen = true;
      else if (seen) { queueMicrotask(() => stop()); return; }
      if (!task) return;
      title.textContent = task.title;
      status.textContent = workStatus(center, task);
      dot.className = `work-dot ${tone(task.status)}`;
    });
  }
  return list;
}
