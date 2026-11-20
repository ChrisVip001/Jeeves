import { JeevesClient, JeevesError, METHODS, StdioTransport, HttpTransport } from "@jeeves/client";
import type {
  AgentTaskView,
  ApprovalRequestedNotification,
  InteractionSource,
  ServerNotification,
  TaskUpdatedNotification,
  TaskWakeParams,
  TaskWakeResponse,
} from "@jeeves/client";

/** What a completed turn leaves behind. */
export interface TurnOutcome {
  /** Jeeves's session id — it is `conversation` itself; see `sessionIdFor`. */
  sessionId: string;
  reply: string;
  /** Tools actually used this turn, in call order; repeats count. */
  tools: string[];
  /** The model that really answered. When graph routes to quick that's the small one — as-is. */
  model: string;
  iterations: number;
}

/** Movement while a turn runs. Adapters can use it to show "typing…". */
export interface ProgressNote {
  conversation: string;
  text: string;
}

/**
 * The single interface between an adapter and Jeeves: one message in, one reply out.
 *
 * A function rather than the `JeevesBridge` itself, so adapters can be tested independently —
 * long polling, WebSocket heartbeats, message splitting, allowlists have nothing to do with Jeeves.
 *
 * Like `ProgressNote`, it belongs to the **platform-agnostic** half, so it lives here rather
 * than in one platform's adapter — otherwise each adapter would import its interface from another
 * adapter's file.
 */
export type Responder = (
  conversation: string,
  text: string,
  /** Set only for a chat-group message from a dispatcher: it goes to the main conversation. */
  source?: InteractionSource,
  /** The platform's id for this message; makes a re-delivery after a restart run only once. */
  requestId?: string,
) => Promise<string>;

/** The id shape the Runtime accepts for a stable message identity. */
const safeId = (id: string): string => id.replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 120);

/** The main conversation's session id (`jeeves-app-server` `MAIN_SESSION`). */
export const MAIN_SESSION = "jeeves-main";

export interface JeevesBridgeOptions {
  /** Attach to a running Web runtime instead of spawning an engine. */
  url?: string;
  token?: string;
  /** `jeeves` binary path. When absent, found via the client's usual order. */
  command?: string;
  args?: string[];
  cwd?: string;
  /** Environment passed through to app-server (`JEEVES_HOME`, the vendors' API keys…). */
  env?: NodeJS.ProcessEnv;
  /**
   * An already-connected client. When absent, an app-server is started here.
   *
   * This seam exists for tests: queuing, notification folding, clearing wreckage after failure
   * are pure logic — proving them shouldn't need a real process, a model, or a network trip.
   */
  client?: JeevesClient;
  /** The server's human words and local noise. Defaults to stderr. */
  onLog?: (text: string) => void;
  /** Progress within a turn. Dropped by default. */
  onProgress?: (note: ProgressNote) => void;
}

/**
 * Session ids derive directly from platform identities.
 *
 * A gateway would otherwise "remember who uses which session" — a table, a file, all invalid
 * after one cold start, plus a migration path nobody ever tests. No table here:
 * `lark:12345` is both this user and that session.
 *
 * The gateway is therefore **stateless**: restart costs nothing, another process doesn't split a
 * person into two talks. To really change sessions, the user taps "restart" and a suffix changes.
 */
export function sessionIdFor(conversation: string): string {
  return conversation;
}

/**
 * Translate what happened in a turn into words for humans.
 *
 * Only the few things really worth interrupting the user for. Successful tool calls are normal —
 * unreported; failures are reported, because the next explanation usually relates to them.
 *
 * Only notifications carrying a recognizable `turnId` appear here — before announcing anything
 * you must know whom to announce to. So `consolidationCompleted` (a batch of facts distilled) is
 * absent: it is sent during a turn but **carries no `turnId`** (see protocol/v2.rs), so with two
 * conversations in flight there's no telling whom to tell. Say less rather than let someone read
 * Alice's memory updates in Bob's chat; bringing it back means adding `turnId` to that notification.
 */
function progressText(notification: ServerNotification): string | null {
  switch (notification.type) {
    case "toolStarted":
      return `using ${notification.tool}…`;
    case "toolCompleted":
      return notification.status === "error"
        ? `${notification.tool} did not succeed; it is trying another way`
        : null;
    default:
      return null;
  }
}

/**
 * Translate an error into one sentence that can go straight to the user.
 *
 * `JeevesError` carries a code precisely for this: **"model not configured" and "feature not built"
 * are two things to a user** — one they fix themselves, the other they wait out. Don't merge them.
 */
export function explain(error: unknown): string {
  if (error instanceof JeevesError) {
    if (error.isProvider) {
      return `the model side is not connected: ${error.message}\ncheck the key and the model name in \`jeeves config\` first.`;
    }
    if (error.isNotImplemented) {
      return `this is not built yet: ${error.message}`;
    }
    if (error.isInvalidParams) {
      return `the arguments are wrong, most likely the gateway and the app-server are different versions: ${error.message}`;
    }
    return `Jeeves reported an error (${error.code}): ${error.message}`;
  }
  return `cannot reach Jeeves: ${error instanceof Error ? error.message : String(error)}`;
}

