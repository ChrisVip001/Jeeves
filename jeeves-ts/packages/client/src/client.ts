import type * as V2 from "./protocol.ts";
import { CODES, METHODS } from "./protocol.ts";
import type {
  ApprovalRespondResponse,
  ApprovalRespondParams,
  ApprovalPendingParams,
  ApprovalPendingResponse,
  ConfigReadParams,
  ConfigReadResponse,
  ConfigWriteParams,
  ConfigWriteResponse,
  ErrorObject,
  JsonRpcError,
  JsonRpcMessage,
  JsonRpcNotification,
  MemoryForgetParams,
  MemoryForgetResponse,
  MemoryListParams,
  MemoryListResponse,
  MemoryValidityParams,
  MemoryValidityResponse,
  MemoryProvenanceParams,
  MemoryProvenanceResponse,
  MemoryRememberParams,
  MemoryRememberResponse,
  MemorySearchParams,
  MemorySearchResponse,
  ModelListParams,
  ModelListResponse,
  ServerNotification,
  SessionListParams,
  SessionListResponse,
  SessionPermissionGetParams,
  SessionPermissionSetParams,
  SessionPermissionResponse,
  SessionNewParams,
  SessionNewResponse,
  TurnInterruptParams,
  TurnInterruptResponse,
  TurnStartParams,
  InteractionStartParams,
  TurnStartResponse,
  TaskStartParams,
  TaskGetParams,
  TaskOperationsResponse,
  TaskReconcileParams,
  TurnOperationsParams,
  TurnReconcileParams,
  TaskWaitParams,
  TaskSignalParams,
  TaskSignalResponse,
  TaskEnvelope,
  TaskEventsParams,
  TaskWakeParams,
  TaskWakeResponse,
  TaskEventsResponse,
} from "./protocol.ts";
import type { Transport } from "./transport.ts";

/**
 * The table of "which method takes what and returns what".
 *
 * Only the **pairing** is handwritten here; the shapes all come from the generated artifacts —
 * the Rust method name is a runtime string constant with no type-level link to the `*Params`
 * types, so the generator can't derive it and it must be written once here.
 *
 * What can't be gotten wrong:
 * - Method names use generated computed names (`METHODS.*`); a Rust rename breaks compile here.
 * - Params/results reference generated types; renamed fields follow automatically.
 * The only drift left is "mispairing" — tsc reports it at the call site, because shapes clash.
 */
