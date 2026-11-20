/** What every channel needs to ask for an approval. */
export interface ApprovalPromptInput {
  turnId: string;
  requestId: string;
  version: number;
  tool: string;
  argsPreview: string;
  reason: string;
  expiresInMs: number;
}

export interface ApprovalPrompt {
  /** `buttons`: the adapter renders its own buttons; `reply`: the text carries the instruction. */
  kind: "buttons" | "reply";
  text: string;
}

/**
 * The same question for every channel. With buttons the adapter attaches them; without, the
 * text says "reply 1 / 2" and carries the approval id and version, so a late or stale reply is
 * refused rather than guessed.
 */
export function approvalPrompt(
  request: ApprovalPromptInput,
  capabilities: { buttons: boolean },
): ApprovalPrompt {
  const minutes = Math.max(1, Math.round(request.expiresInMs / 60_000));
  const base = [
    `Wants to run: ${request.tool}`,
    request.argsPreview,
    `Why it asks: ${request.reason}`,
    `No answer within ${minutes} min counts as a no. Only listed dispatchers can answer, once.`,
  ];
  if (capabilities.buttons) return { kind: "buttons", text: base.join("\n") };
  const id = `${request.requestId} v${request.version}`;
  return {
    kind: "reply",
    text: [...base, `Reply "1 ${id}" to approve once, or "2 ${id}" to deny.`].join("\n"),
  };
}

export interface ApprovalReply {
  requestId: string;
  version: number;
  approved: boolean;
}

/** Read "1 <id> v<version>" / "2 <id> v<version>"; anything looser is not an answer. */
export function parseApprovalReply(text: string): ApprovalReply | null {
  const match = /^\s*([12])\s+(\S+)\s+v(\d+)\s*$/.exec(text);
  if (match === null) return null;
  return { approved: match[1] === "1", requestId: match[2]!, version: Number(match[3]) };
}
