import assert from "node:assert/strict";
import { test } from "node:test";
import type { DesktopObservation, DesktopStatus } from "../src/protocol.ts";
import { useDesktop } from "../src/ui/useDesktop.ts";

const observation: DesktopObservation = {
  observationId: "first",
  window: "Notes",
  elements: [{ id: 7, label: "Title", actions: ["setText"] }],
};
const selected: DesktopStatus = {
  available: true,
  selected: true,
  screenProfile: "local",
  receipts: [],
};
function fixture(responses: Record<string, (body: unknown) => unknown>) {
  const calls: Array<{ path: string; body: unknown }> = [];
  const panel = useDesktop("task", async <T>(path: string, body?: unknown) => {
    calls.push({ path, body });
    const op = path.substring("/api/desktop/".length).split("?")[0];
    if (!(op in responses)) throw Error(`Unexpected request ${op}`);
    return (await responses[op](body)) as T;
  });
  return { panel, calls };
}
test("a local preview requires a separate one-shot authorization for the displayed frame", async () => {
  const { panel, calls } = fixture({
    select: () => observation,
    status: () => selected,
    frame: () => ({
      frameId: "image-1",
      dataUrl: "data:image/png;base64,fixture",
      destination: "local · model",
    }),
    share: () => ({ text: "Check the title." }),
  });
  await panel.select();
  await panel.capture();
  assert.deepEqual(
    calls.map((c) => c.path.split("/").at(-1)),
    ["select", "status?taskId=task", "frame"],
  );
  await Promise.all([panel.share(), panel.share()]);
  assert.deepEqual(calls.at(-1)?.body, {
    taskId: "task",
    frameId: "image-1",
    confirmed: true,
  });
  assert.equal(calls.filter((c) => c.path.endsWith("/share")).length, 1);
  assert.equal(panel.state.frame, null);
  assert.equal(panel.state.advice, "Check the title.");
  assert.equal(panel.state.observation, null);
});

test("uncertain input is not repeated and its receipt is fetched without showing the input", async () => {
  let acted = false;
  const { panel, calls } = fixture({
    select: () => observation,
    status: () =>
      acted
        ? {
            ...selected,
            receipts: [{ id: "1", action: "setText", status: "unknown" }],
          }
        : selected,
    act: () => {
      acted = true;
      throw Error("Window response lost");
    },
  });
  await panel.select();
  await panel.act(7, "setText", "fixture-private");
  await panel.act(7, "setText", "fixture-private");
  assert.equal(panel.state.observation, null);
  assert.equal(panel.state.status?.receipts[0].status, "unknown");
  assert.equal(panel.state.error, "Window response lost");
  assert.equal(calls.filter((c) => c.path.endsWith("/act")).length, 1);
  assert.deepEqual(calls.find((c) => c.path.endsWith("/act"))?.body, {
    taskId: "task",
    observationId: "first",
    elementId: 7,
    action: "setText",
    text: "fixture-private",
  });
  assert.ok(!JSON.stringify(panel.state).includes("fixture-private"));
});

test("an in-flight window image cannot reappear after takeover or global invalidation", async () => {
  for (const action of ["takeover", "invalidate"] as const) {
    const image = Promise.withResolvers<unknown>();
    const { panel, calls } = fixture({
      select: () => observation,
      status: () => selected,
      frame: () => image.promise,
      takeover: () => ({ ...selected, selected: false }),
    });
    await panel.select();
    const pending = panel.capture();
    await panel[action]();
    image.resolve({
      frameId: "stale",
      dataUrl: "fixture",
      destination: "local",
    });
    await pending;
    await panel.share();
    assert.equal(panel.state.frame, null);
    assert.equal(panel.state.observation, null);
    assert.ok(!calls.some((c) => c.path.endsWith("/share")));
  }
});

test("changing the screen profile discards the preview made for the earlier destination", async () => {
  const { panel, calls } = fixture({
    select: () => observation,
    status: () => selected,
    model: () => ({ ...selected, screenProfile: "another" }),
    frame: () => ({
      frameId: "image",
      dataUrl: "fixture",
      destination: "local",
    }),
  });
  await panel.select();
  await panel.capture();
  await panel.model("another");
  await panel.share();
  assert.equal(panel.state.frame, null);
  assert.equal(panel.state.status?.screenProfile, "another");
  assert.deepEqual(calls.at(-1)?.body, {
    taskId: "task",
    profileId: "another",
  });
  assert.ok(!calls.some((c) => c.path.endsWith("/share")));
});