export interface Methods {
  [METHODS.DEVICE_LIST]: { params: null; result: V2.DeviceListResponse };
  [METHODS.DEVICE_PAIR]: {
    params: V2.DevicePairParams;
    result: V2.DeviceGrantView;
  };
  [METHODS.DEVICE_REVOKE]: {
    params: V2.DeviceRevokeParams;
    result: V2.DeviceGrantView;
  };
  [METHODS.DEVICE_REPORT]: {
    params: V2.DeviceReportParams;
    result: V2.DeviceReportResponse;
  };
  [METHODS.DESKTOP_STATUS]: { params: V2.TaskGetParams; result: V2.DesktopStatus };
  [METHODS.DESKTOP_SELECT]: { params: V2.TaskGetParams; result: V2.DesktopObservation };
  [METHODS.DESKTOP_OBSERVE]: { params: V2.TaskGetParams; result: V2.DesktopObservation };
  [METHODS.DESKTOP_ACT]: { params: V2.DesktopActParams; result: V2.DesktopObservation };
  [METHODS.DESKTOP_TAKEOVER]: { params: V2.TaskGetParams; result: V2.DesktopStatus };
  [METHODS.DESKTOP_MODEL]: { params: V2.DesktopModelParams; result: V2.DesktopStatus };
  [METHODS.DESKTOP_FRAME]: { params: V2.DesktopFrameParams; result: V2.DesktopFrame };
  [METHODS.DESKTOP_SHARE]: { params: V2.DesktopShareParams; result: V2.DesktopAdvice };
  [METHODS.EXECUTOR_CONTROL]: { params: null; result: V2.ExecutorControlView };
  [METHODS.EXECUTOR_STOP]: { params: null; result: V2.ExecutorControlView };
  [METHODS.EXECUTOR_RELEASE]: {
    params: V2.ExecutorReleaseParams;
    result: V2.ExecutorControlView;
  };
  [METHODS.BROWSER_LIST]: {
    params: V2.BrowserListParams;
    result: V2.BrowserListResponse;
  };
  [METHODS.BROWSER_START]: {
    params: V2.BrowserStartParams;
    result: V2.BrowserSessionView;
  };
  [METHODS.BROWSER_INPUT]: { params: V2.BrowserHumanInputParams; result: V2.BrowserFrameResponse };
  [METHODS.BROWSER_DOWNLOADS]: { params: V2.TaskGetParams; result: V2.BrowserDownloadsResponse };
  [METHODS.BROWSER_DOWNLOAD]: { params: V2.BrowserDownloadParams; result: V2.BrowserDownloadsResponse };
  [METHODS.BROWSER_CONTROL]: {
    params: V2.BrowserControlParams;
    result: V2.BrowserSessionView;
  };
  [METHODS.BROWSER_FRAME]: {
    params: V2.TaskGetParams;
    result: V2.BrowserFrameResponse;
  };
  [METHODS.APPROVAL_PENDING]: {
    params: ApprovalPendingParams;
    result: ApprovalPendingResponse;
  };
  [METHODS.SESSION_MAIN]: { params: null; result: SessionNewResponse };
  [METHODS.INTERACTION_START]: {
    params: InteractionStartParams;
    result: TurnStartResponse;
  };
  [METHODS.SESSION_PERMISSION_GET]: {
    params: SessionPermissionGetParams;
    result: SessionPermissionResponse;
  };
  [METHODS.SESSION_PERMISSION_SET]: {
    params: SessionPermissionSetParams;
    result: SessionPermissionResponse;
  };
  [METHODS.TURN_OPERATIONS]: {
    params: TurnOperationsParams;
    result: TaskOperationsResponse;
  };
  [METHODS.TURN_RECONCILE]: {
    params: TurnReconcileParams;
    result: TaskOperationsResponse;
  };
  [METHODS.TASK_GET]: { params: TaskGetParams; result: TaskEnvelope };
  [METHODS.TASK_START]: { params: TaskStartParams; result: TaskEnvelope };
  [METHODS.TASK_OPERATIONS]: {
    params: TaskGetParams;
    result: TaskOperationsResponse;
  };
  [METHODS.TASK_RECONCILE]: {
    params: TaskReconcileParams;
    result: TaskOperationsResponse;
  };
  [METHODS.TASK_EVENTS]: {
    params: TaskEventsParams;
    result: TaskEventsResponse;
  };
  [METHODS.TASK_WAIT]: { params: TaskWaitParams; result: TaskEnvelope };
  [METHODS.TASK_WAKE]: { params: TaskWakeParams; result: TaskWakeResponse };
  [METHODS.TASK_SIGNAL]: {
    params: TaskSignalParams;
    result: TaskSignalResponse;
  };
  [METHODS.TURN_START]: { params: TurnStartParams; result: TurnStartResponse };
  [METHODS.TURN_INTERRUPT]: {
    params: TurnInterruptParams;
    result: TurnInterruptResponse;
  };
  [METHODS.APPROVAL_RESPOND]: {
    params: ApprovalRespondParams;
    result: ApprovalRespondResponse;
  };
  [METHODS.SESSION_LIST]: {
    params: SessionListParams;
    result: SessionListResponse;
  };
  [METHODS.SESSION_NEW]: {
    params: SessionNewParams;
    result: SessionNewResponse;
  };
  [METHODS.MEMORY_SEARCH]: {
    params: MemorySearchParams;
    result: MemorySearchResponse;
  };
  [METHODS.MEMORY_LIST]: {
    params: MemoryListParams;
    result: MemoryListResponse;
  };
  [METHODS.MEMORY_VALIDITY]: {
    params: MemoryValidityParams;
    result: MemoryValidityResponse;
  };
  [METHODS.MEMORY_PROVENANCE]: {
    params: MemoryProvenanceParams;
    result: MemoryProvenanceResponse;
  };
  [METHODS.MEMORY_REMEMBER]: {
    params: MemoryRememberParams;
    result: MemoryRememberResponse;
  };
  [METHODS.MEMORY_FORGET]: {
    params: MemoryForgetParams;
    result: MemoryForgetResponse;
  };
  [METHODS.CONFIG_READ]: {
    params: ConfigReadParams;
    result: ConfigReadResponse;
  };
  [METHODS.CONFIG_WRITE]: {
    params: ConfigWriteParams;
    result: ConfigWriteResponse;
  };
  [METHODS.MODEL_LIST]: { params: ModelListParams; result: ModelListResponse };
}

