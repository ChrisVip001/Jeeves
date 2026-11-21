import type {
  AgentTaskView,
  ApprovalRequestedNotification,
  InteractionSource,
} from "@jeeves/client";

/**
 * What a button on the card carries back. `jeeves: "approval"` tells our callback apart from any
 * other card the app might send; the ids are echoed verbatim to `approval/respond`.
 */
export interface ApprovalAction {
  jeeves: "approval";
  turnId: string;
  requestId: string;
  approved: boolean;
  version?: number;
  bindToSender?: true;
}

/** Read a card button's `value` back; anything else is not ours and returns `null`. */
export function approvalActionOf(value: unknown): ApprovalAction | null {
  if (typeof value !== "object" || value === null) return null;
  const v = value as Record<string, unknown>;
  if (v["jeeves"] !== "approval") return null;
  if (typeof v["turnId"] !== "string" || typeof v["requestId"] !== "string") return null;
  if (typeof v["approved"] !== "boolean") return null;
  const version = v["version"];
  const bindToSender = v["bindToSender"];
  if (version !== undefined && (typeof version !== "number" || !Number.isSafeInteger(version) || version < 1)) return null;
  if (bindToSender !== undefined && (bindToSender !== true || version === undefined)) return null;
  return {
    jeeves: "approval", turnId: v["turnId"], requestId: v["requestId"], approved: v["approved"],
    ...(version === undefined ? {} : { version }),
    ...(bindToSender === true ? { bindToSender } : {}),
  };
}

export function approvalReplyText(request: ApprovalRequestedNotification): string {
  const key = `${request.requestId} v${request.version}`;
  return `也可回复「批准 ${key}」或「拒绝 ${key}」。群聊请同时 @机器人。`;
}

/** Lark markdown treats `*`, `_`, `~` and backticks as markup; a command line must show as typed. */
function code(text: string): string {
  return "```\n" + text.replace(/```/g, "ˋˋˋ") + "\n```";
}

/**
 * The interactive card: what wants to run, why it was not let through, which task asked, and
 * when silence becomes a "no". Two buttons; the answer is once only (`remember: once`), so a
 * standing rule can never be granted from a group chat.
 */
export function approvalCard(
  request: ApprovalRequestedNotification,
  task: AgentTaskView | null,
  bindToSender = false,
): Record<string, unknown> {
  const minutes = Math.max(1, Math.round(request.expiresInMs / 60_000));
  const lines = [
    task === null ? null : `**Task:** ${task.title}`,
    `**Wants to run:** ${request.tool}`,
    code(request.argsPreview),
    `**Why it asks:** ${request.reason}`,
    bindToSender
      ? `原发起人可在 ${Math.ceil(request.expiresInMs / 1000)} 秒内确认一次。`
      : `No answer within ${minutes} min counts as a no. Only listed dispatchers can answer, once.`,
    bindToSender ? approvalReplyText(request) : null,
  ].filter((line): line is string => line !== null);
  const button = (label: string, kind: string, approved: boolean) => ({
    tag: "button",
    text: { tag: "plain_text", content: label },
    type: kind,
    value: {
      jeeves: "approval",
      turnId: request.turnId,
      requestId: request.requestId,
      approved,
      version: request.version,
      ...(bindToSender ? { bindToSender: true } : {}),
    } satisfies ApprovalAction,
  });
  return {
    config: { wide_screen_mode: true },
    header: { template: "orange", title: { tag: "plain_text", content: "Jeeves needs your confirmation" } },
    elements: [
      { tag: "div", text: { tag: "lark_md", content: lines.join("\n") } },
      { tag: "action", actions: [
        button(bindToSender ? "批准一次" : "Approve once", "primary", true),
        button(bindToSender ? "拒绝" : "Deny", "danger", false),
      ] },
    ],
  };
}

/** Where an approval belongs: the chat it should be asked in, and the task (if any) behind it. */
export interface ApprovalOrigin {
  source: InteractionSource | null;
  task: AgentTaskView | null;
  bindToSender?: boolean;
}

export interface RelayDeps {
  /** Find the chat a waiting request came from. `null` = not found (already answered, or gone). */
  locate(turnId: string, requestId: string): Promise<ApprovalOrigin | null>;
  /** Post a card into a chat as a reply to the dispatching message. */
  sendCard(chatId: string, messageId: string, card: Record<string, unknown>): Promise<void>;
  sendOriginApproval?(source: InteractionSource, request: ApprovalRequestedNotification): Promise<void>;
  onLog?: (text: string) => void;
}

/** Route a live approval to its originating Lark conversation. Local views handle their own requests. */
export function approvalRelay(
  deps: RelayDeps,
): (request: ApprovalRequestedNotification) => void {
  const asked = new Set<string>();
  return (request) => {
    const key = `${request.requestId}:${request.version}`;
    if (asked.has(key)) return;
    asked.add(key);
    setTimeout(() => asked.delete(key), Math.max(0, request.expiresInMs)).unref();
    void (async () => {
      try {
        const origin = await deps.locate(request.turnId, request.requestId);
        const source = origin?.source ?? null;
        if (source === null || source.channel !== "lark") return;
        if (origin?.bindToSender) {
          await deps.sendOriginApproval?.(source, request);
          return;
        }
        await deps.sendCard(source.chatId, source.messageId, approvalCard(request, origin?.task ?? null));
      } catch (error) {
        deps.onLog?.(
          `could not deliver approval request ${request.requestId}: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
    })();
  };
}

/** The one line posted under the card after a click. */
export type ApprovalAnswer = boolean | { accepted: boolean; answeredBy?: string | null };

export function answerLine(
  action: ApprovalAction,
  answer: ApprovalAnswer,
  who: string,
): string {
  const accepted = typeof answer === "boolean" ? answer : answer.accepted;
  const first = typeof answer === "boolean" ? null : (answer.answeredBy ?? null);
  if (!accepted) {
    return first === null
      ? "This request was already answered or has expired; nothing changed."
      : `Already answered by ${first}; nothing changed.`;
  }
  return action.approved ? `Approved once by ${who}.` : `Denied by ${who}.`;
}

export const NOT_ALLOWED = "You are not allowed to confirm this; only the listed dispatchers can.";
