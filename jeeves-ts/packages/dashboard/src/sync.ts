
import { followEvents, type EventPing } from "./api";

export interface SyncHandlers {
  /** Coalesced activity: pull /api/data (and the open session) again. */
  onActivity: (ping: EventPing) => void;
}

/**
 * A burst must not drown its headline: "a crashed turn is resuming" is the whole point of
 * the burst it rides in (resumed → progress → …). Only the last ping of a burst is
 * delivered, so the resume ping keeps its seat until the burst goes out.
 */
const isResume = (ping: EventPing): boolean => ping.kind === "task_resumed";

export function startSync(handlers: SyncHandlers): void {
  let timer: number | null = null;
  let pending: EventPing | null = null;

  const knock = (ping: EventPing): void => {
    if (pending !== null && isResume(pending) && !isResume(ping)) return;
    pending = ping;
    if (timer !== null) return;
    timer = window.setTimeout(() => {
      timer = null;
      const burst = pending;
      pending = null;
      if (burst !== null) handlers.onActivity(burst);
    }, 500);
  };

  void (async () => {
    for (;;) {
      try {
        for await (const ping of followEvents()) knock(ping);
      } catch {
      }
      await new Promise((resolve) => setTimeout(resolve, 3000));
    }
  })();
}
