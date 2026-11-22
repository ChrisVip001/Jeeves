import type { TaskEventsParams, TaskEventsResponse, TaskEventView } from "../schema/typescript/v2/index.ts";

/** What one catch-up produced. `reset` means the server's history no longer contains our cursor. */
export interface CatchUp {
  events: TaskEventView[];
  reset: boolean;
}

export type FetchTaskEvents = (params: TaskEventsParams) => Promise<TaskEventsResponse>;

/**
 * Remembers the last cursor in memory and pulls only what is newer. Duplicates (same cursor)
 * are dropped; a gap cannot occur because pages are contiguous from the remembered cursor.
 * Only when the server's newest cursor is behind ours (its log was replaced) does it start
 * over, and it says so with `reset: true` so the caller reloads its snapshot.
 */
export class TaskEventFollower {
  #cursor = 0;
  readonly #fetch: FetchTaskEvents;
  readonly #taskId: string | undefined;

  constructor(fetch: FetchTaskEvents, taskId?: string) {
    this.#fetch = fetch;
    this.#taskId = taskId;
  }

  get cursor(): number {
    return this.#cursor;
  }

  async catchUp(): Promise<CatchUp> {
    const events: TaskEventView[] = [];
    let reset = false;
    for (;;) {
      const page = await this.#fetch({ cursor: this.#cursor, taskId: this.#taskId, limit: 500 });
      if (page.latestCursor < this.#cursor) {
        this.#cursor = 0;
        events.length = 0;
        reset = true;
        continue;
      }
      for (const event of page.events) {
        if (event.cursor > this.#cursor) events.push(event);
      }
      this.#cursor = Math.max(this.#cursor, page.nextCursor);
      if (!page.hasMore) return { events, reset };
    }
  }
}
