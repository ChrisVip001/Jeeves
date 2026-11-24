import { watchEffect } from "vue";
import { t } from "./i18n";
import { el } from "./render";
import type { TaskCenter } from "./ui/useTasks";
import { workStatus } from "./work-status";
import type { QueuedTask } from "./task-queued";

/**
 * Cards for the tasks a turn started, each with its live status and a way into work mode.
 * The status follows the task center until the card leaves the page.
 */
export function taskCards(tasks: QueuedTask[], center: TaskCenter, open: (id: string) => Promise<boolean>): HTMLElement | null {
  if (!tasks.length) return null;
  const list = el("div", { class: "chat-task-cards" });
  for (const queued of tasks) {
    const status = el("small", { class: "chat-task-status", text: t("等待执行") });
    const note = el("span", { role: "status" });
    const button = el("button", { type: "button", class: "today-btn", text: t("查看工作") });
    button.addEventListener("click", async () => {
      note.textContent = await open(queued.id) ? "" : t("暂时无法打开原任务，请重试。");
    });
    const card = el("div", { class: "chat-task-card" }, el("strong", { text: queued.title }), status, button, note);
    card.dataset.taskId = queued.id;
    list.append(card);
    let seen = false;
    const stop = watchEffect(() => {
      const task = center.state.tasks.find(row => row.id === queued.id);
      if (card.isConnected) seen = true;
      else if (seen) { queueMicrotask(() => stop()); return; }
      if (task) status.textContent = workStatus(center, task);
    });
  }
  return list;
}