interface InFlight {
  conversation: string;
  /** The derived session id. Also the only clue `turnStarted` gives for "whose turn is this". */
  sessionId: string;
  tools: string[];
  /** This turn's id. Unknowable before `turnStarted`, hence nullable. */
  turnId: string | null;
  /** Set when a group message started this turn: an approval it asks for goes back there. */
  source?: InteractionSource;
}

/** A completed reply, waiting to be picked up by its own `turn/start` response. */
interface Completed {
  reply: string;
  model: string;
  iterations: number;
}

/**
 * Which turn a notification belongs to.
 *
 * Almost all notifications carry `turnId`; `error` doesn't — it may be a protocol-level error
 * (parse failure, unknown method), unrelated to any turn.
 */
function turnIdOf(notification: ServerNotification): string | null {
  return "turnId" in notification ? notification.turnId : null;
}

/**
 * One app-server, serving many people.
 *
 * Why not "one process per person": state.db allows a single writer; more processes fight over
 * the lock. So the gateway keeps concurrency outside — one queue per platform user, and queues
 * don't compete either, because the server's stdio loop is serial anyway (see stdio.rs).
 */
export class JeevesBridge {
  readonly #client: JeevesClient;
  readonly #onProgress: (note: ProgressNote) => void;

  /** One tail per conversation: the next message starts only after the previous one finishes. */
  readonly #tails = new Map<string, Promise<void>>();

  /**
   * Turns in flight, indexed by `turnId`.
   *
   * A single "current turn" slot won't do: the server's stdio loop is serial, but **two turns of
   * different conversations can be in flight at once** — while Alice's runs, Bob's request already
   * went out. Notifications carry only `turnId`; one slot would let Bob's turn claim Alice's.
   */
  readonly #inFlight = new Map<string, InFlight>();

  /** Turns past `turn/start`, awaiting `turnStarted`, by session — turnId still unknown here. */
  readonly #awaitingTurnId = new Map<string, InFlight>();

  /** Completed replies, likewise keyed by `turnId`. */
  readonly #completed = new Map<string, Completed>();

