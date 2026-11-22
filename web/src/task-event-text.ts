import type { TaskEventView } from "./ui/types";
import type { UiTranslator } from "./ui/translate";

/** Localize only the known event vocabulary; historic/unrecognized events keep their evidence. */
export function taskEventText(event: TaskEventView, t: UiTranslator) {
  const labels: Record<string, string> = {
    verify: t("验收"),
    budget: t("预算用完"),
    auto_continue: t("自动续跑"),
    contradiction: t("发现矛盾"),
    job_finished: t("后台作业结束"),
    run_started: t("本次执行开始"),
    run_finished: t("本次执行结束"),
    approval_requested: t("请求确认"),
    approval_decided: t("确认结果"),
    git_checkpoint: t("Git 检查点"),
  };
  const known = Object.hasOwn(labels, event.kind);
  const label = known ? labels[event.kind]! : event.kind;
  let detail = event.detail;
  if (event.kind === "run_finished" && detail) {
    const states: Record<string, string> = {
      completed: t("本次执行完成"),
      waiting: t("等待人工处理"),
      suspended: t("已暂停"),
      failed: t("执行失败"),
      cancelled: t("已取消"),
    };
    if (Object.hasOwn(states, detail)) detail = states[detail]!;
  }
  if (event.kind === "approval_decided" && detail) {
    const match = /^([^·]+) · (approved|denied|timeout|voided) · (.*)$/s.exec(detail);
    if (match) {
      const outcomes: Record<string, string> = {
        approved: t("已允许"), denied: t("已拒绝"),
        timeout: t("已超时"), voided: t("确认已失效"),
      };
      detail = [match[1], outcomes[match[2]!], match[3]].filter(Boolean).join(" · ");
    }
  }
  return { label, detail, known };
}