export type MethodName = keyof Methods;

/**
 * The error the server returned, restored from the JSON-RPC error object.
 *
 * Carrying `code` rather than just a message, because callers **really branch on it**:
 * `PROVIDER_ERROR` should tell the user to configure a key; `NOT_IMPLEMENTED` means "not built
 * yet", not "you called it wrong" — two different things to a user (see the comment in rpc.rs).
 */
export class JeevesError extends Error {
  readonly code: number;
  readonly data: unknown;

  constructor(object: ErrorObject) {
    super(object.message);
    this.name = "JeevesError";
    this.code = object.code;
    this.data = object.data ?? null;
  }

  /** Bad key, rate limited, model missing… all in this class. */
  get isProvider(): boolean {
    return this.code === CODES.PROVIDER_ERROR;
  }

  /** In the protocol but not built in this phase. */
  get isNotImplemented(): boolean {
    return this.code === CODES.NOT_IMPLEMENTED;
  }

  /** Wrong param shape — most likely a version mismatch with the server. */
  get isInvalidParams(): boolean {
    return this.code === CODES.INVALID_PARAMS;
  }
}

/** Notifications all ride under this method name on the wire; the real discriminant is `params.type`. */
const NOTIFICATION_METHOD = "turn/notification";

export interface JeevesClientOptions {
  transport: Transport;
  /**
   * The server's human words (stderr) and unparseable frames. Defaults to `console.error`:
   * these deserve to be seen but must not interrupt flow — swallowing them is the real bug.
   */
  onLog?: (text: string) => void;
}

interface Pending {
  resolve: (result: unknown) => void;
  reject: (error: Error) => void;
  method: string;
}

/**
 * The app-server client.
 *
 * It does exactly three things, nothing more:
 * 1. Attach ids to requests and return each reply to the asking `await`, by id;
 * 2. Hand notifications to subscribers as-is — **no interpreting, caching, or replay**: a turn's
 *    process events belong to the UI of the moment; remembering them only duplicates state.
 * 3. The moment the process dies, reject all pending requests at once — callers would otherwise
 *    wait forever, and "waiting forever" is the hardest kind of bug to find.
 */
export class JeevesClient {
  readonly #transport: Transport;
  readonly #onLog: (text: string) => void;
  /** Keys are normalized id strings — see `asId`. */
  readonly #pending = new Map<string, Pending>();
  readonly #notificationHandlers = new Set<
    (notification: ServerNotification) => void
  >();
  #nextId = 1;
  #closedReason: string | null = null;

  constructor(options: JeevesClientOptions) {
    this.#transport = options.transport;
    this.#onLog = options.onLog ?? ((text) => console.error(text));

    this.#transport.onLine((line) => this.#handleLine(line));
    this.#transport.onLog((text) => this.#onLog(text));
    this.#transport.onClose((reason) => this.#handleClose(reason));
  }

