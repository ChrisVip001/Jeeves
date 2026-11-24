import { reactive } from "vue";
import type { PermissionMode, SessionPermissionResponse } from "./types";

export interface PermissionActions {
  read(sessionId: string): Promise<SessionPermissionResponse>;
  write(
    sessionId: string,
    mode: PermissionMode,
    expectedRevision: number,
  ): Promise<SessionPermissionResponse>;
}

/**
 * The session's permission preset, as the switch on an approval card needs it. Presentation state
 * only: the runtime owns what each mode actually allows, and a mode is only ever written from a
 * human control, never on the model's behalf.
 */
export function usePermission(actions: PermissionActions) {
  const state = reactive({
    sessionId: "",
    mode: null as PermissionMode | null,
    revision: 0,
    loaded: false,
    busy: false,
    error: "",
  });
  function adopt(policy: SessionPermissionResponse) {
    state.sessionId = policy.sessionId;
    state.mode = policy.mode;
    state.revision = policy.revision;
    state.loaded = true;
  }
  async function load(sessionId: string): Promise<void> {
    if (!sessionId) return;
    try {
      const policy = await actions.read(sessionId);
      if (policy.sessionId === sessionId) adopt(policy);
    } catch {
      state.loaded = false;
    }
  }
  /**
   * Hand the session full access. Re-reads the revision first, so a preset changed in another
   * window loses the race instead of being silently overwritten.
   */
  async function trust(sessionId: string): Promise<boolean> {
    if (state.busy || !sessionId) return false;
    state.busy = true;
    state.error = "";
    try {
      const current = await actions.read(sessionId);
      const policy = await actions.write(
        sessionId,
        "fullAccess",
        current.revision,
      );
      adopt(policy);
      return policy.mode === "fullAccess";
    } catch (error) {
      state.error = String(error);
      return false;
    } finally {
      state.busy = false;
    }
  }
  /** A preset changed elsewhere arrives as a notification; keep the switch in step with it. */
  function changed(policy: SessionPermissionResponse): void {
    if (!state.sessionId || policy.sessionId === state.sessionId) adopt(policy);
  }
  return { state, load, trust, changed };
}
export type SessionPermission = ReturnType<typeof usePermission>;
export type { PermissionMode };
