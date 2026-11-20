import type { ApprovalRequestedNotification } from "@jeeves/client";
import type { Responder } from "./bridge.ts";
import { explain } from "./bridge.ts";
import type {
  ChannelAdapter,
  ChannelCapabilities,
  ChannelProbe,
  ChannelTarget,
  OutboundMessage,
  SendReceipt,
} from "./channel.ts";
import type { ApprovalAnswer } from "./approvals.ts";
import { Allowlist, Dedupe, approvalPrompt, sourceKey, splitMessage } from "./shared/index.ts";

export const MESSAGE_LIMIT = 4096;

/** An approval answer read from a button: the exact question (with version) and the verdict. */
export interface TelegramApproval {
  turnId: string;
  requestId: string;
  version: number;
  approved: boolean;
}

export interface TelegramOptions {
  /** Bot token. Read from the environment by the launcher; never logged. */
  token: string;
  /** Telegram user ids (numbers, as text). Unset = answer everyone. */
  allow?: readonly string[];
  /** No owner yet: answer nobody and log the first sender as a candidate. */
  claim?: boolean;
  apiBase?: string;
  fetch?: typeof globalThis.fetch;
  /** Where `getUpdates` resumes after a restart. Default: in memory only. */
  offset?: { read(): number; write(next: number): void };
  /** Long-poll seconds. 0 in tests. */
  pollTimeoutSec?: number;
  onApproval?: (action: TelegramApproval, actor: string) => Promise<ApprovalAnswer>;
  onLog?: (text: string) => void;
}

interface TgUser {
  id: number;
}
interface TgMessage {
  message_id: number;
  from?: TgUser;
  chat: { id: number; type?: string };
  text?: string;
}
interface TgUpdate {
  update_id: number;
  message?: TgMessage;
  callback_query?: { id: string; from: TgUser; data?: string; message?: TgMessage };
}

export class TelegramAdapter implements ChannelAdapter {
  readonly id = "telegram";
  readonly capabilities: ChannelCapabilities = {
    markdown: false,
    buttons: true,
    editMessage: true,
    files: false,
    maxTextLen: MESSAGE_LIMIT,
    groups: false,
  };

  readonly #token: string;
  readonly #allow: Allowlist;
  readonly #base: string;
  readonly #fetch: typeof globalThis.fetch;
  readonly #offset: NonNullable<TelegramOptions["offset"]>;
  readonly #pollSec: number;
  readonly #respond: Responder;
  readonly #onApproval: TelegramOptions["onApproval"];
  readonly #log: (text: string) => void;
  readonly #seen = new Dedupe();
  /** Short keys for button payloads (Telegram caps callback data at 64 bytes). Lost on restart. */
  readonly #pending = new Map<string, Omit<TelegramApproval, "approved">>();
  #next = 1;
  #stopped = false;

  constructor(options: TelegramOptions, respond: Responder) {
    this.#token = options.token;
    this.#allow = new Allowlist(options.allow, options.claim === true);
    this.#base = options.apiBase ?? "https://api.telegram.org";
    this.#fetch = options.fetch ?? globalThis.fetch;
    let memory = 0;
    this.#offset = options.offset ?? { read: () => memory, write: (n) => void (memory = n) };
    this.#pollSec = options.pollTimeoutSec ?? 30;
    this.#respond = respond;
    this.#onApproval = options.onApproval;
    this.#log = options.onLog ?? ((text) => console.error(text));
  }

