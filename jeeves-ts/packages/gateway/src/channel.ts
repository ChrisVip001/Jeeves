import type { InteractionSource } from "@jeeves/client";

/** What a channel can do; the shared layer degrades around what it cannot. */
export interface ChannelCapabilities {
  markdown: boolean;
  buttons: boolean;
  editMessage: boolean;
  files: boolean;
  /** Longest single text message, in UTF-16 units. */
  maxTextLen: number;
  groups: boolean;
}

/** Where a message goes: a chat (or user) on one channel, optionally as a reply. */
export interface ChannelTarget {
  channel: string;
  chatId: string;
  replyTo?: string;
}

export interface OutboundMessage {
  text: string;
  /** Interactive card / buttons payload, only when `capabilities.buttons`. */
  card?: Record<string, unknown>;
}

export interface SendReceipt {
  ok: boolean;
  /** The platform's id for what was sent, when it gave one. */
  messageId?: string;
  error?: string;
}

/** One accepted inbound message, after allowlist and dedupe. */
export interface InboundMessage {
  /** `<channel>:<messageId>` */
  sourceKey: string;
  conversation: string;
  text: string;
  senderId: string;
  chatId: string;
  source?: InteractionSource;
}

export interface InboundSink {
  (message: InboundMessage): Promise<void>;
}

/** The read-only state of one channel, for the capability view. */
export interface ChannelProbe {
  id: string;
  state: "ready" | "unauthorized" | "offline" | "unknown";
  detail?: string;
}

export interface ChannelAdapter {
  readonly id: string;
  readonly capabilities: ChannelCapabilities;
  /** Run until `stop()`. */
  start(sink?: InboundSink): Promise<void>;
  stop(): void;
  send(to: ChannelTarget, message: OutboundMessage): Promise<SendReceipt>;
  probe(): Promise<ChannelProbe>;
}
