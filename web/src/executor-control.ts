import { reactive } from "vue";
import { preferenceRequest } from "./api.ts";
import { t } from "./i18n.ts";
import type { ExecutorControlView, ExecutorReleaseParams } from "./protocol.ts";

export const executor = reactive({
  view: null as ExecutorControlView | null,
  busy: false,
  readFailed: false,
  message: "",
});

let revision = 0;
let reading: number | undefined;
export async function refreshExecutor(): Promise<void> {
  if (executor.busy || reading === revision) return;
  const request = ++revision;
  reading = request;
  try {
    const view = await preferenceRequest<ExecutorControlView>(
      "/api/executor/control",
    );
    if (request === revision) {
      executor.view = view;
      executor.readFailed = false;
    }
  } catch {
    if (request === revision) {
      executor.view = null;
      executor.readFailed = true;
    }
  } finally {
    if (reading === request) reading = undefined;
  }
}

export async function changeExecutor(
  action: "stop" | "release",
): Promise<void> {
  if (executor.busy) return;
  let body = {};
  if (action === "release") {
    if (!executor.view?.stopped) return;
    body = {
      expectedGeneration: executor.view.generation,
    } satisfies ExecutorReleaseParams;
  }
  executor.busy = true;
  executor.message = "";
  ++revision;
  let failed = false;
  try {
    executor.view = await preferenceRequest<ExecutorControlView>(
      `/api/executor/${action}`,
      body,
    );
    executor.readFailed = false;
  } catch (error) {
    executor.message = t("操作结果待核对：{error}", { error: String(error) });
    executor.view = null;
    failed = true;
  } finally {
    executor.busy = false;
  }
  if (failed) await refreshExecutor();
}
