import { jsonEvents } from "../../../../protocol/typescript/event-stream.mts";
import type { Transport } from "./transport.ts";
import { CODES } from "./protocol.ts";

/**
 * Carries the existing protocol to a running cockpit without starting another engine.
 *
 * The notifications SSE is the client's only inbound channel, so a drop (Runtime restart,
 * proxy idle timeout, network blip) must not permanently poison the transport: the gateway
 * reuses one client for its whole process life. We reconnect with backoff and only treat an
 * explicit close() as terminal — `#closedReason` on JeevesClient must stay null across drops.
 */
export class HttpTransport implements Transport {
  readonly #url: string;
  readonly #headers: Record<string, string>;
  readonly #fetch: typeof fetch;
  readonly #lines = new Set<(line: string) => void>();
  readonly #closes = new Set<(reason: string) => void>();
  readonly #completed = new Set<string>();
  readonly #waiting = new Map<string, { resolve: () => void; reject: (error: Error) => void }>();
  #streamAbort: AbortController | null = null;
  #reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  #reconnectDelay = 500;
  #closed = false;
  #connecting = false;
  #connected = false;
  /** Resolves when the first connection attempt has ended. Never rejects, so a transient
   *  startup failure cannot permanently poison requests the way the old #ready could. */
  readonly #firstAttempt: Promise<void>;

  constructor(options: { url: string; token?: string; fetch?: typeof fetch }) {
    this.#url = options.url.replace(/\/$/, "");
    this.#headers = options.token ? { Authorization: `Bearer ${options.token}` } : {};
    this.#fetch = options.fetch ?? fetch;
    this.#firstAttempt = this.#connect();
  }

  /** Establish (or re-establish) the inbound SSE stream. Never throws — retries on failure. */
  async #connect(): Promise<void> {
    if (this.#closed || this.#connecting || this.#connected) return;
    this.#connecting = true;
    const abort = new AbortController();
    this.#streamAbort = abort;
    try {
      const response = await this.#fetch(`${this.#url}/api/notifications`, { headers: this.#headers, signal: abort.signal });
      if (!response.ok || !response.body) throw new Error(`Jeeves notifications failed: HTTP ${response.status}`);
      this.#connecting = false;
      this.#connected = true;
      this.#reconnectDelay = 500;
      void this.#read(response.body);
    } catch {
      this.#connecting = false;
      this.#lost("Could not connect to Jeeves notifications");
    }
  }

  async #read(body: ReadableStream<Uint8Array>): Promise<void> {
    try {
      for await (const notification of jsonEvents<{ type?: string; turnId?: string }>(body)) {
        this.#emit({ jsonrpc: "2.0", method: "turn/notification", params: notification });
        if (notification.type === "turnCompleted" && notification.turnId) {
          this.#completed.add(notification.turnId);
          if (this.#completed.size > 1000) this.#completed.delete(this.#completed.values().next().value!);
          this.#waiting.get(notification.turnId)?.resolve();
        }
      }
    } catch {
    } finally {
      this.#connected = false;
      this.#lost("Jeeves notification connection closed");
    }
  }

  /**
   * Stream ended or failed. Resolve any completion waiters with an error (they will be
   * re-driven), then reconnect with exponential backoff. This is NOT a client close: onClose is
   * deliberately not fired so JeevesClient.#closedReason stays null and future calls work.
   */
  #lost(reason: string): void {
    if (this.#closed || this.#reconnectTimer || this.#connecting || this.#connected) return;
    for (const waiter of this.#waiting.values()) waiter.reject(new Error(reason));
    this.#waiting.clear();
    const delay = this.#reconnectDelay;
    this.#reconnectDelay = Math.min(this.#reconnectDelay * 2, 10_000);
    this.#reconnectTimer = setTimeout(() => {
      this.#reconnectTimer = null;
      if (!this.#closed) void this.#connect();
    }, delay);
  }

  write(line: string): void {
    const request = JSON.parse(line) as { id: unknown; method: string };
    void this.#request(request).catch(() => this.#emit({ jsonrpc: "2.0", id: request.id, error: { code: CODES.INTERNAL_ERROR, message: "Cannot reach the existing Jeeves runtime; the request was not replayed" } }));
  }

  async #request(request: { id: unknown; method: string }): Promise<void> {
    await this.#firstAttempt;
    if (this.#closed) throw new Error("Jeeves transport is closed");
    const startsTurn = request.method === "turn/start" || request.method === "interaction/start";
    const subscription = this.#streamAbort;
    if (startsTurn && !this.#connected) throw new Error("Jeeves notifications are disconnected");
    const response = await this.#fetch(`${this.#url}/api/gateway/rpc`, {
      method: "POST", headers: { ...this.#headers, "Content-Type": "application/json" },
      body: JSON.stringify(request),
    });
    if (!response.ok) throw new Error(`Jeeves request failed: HTTP ${response.status}`);
    const message = await response.json() as { result?: { turnId?: string } };
    const turnId = message.result?.turnId;
    if (turnId && !this.#completed.has(turnId)) {
      if (!this.#connected || subscription !== this.#streamAbort || this.#closed) {
        throw new Error("Jeeves completion subscription was lost");
      }
      let timer: ReturnType<typeof setTimeout> | undefined;
      try {
        await new Promise<void>((resolve, reject) => {
          this.#waiting.set(turnId, { resolve, reject });
          timer = setTimeout(() => reject(new Error("Jeeves completion notification was not received")), 20_000);
        });
      } finally {
        clearTimeout(timer);
        this.#waiting.delete(turnId);
      }
    }
    this.#emit(message);
  }

  #emit(value: unknown): void {
    for (const handler of this.#lines) handler(JSON.stringify(value));
  }

  onLine(handler: (line: string) => void): void { this.#lines.add(handler); }
  onLog(_handler: (text: string) => void): void {}
  onClose(handler: (reason: string) => void): void { this.#closes.add(handler); }
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    if (this.#reconnectTimer) { clearTimeout(this.#reconnectTimer); this.#reconnectTimer = null; }
    this.#streamAbort?.abort();
    const reason = "Jeeves HTTP transport closed";
    for (const waiter of this.#waiting.values()) waiter.reject(new Error(reason));
    this.#waiting.clear();
    for (const handler of this.#closes) handler(reason);
  }
}
