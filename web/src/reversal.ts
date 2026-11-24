import { t } from "./i18n.ts";
import type { ToolOperationView } from "./protocol.ts";

/** The only reversal this journal offers: a human-approved next turn, never a hidden undo. */
export function reversalHint(operation: ToolOperationView): string {
  if (operation.status === "intent" || operation.status === "not_executed") {
    return t("这次操作没有产生需要撤销的效果。");
  }
  const args: unknown = operation.arguments;
  const path =
    typeof args === "object" && args !== null
      ? (args as Record<string, unknown>).path
      : null;
  if (
    typeof path === "string" &&
    path.length > 0 &&
    ["write_file", "edit", "apply_patch"].includes(operation.tool)
  ) {
    return t(
      "这条记录只说明观察到什么，不会自动撤销。要恢复 {path}，请在对话里说明目标版本，由你再次确认后执行。",
      { path },
    );
  }
  if (["send_message", "feishu_send", "email_send"].includes(operation.tool)) {
    return t("外部发送不能在这里撤销；请到接收端核对，需要时再走一次审批。");
  }
  return t("这类操作没有通用逆操作；请说明要达成的状态，Jeeves 会在新的审批下处理。");
}
