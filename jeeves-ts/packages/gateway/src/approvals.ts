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
}

/** Read a card button's `value` back; anything else is not ours and returns `null`. */
export function approvalActionOf(value: unknown): ApprovalAction | null {
  if (typeof value !== "object" || value === null) return null;
  const v = value as Record<string, unknown>;
  if (v["jeeves"] !== "approval") return null;
  if (typeof v["turnId"] !== "string" || typeof v["requestId"] !== "string") return null;
  if (typeof v["approved"] !== "boolean") return null;
  return { jeeves: "approval", turnId: v["turnId"], requestId: v["requestId"], approved: v["approved"] };
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
): Record<string, unknown> {
  const minutes = Math.max(1, Math.round(request.expiresInMs / 60_000));
  const lines = [
    task === null ? null : `**Task:** ${task.title}`,
    `**Wants to run:** ${request.tool}`,
    code(request.argsPreview),
    `**Why it asks:** ${request.reason}`,
    `No answer within ${minutes} min counts as a no. Only listed dispatchers can answer, once.`,
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
    } satisfies ApprovalAction,
  });
  return {
    config: { wide_screen_mode: true },
    header: { template: "orange", title: { tag: "plain_text", content: "Jeeves needs your confirmation" } },
    elements: [
      { tag: "div", text: { tag: "lark_md", content: lines.join("\n") } },
      { tag: "action", actions: [button("Approve once", "primary", true), button("Deny", "danger", false)] },
    ],
  };
}

/** Where an approval belongs: the chat it should be asked in, and the task (if any) behind it. */
export interface ApprovalOrigin {
  source: InteractionSource | null;
  task: AgentTaskView | null;
  replyWithText?: boolean;
}

export interface RelayDeps {
  /** Find the chat a waiting request came from. `null` = not found (already answered, or gone). */
  locate(turnId: string, requestId: string): Promise<ApprovalOrigin | null>;
  /** Post a card into a chat as a reply to the dispatching message. */
  sendCard(chatId: string, messageId: string, card: Record<string, unknown>): Promise<void>;
  sendTextApproval?(source: InteractionSource, request: ApprovalRequestedNotification): Promise<void>;
  onLog?: (text: string) => void;
}

/**
 * Returns the handler for `approvalRequested`: when the asking turn was dispatched from a Lark
 * group (directly, or through a task it created), ask in that thread with a card. Requests from
 * local views are left to the views; nothing else here answers on anyone's behalf.
 */
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
        if (origin?.replyWithText) {
          await deps.sendTextApproval?.(source, request);
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
