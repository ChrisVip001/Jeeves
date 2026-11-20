export interface DedupeOptions {
  ttlMs?: number;
  max?: number;
  now?: () => number;
}

/**
 * "Have I handled this message already?" Platforms re-send when an answer is slow or a
 * connection drops; the key is `sourceKey(channel, messageId)`, so channels never collide.
 */
export class Dedupe {
  readonly #seen = new Map<string, number>();
  readonly #ttl: number;
  readonly #max: number;
  readonly #now: () => number;

  constructor(options: DedupeOptions = {}) {
    this.#ttl = options.ttlMs ?? 30 * 60 * 1000;
    this.#max = options.max ?? 1000;
    this.#now = options.now ?? Date.now;
  }

  /** `true` the first time a key is seen (and records it); `false` for a repeat. */
  claim(key: string): boolean {
    const now = this.#now();
    const at = this.#seen.get(key);
    if (at !== undefined && now - at < this.#ttl) return false;
    this.#seen.delete(key);
    this.#seen.set(key, now);
    while (this.#seen.size > this.#max) {
      const oldest = this.#seen.keys().next().value;
      if (oldest === undefined) break;
      this.#seen.delete(oldest);
    }
    return true;
  }

  /** Drop expired keys; call from a heartbeat. */
  sweep(): void {
    const now = this.#now();
    for (const [key, at] of this.#seen) if (now - at >= this.#ttl) this.#seen.delete(key);
  }
}

/** The inbound identity of a message: `<channel>:<messageId>`. */
export function sourceKey(channel: string, messageId: string): string {
  return `${channel}:${messageId}`;
}