  /**
   * Call a method and wait for its reply.
   *
   * Params are required — even "nothing to pass" must be `{}`. An optional second parameter
   * makes "forgot to pass" and "passed empty" look identical; in this protocol they often aren't.
   */
  request<M extends MethodName>(
    method: M,
    params: Methods[M]["params"],
  ): Promise<Methods[M]["result"]> {
    if (this.#closedReason !== null) {
      return Promise.reject(
        new Error(`the connection is gone: ${this.#closedReason}`),
      );
    }

    const id = this.#nextId;
    this.#nextId += 1;

    return new Promise<Methods[M]["result"]>((resolve, reject) => {
      this.#pending.set(asId(id), {
        resolve: resolve as (result: unknown) => void,
        reject,
        method,
      });
      this.#transport.write(
        JSON.stringify({ jsonrpc: "2.0", id, method, params }),
      );
    });
  }

  /** Subscribe to process events. Returns an unsubscribe function. */
  onNotification(
    handler: (notification: ServerNotification) => void,
  ): () => void {
    this.#notificationHandlers.add(handler);
    return () => {
      this.#notificationHandlers.delete(handler);
    };
  }

  /** Civilized close: stop accepting new requests, wait for queued frames to flush. */
  close(): void {
    if (this.#closedReason !== null) return;
    this.#closedReason = "the client closed it";
    this.#transport.close();
    this.#rejectAll(this.#closedReason);
  }

  #handleLine(line: string): void {
    let frame: JsonRpcMessage | JsonRpcNotification;
    try {
      frame = JSON.parse(line) as JsonRpcMessage | JsonRpcNotification;
    } catch {
      this.#onLog(`this frame cannot be read; pasted as it came: ${line}`);
      return;
    }

    if ("method" in frame) {
      this.#handleNotification(frame);
      return;
    }

    if ("error" in frame) {
      this.#handleError(frame);
      return;
    }

    const key = asId(frame.id);
    const pending = this.#pending.get(key);
    if (pending === undefined) {
      this.#onLog(
        `nobody is waiting for response id ${JSON.stringify(frame.id)}`,
      );
      return;
    }
    this.#pending.delete(key);
    pending.resolve(frame.result);
  }

  #handleNotification(frame: JsonRpcNotification): void {
    if (frame.method !== NOTIFICATION_METHOD) {
      this.#onLog(`unknown notification method: ${frame.method}`);
      return;
    }
    const notification = frame.params as ServerNotification | null | undefined;
    if (notification == null || typeof notification !== "object") {
      this.#onLog(`the notification carries nothing: ${JSON.stringify(frame)}`);
      return;
    }
    for (const handler of this.#notificationHandlers) handler(notification);
  }

  #handleError(frame: JsonRpcError): void {
    if (frame.id === null) {
      this.#onLog(frame.error.message);
      return;
    }

    const id = asId(frame.id);
    const pending = this.#pending.get(id);
    if (pending === undefined) {
      this.#onLog(`nobody is waiting for the error id: ${frame.error.message}`);
      return;
    }
    this.#pending.delete(id);
    pending.reject(new JeevesError(frame.error));
  }

  #handleClose(reason: string): void {
    if (this.#closedReason === null) this.#closedReason = reason;
    this.#rejectAll(reason);
  }

  #rejectAll(reason: string): void {
    const pending = [...this.#pending.values()];
    this.#pending.clear();
    for (const entry of pending) {
      entry.reject(
        new Error(
          `${entry.method} never got its response; the connection dropped: ${reason}`,
        ),
      );
    }
  }
}

/**
 * Normalize a frame's id into a Map key.
 *
 * A JSON-RPC id may be a number or a string (that's how `RequestId` is defined); both must pair
 * back through the same path — so store as strings, never guessing types.
 *
 * Both writes and reads **must** go through here: `Map` doesn't convert `1` and `"1"`; a numeric
 * key looked up as a string fails silently — and that `await` hangs forever.
 */
function asId(id: number | string): string {
  return String(id);
}