  async #api<T>(method: string, body: Record<string, unknown>): Promise<T> {
    const response = await this.#fetch(`${this.#base}/bot${this.#token}/${method}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    const json = (await response.json()) as { ok: boolean; result?: T; description?: string };
    if (!json.ok) throw new Error(`${method}: ${json.description ?? response.status}`);
    return json.result as T;
  }

  start(): Promise<void> {
    return this.run();
  }

  stop(): void {
    this.#stopped = true;
  }

  /** Long-poll until stopped. A failed poll waits and tries again; it never ends the process. */
  async run(): Promise<void> {
    let delay = 1000;
    while (!this.#stopped) {
      try {
        const offset = this.#offset.read();
        const updates = await this.#api<TgUpdate[]>("getUpdates", {
          offset,
          timeout: this.#pollSec,
          allowed_updates: ["message", "callback_query"],
        });
        delay = 1000;
        for (const update of updates) {
          await this.#handle(update).catch((error: unknown) =>
            this.#log(`an update was not handled: ${error instanceof Error ? error.message : String(error)}`),
          );
          this.#offset.write(update.update_id + 1);
        }
      } catch (error) {
        this.#log(`getUpdates failed (${error instanceof Error ? error.message : String(error)}); retrying in ${delay / 1000}s`);
        await new Promise((resolve) => setTimeout(resolve, delay));
        delay = Math.min(delay * 2, 30_000);
      }
    }
  }

  async #handle(update: TgUpdate): Promise<void> {
    if (!this.#seen.claim(sourceKey("telegram", `u${update.update_id}`))) {
      this.#log("this update was handled a moment ago; skipped");
      return;
    }
    if (update.callback_query !== undefined) return this.#clicked(update.callback_query);
    const message = update.message;
    const userId = String(message?.from?.id ?? "");
    if (message === undefined || userId === "" || message.text === undefined) return;
    if (this.#allow.claiming) {
      this.#log(`claim candidate: ${userId}`);
      return;
    }
    if (!this.#allow.allows(userId)) {
      this.#log(`outside the allowlist, not answering: ${userId}`);
      return;
    }
    if (message.chat.type !== undefined && message.chat.type !== "private") {
      this.#log("a group message; only direct chats are answered for now");
      return;
    }
    let reply: string;
    try {
      reply = await this.#respond(`telegram:${userId}`, message.text, undefined, `telegram:${update.update_id}`);
    } catch (error) {
      reply = explain(error);
    }
    await this.send({ channel: "telegram", chatId: String(message.chat.id) }, { text: reply });
  }

  async #clicked(query: NonNullable<TgUpdate["callback_query"]>): Promise<void> {
    const answer = (text: string) => this.#api("answerCallbackQuery", { callback_query_id: query.id, text });
    const actor = String(query.from.id);
    const parts = (query.data ?? "").split(":");
    const pending = parts[0] === "ap" ? this.#pending.get(parts[1] ?? "") : undefined;
    if (!this.#allow.allows(actor) || this.#allow.claiming) {
      await answer("You are not allowed to confirm this.");
      return;
    }
    if (pending === undefined || this.#onApproval === undefined) {
      await answer("This request was already answered or has expired; nothing changed.");
      return;
    }
    const approved = parts[2] === "1";
    const result = await this.#onApproval({ ...pending, approved }, actor);
    const accepted = typeof result === "boolean" ? result : result.accepted;
    if (!accepted) {
      const first = typeof result === "boolean" ? null : (result.answeredBy ?? null);
      const note = first === null
        ? "This request was already answered or has expired; nothing changed."
        : `Already answered by ${first}; nothing changed.`;
      await answer(note);
      this.#pending.delete(parts[1] ?? "");
      const where = query.message;
      if (where !== undefined) {
        await this.#api("editMessageText", { chat_id: where.chat.id, message_id: where.message_id, text: note }).catch(() => undefined);
      }
      return;
    }
    this.#pending.delete(parts[1] ?? "");
    await answer(approved ? "Approved once." : "Denied.");
    const where = query.message;
    if (where !== undefined) {
      await this.#api("editMessageText", {
        chat_id: where.chat.id,
        message_id: where.message_id,
        text: `${approved ? "Approved once" : "Denied"} by ${actor}.`,
      }).catch(() => undefined);
    }
  }

  async send(to: ChannelTarget, message: OutboundMessage): Promise<SendReceipt> {
    let last: number | undefined;
    try {
      for (const chunk of splitMessage(message.text, MESSAGE_LIMIT)) {
        const sent = await this.#api<{ message_id: number }>("sendMessage", {
          chat_id: to.chatId,
          text: chunk,
          ...(to.replyTo === undefined ? {} : { reply_parameters: { message_id: Number(to.replyTo) } }),
        });
        last = sent.message_id;
      }
      return last === undefined ? { ok: true } : { ok: true, messageId: String(last) };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
  }

  /** Ask for an approval with two buttons; the answer comes back through `onApproval`. */
  async sendApproval(chatId: string, request: ApprovalRequestedNotification): Promise<void> {
    const key = String(this.#next++);
    this.#pending.set(key, { turnId: request.turnId, requestId: request.requestId, version: request.version });
    await this.#api("sendMessage", {
      chat_id: chatId,
      text: `Jeeves needs your confirmation\n${approvalPrompt(request, { buttons: true }).text}`,
      reply_markup: {
        inline_keyboard: [
          [
            { text: "Approve once", callback_data: `ap:${key}:1` },
            { text: "Deny", callback_data: `ap:${key}:0` },
          ],
        ],
      },
    });
  }

  async probe(): Promise<ChannelProbe> {
    try {
      await this.#api("getMe", {});
      return { id: this.id, state: "ready" };
    } catch (error) {
      return { id: this.id, state: "offline", detail: error instanceof Error ? error.message : String(error) };
    }
  }
}
