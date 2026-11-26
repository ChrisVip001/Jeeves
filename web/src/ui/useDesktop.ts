import { reactive } from "vue";
import { errorText } from "../api.ts";
import type {
  DesktopAction,
  DesktopActParams,
  DesktopAdvice,
  DesktopFrame,
  DesktopFrameParams,
  DesktopModelParams,
  DesktopObservation,
  DesktopShareParams,
  DesktopStatus,
} from "../protocol.ts";

type Request = <T>(path: string, body?: unknown) => Promise<T>;

/** Only the currently displayed observation and frame can be used from this panel. */
export function useDesktop(taskId: string, request: Request) {
  const state = reactive({
    status: null as DesktopStatus | null,
    observation: null as DesktopObservation | null,
    frame: null as DesktopFrame | null,
    advice: null as DesktopAdvice | null,
    busy: false,
    error: "",
  });
  let revision = 0;
  const status = () =>
    request<DesktopStatus>(
      `/api/desktop/status?taskId=${encodeURIComponent(taskId)}`,
    );
  function clearPreview() {
    state.frame = null;
    state.advice = null;
  }
  function invalidate() {
    ++revision;
    state.busy = false;
    state.observation = null;
    state.status = null;
    state.error = "";
    clearPreview();
  }
  async function run<T>(
    read: () => Promise<T>,
    apply: (result: T) => void,
    reconcile = false,
  ) {
    if (state.busy) return;
    const version = ++revision;
    state.busy = true;
    state.error = "";
    try {
      const result = await read();
      if (version === revision) apply(result);
    } catch (error) {
      if (version !== revision) return;
      state.observation = null;
      state.status = null;
      clearPreview();
      state.error = errorText(error);
      if (reconcile) {
        try {
          const current = await status();
          if (version === revision) state.status = current;
        } catch {
        }
      }
    } finally {
      if (version === revision) state.busy = false;
    }
  }
  function refresh() {
    return run(status, (result) => {
      state.status = result;
    });
  }
  async function observe(
    action: "select" | "observe" | "act",
    params: { taskId: string } | DesktopActParams,
  ) {
    if (state.busy) return;
    state.observation = null;
    clearPreview();
    await run(
      async () => {
        const observation = await request<DesktopObservation>(
          `/api/desktop/${action}`,
          params,
        );
        return { observation, current: await status() };
      },
      ({ observation, current }) => {
        state.status = current;
        if (current.selected) state.observation = observation;
      },
      action !== "observe",
    );
  }
  return {
    state,
    refresh,
    invalidate,
    clearPreview,
    select: () => observe("select", { taskId }),
    observe: () => observe("observe", { taskId }),
    act(elementId: number, action: DesktopAction, text?: string) {
      const observation = state.observation;
      if (!observation) return;
      return observe("act", {
        taskId,
        observationId: observation.observationId,
        elementId,
        action,
        text,
        lease: observation.lease ?? state.status?.lease,
      });
    },
    takeover() {
      invalidate();
      return run(
        () => request<DesktopStatus>("/api/desktop/takeover", { taskId }),
        (current) => {
          state.status = current;
        },
      );
    },
    model(profileId: string | null) {
      clearPreview();
      return run(
        () =>
          request<DesktopStatus>("/api/desktop/model", {
            taskId,
            profileId,
          } satisfies DesktopModelParams),
        (current) => {
          state.status = current;
          if (!current.selected) state.observation = null;
        },
      );
    },
    capture(localOnly = false) {
      const observationId = state.observation?.observationId;
      if (!observationId) return;
      clearPreview();
      return run(
        () =>
          request<DesktopFrame>("/api/desktop/frame", {
            taskId,
            observationId,
            localOnly,
            lease: state.observation?.lease ?? state.status?.lease,
          } satisfies DesktopFrameParams),
        (frame) => {
          state.frame = frame;
        },
      );
    },
    share() {
      const frame = state.frame;
      if (!frame?.destination) return;
      const lease = state.observation?.lease ?? state.status?.lease;
      clearPreview();
      state.observation = null;
      return run(
        () =>
          request<DesktopAdvice>("/api/desktop/share", {
            taskId,
            frameId: frame.frameId,
            confirmed: true,
            lease,
          } satisfies DesktopShareParams),
        (advice) => {
          state.advice = advice;
        },
      );
    },
  };
}
