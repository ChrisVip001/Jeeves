import { reactive } from "vue";
import type * as V2 from "../../../protocol/schema/typescript/v2/index";
export interface BrowserActions {
  list(sessionId: string): Promise<V2.BrowserListResponse>;
  start(params: V2.BrowserStartParams): Promise<V2.BrowserSessionView>;
  control(params: V2.BrowserControlParams): Promise<V2.BrowserSessionView>;
  frame(taskId: string): Promise<V2.BrowserFrameResponse>;
  input(params: V2.BrowserHumanInputParams): Promise<V2.BrowserFrameResponse>;
  downloads(taskId: string): Promise<V2.BrowserDownloadsResponse>;
  download(
    params: V2.BrowserDownloadParams,
  ): Promise<V2.BrowserDownloadsResponse>;
}
/** The human-facing text of a thrown error, without the `ApiError:` class-name prefix String() adds. */
function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
/** Presentation only; scope, permission and control fences belong to the Runtime. */
export function useBrowsers(actions: BrowserActions) {
  const state = reactive({
    owner: "",
    available: false,
    sessions: [] as V2.BrowserSessionView[],
    busy: {} as Record<string, boolean>,
    errors: {} as Record<string, string>,
    inputFailed: {} as Record<string, boolean>,
    downloads: {} as Record<string, V2.BrowserDownloadsResponse["downloads"]>,
    frames: {} as Record<string, V2.BrowserFrameResponse>,
  });
  let version = 0;
  let ownerVersion = 0;
  let refreshing = false;
  async function refresh(owner: string) {
    if (owner !== state.owner) {
      version++;
      ownerVersion++;
      state.owner = owner;
      state.available = false;
      state.sessions = [];
      state.errors = {};
      state.frames = {};
      state.downloads = {};
      state.inputFailed = {};
      state.busy = {};
    }
    if (!owner || refreshing) return;
    refreshing = true;
    const epoch = version;
    try {
      const result = await actions.list(owner);
      if (owner === state.owner && epoch === version) {
        state.available = result.available;
        for (const prior of state.sessions) {
          const next = result.sessions.find(
            (row) => row.taskId === prior.taskId,
          );
          if (
            next?.id !== prior.id ||
            next.generation !== prior.generation ||
            next.status !== prior.status
          )
            delete state.frames[prior.taskId];
          if (
            next?.id !== prior.id ||
            next.status === "closed" ||
            next.status === "needsAttention"
          )
            delete state.downloads[prior.taskId];
        }
        state.sessions = result.sessions;
        delete state.errors.list;
      }
    } catch (error) {
      if (owner === state.owner && epoch === version)
        state.errors.list = message(error);
    } finally {
      refreshing = false;
    }
  }
  async function run(
    taskId: string,
    work: () => Promise<
      | V2.BrowserSessionView
      | V2.BrowserFrameResponse
      | V2.BrowserDownloadsResponse
    >,
    input = false,
  ) {
    if (state.busy[taskId]) return;
    const owner = state.owner;
    const ownerEpoch = ownerVersion;
    const initial = state.sessions.find((row) => row.taskId === taskId);
    version++;
    state.busy[taskId] = true;
    delete state.errors[taskId];
    if (input) delete state.inputFailed[taskId];
    try {
      const result = await work();
      if (owner !== state.owner || ownerEpoch !== ownerVersion) return;
      const current = state.sessions.find((row) => row.taskId === taskId);
      if ("dataUrl" in result || "downloads" in result) {
        if (
          !current ||
          current.id !== initial?.id ||
          current.generation !== initial.generation ||
          current.status !== initial.status
        )
          return;
        if ("dataUrl" in result) {
          state.frames[taskId] = result;
          delete state.inputFailed[taskId];
        } else state.downloads[taskId] = result.downloads;
      } else {
        state.sessions = state.sessions
          .filter((row) => row.taskId !== taskId)
          .concat(result);
        delete state.frames[taskId];
      }
    } catch (error) {
      if (owner === state.owner && ownerEpoch === ownerVersion) {
        if (input) {
          delete state.frames[taskId];
          state.inputFailed[taskId] = true;
        } else state.errors[taskId] = message(error);
      }
    } finally {
      if (owner === state.owner && ownerEpoch === ownerVersion) {
        version++;
        state.busy[taskId] = false;
        void refresh(owner);
      }
    }
  }
  return {
    state,
    refresh,
    start: (params: V2.BrowserStartParams) =>
      run(params.taskId, () => actions.start(params)),
    control: (
      row: V2.BrowserSessionView,
      action: V2.BrowserControlAction,
      origin: string | null = null,
    ) =>
      run(row.taskId, () =>
        actions.control({
          taskId: row.taskId,
          generation: row.generation,
          action,
          origin,
        }),
      ),
    frame: (taskId: string) => run(taskId, () => actions.frame(taskId)),
    input: (
      row: V2.BrowserSessionView,
      frameId: string,
      input: V2.BrowserHumanInputParams["input"],
    ) => {
      if (
        row.status !== "human" ||
        !frameId ||
        frameId !== state.frames[row.taskId]?.frameId
      )
        return;
      return run(
        row.taskId,
        () =>
          actions.input({
            taskId: row.taskId,
            generation: row.generation,
            frameId,
            input,
          }),
        true,
      );
    },
    downloads: (taskId: string) => run(taskId, () => actions.downloads(taskId)),
    download: (
      row: V2.BrowserSessionView,
      downloadId: string,
      action: V2.BrowserDownloadParams["action"],
    ) =>
      run(row.taskId, () =>
        actions.download({
          taskId: row.taskId,
          generation: row.generation,
          downloadId,
          action,
        }),
      ),
  };
}
export type BrowserCenter = ReturnType<typeof useBrowsers>;