  constructor(options: JeevesBridgeOptions = {}) {
    if (options.client !== undefined) {
      this.#client = options.client;
    } else {
      this.#client = new JeevesClient({
        transport: options.url ? new HttpTransport({ url: options.url, token: options.token }) : new StdioTransport({
          command: options.command,
          args: options.args,
          cwd: options.cwd,
          env: options.env,
        }),
        onLog: options.onLog ?? ((text) => console.error(text)),
      });
    }

    this.#onProgress = options.onProgress ?? (() => {});
    this.#client.onNotification((notification) =>
      this.#handleNotification(notification),
    );
  }

  /**
   * One question in, one reply out.
   *
   * Two calls on the same conversation **always** run back to back — two messages fired quickly,
   * each running a full loop, would stomp context and return out of order. Queuing is the only job.
   */
  ask(
    conversation: string,
    text: string,
    source?: InteractionSource,
    requestId?: string,
  ): Promise<TurnOutcome> {
    const key = source === undefined ? conversation : MAIN_SESSION;
    const previous = this.#tails.get(key) ?? Promise.resolve();
    const turn = previous.then(() => this.#runTurn(conversation, text, source, requestId));

    const tail = turn.then(
      () => {},
      () => {},
    );
    this.#tails.set(key, tail);
    void tail.then(() => {
      if (this.#tails.get(key) === tail) this.#tails.delete(key);
    });

    return turn;
  }

  /** Task status changes, for reporting back to where a task was dispatched from. */
  onTaskUpdated(handler: (update: TaskUpdatedNotification) => void): () => void {
    return this.#client.onNotification((notification) => {
      if (notification.type === "taskUpdated") handler(notification);
    });
  }

  /** A turn (a dispatched one, or a task's) is waiting for a human yes/no. */
  onApprovalRequested(handler: (request: ApprovalRequestedNotification) => void): () => void {
    return this.#client.onNotification((notification) => {
      if (notification.type === "approvalRequested") handler(notification);
    });
  }

  /**
   * Where a waiting request came from. A dispatched turn still in flight is known here; a task's
   * request is looked up in the Runtime's live list for the main conversation, whose task view
   * carries the `source`. `null` when the request is no longer waiting.
   */
  async locateApproval(
    turnId: string,
    requestId: string,
  ): Promise<{ source: InteractionSource | null; task: AgentTaskView | null } | null> {
    const own = this.#inFlight.get(turnId)?.source;
    if (own !== undefined) return { source: own, task: null };
    const { approvals } = await this.#client.request(METHODS.APPROVAL_PENDING, {
      sessionId: MAIN_SESSION,
    });
    const found = approvals.find(
      (view) => view.request.turnId === turnId && view.request.requestId === requestId,
    );
    if (found === undefined) return null;
    return { source: found.task?.source ?? null, task: found.task ?? null };
  }

  /** Like `answerApproval`, but a refused answer also says who answered first (`answeredBy`). */
  async answerApprovalDetailed(
    turnId: string,
    requestId: string,
    approved: boolean,
    actor?: string,
    version?: number,
  ): Promise<{ accepted: boolean; answeredBy?: string | null }> {
    return this.#client.request(METHODS.APPROVAL_RESPOND, {
      turnId,
      requestId,
      approved,
      remember: "once",
      actor: actor ?? null,
      ...(version === undefined ? {} : { version }),
    });
  }

  /** Answer once (never a standing rule from a chat). `false` = too late, nobody heard it. */
  async answerApproval(
    turnId: string,
    requestId: string,
    approved: boolean,
    actor?: string,
    version?: number,
  ): Promise<boolean> {
    const { accepted } = await this.#client.request(METHODS.APPROVAL_RESPOND, {
      turnId,
      requestId,
      approved,
      remember: "once",
      actor: actor ?? null,
      ...(version === undefined ? {} : { version }),
    });
    return accepted;
  }

  /** An inbound channel message that may be the input a task is waiting for (`task/wake`). */
  async wake(params: TaskWakeParams): Promise<TaskWakeResponse> {
    return this.#client.request(METHODS.TASK_WAKE, params);
  }

  /** One task as the Runtime sees it (its `source` says where to report). */
  async task(taskId: string): Promise<AgentTaskView> {
    return (await this.#client.request(METHODS.TASK_GET, { taskId })).task;
  }

  /** Close app-server. It finishes writing queued frames before exiting. */
  close(): void {
    this.#client.close();
  }

  async #runTurn(
    conversation: string,
    text: string,
    source?: InteractionSource,
    requestId?: string,
  ): Promise<TurnOutcome> {
    const sessionId = source === undefined ? sessionIdFor(conversation) : MAIN_SESSION;
    const current: InFlight = {
      conversation,
      sessionId,
      tools: [],
      turnId: null,
      ...(source === undefined ? {} : { source }),
    };
    this.#awaitingTurnId.set(sessionId, current);

    try {
      const { turnId } =
        source === undefined
          ? await this.#client.request(METHODS.TURN_START, {
              sessionId,
              message: text,
              stream: false,
              ...(requestId === undefined ? {} : { requestId: safeId(requestId) }),
            })
          : await this.#startInteraction(text, source);

      const completed = this.#completed.get(turnId);
      if (completed === undefined) {
        throw new Error(
          `this turn never saw turnCompleted (turnId=${turnId}) — the server's notifications and answers do not line up`,
        );
      }

      return {
        sessionId,
        reply: completed.reply,
        tools: current.tools,
        model: completed.model,
        iterations: completed.iterations,
      };
    } finally {
      this.#forget(current);
    }
  }

  /** Hand a dispatcher's group message to the main conversation, which may create tasks. */
  async #startInteraction(text: string, source: InteractionSource) {
    if (!this.#mainOpened) {
      await this.#client.request(METHODS.SESSION_MAIN, null);
      this.#mainOpened = true;
    }
    return this.#client.request(METHODS.INTERACTION_START, {
      sessionId: MAIN_SESSION,
      messageId: safeId(source.messageId),
      message: text,
      taskId: null,
      cwd: null,
      attachments: null,
      source,
    });
  }

  #mainOpened = false;

  /** Turn wrap-up: erase every trace it left in the tables. */
  #forget(current: InFlight): void {
    this.#awaitingTurnId.delete(current.sessionId);
    if (current.turnId === null) return;
    this.#inFlight.delete(current.turnId);
    this.#completed.delete(current.turnId);
  }

  #handleNotification(notification: ServerNotification): void {
    if (notification.type === "turnStarted") {
      const waiting = this.#awaitingTurnId.get(notification.sessionId);
      if (waiting === undefined) return;
      waiting.turnId = notification.turnId;
      this.#inFlight.set(notification.turnId, waiting);
      return;
    }

    const turnId = turnIdOf(notification);
    if (turnId === null) return;
    const current = this.#inFlight.get(turnId);
    if (current === undefined) return;

    const note = progressText(notification);
    if (note !== null) {
      this.#onProgress({ conversation: current.conversation, text: note });
    }

    switch (notification.type) {
      case "toolStarted":
        current.tools.push(notification.tool);
        return;
      case "turnCompleted":
        this.#completed.set(turnId, {
          reply: notification.reply,
          model: notification.meta.model,
          iterations: notification.meta.iterations,
        });
        this.#inFlight.delete(turnId);
        return;
      default:
        return;
    }
  }
}
