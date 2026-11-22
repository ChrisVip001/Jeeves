import assert from "node:assert/strict";
import { test } from "node:test";
import { useBrowsers, type BrowserActions } from "../src/ui/useBrowsers.ts";
import type * as V2 from "../src/protocol.ts";

const session: V2.BrowserSessionView = {
  id: "browser",
  taskId: "task",
  status: "human",
  generation: 2,
  origins: ["https://example.test"],
  url: "https://example.test",
  title: "Report",
  observedAt: null,
  error: null,
};
const picture = {
  dataUrl: "data:image/jpeg;base64,fixture",
  observedAt: "2026-11-20T10:00:00Z",
  frameId: "first",
};
function fixture(overrides: Partial<BrowserActions> = {}) {
  return useBrowsers({
    list: async () => ({ available: true, sessions: [session] }),
    start: async () => session,
    control: async () => session,
    frame: async () => picture,
    input: async () => ({ ...picture, frameId: "next" }),
    downloads: async () => ({ downloads: [] }),
    download: async () => ({ downloads: [] }),
    ...overrides,
  });
}

test("human input uses the displayed frame only once and renews it from the receipt", async () => {
  const requests: V2.BrowserHumanInputParams[] = [];
  const center = fixture({
    input: async (p) => {
      requests.push(p);
      return { ...picture, frameId: "next" };
    },
  });
  await center.refresh("owner");
  await center.frame("task");
  await center.input(session, "first", { kind: "click", x: 65, y: 90 });
  await center.input(session, "first", { kind: "key", key: "enter" });
  assert.deepEqual(requests, [
    {
      taskId: "task",
      generation: 2,
      frameId: "first",
      input: { kind: "click", x: 65, y: 90 },
    },
  ]);
  assert.equal(center.state.frames.task.frameId, "next");
});

test("an uncertain input is not repeated or reflected and a new frame is required", async () => {
  let count = 0;
  const center = fixture({
    input: async (p) => {
      count++;
      throw Error(JSON.stringify(p.input));
    },
  });
  await center.refresh("owner");
  await center.frame("task");
  await center.input(session, "first", {
    kind: "text",
    text: "fixture-private-value",
  });
  await center.input(session, "first", {
    kind: "text",
    text: "fixture-private-value",
  });
  assert.equal(count, 1);
  assert.equal(center.state.inputFailed.task, true);
  assert.equal(center.state.frames.task, undefined);
  assert.ok(!JSON.stringify(center.state).includes("fixture-private-value"));
});

test("a frame arriving after return cannot reenable human input", async () => {
  let owner = { ...session };
  let finish!: (value: typeof picture) => void;
  const center = fixture({
    list: async () => ({ available: true, sessions: [owner] }),
    frame: () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  });
  await center.refresh("owner");
  const loading = center.frame("task");
  owner = { ...session, generation: 3, status: "agent" };
  await center.refresh("owner");
  finish(picture);
  await loading;
  assert.equal(center.state.frames.task, undefined);
});

test("save and cancel distinguish same-name downloads and bind the task and generation", async () => {
  const files: V2.BrowserDownloadView[] = ["first", "second"].map((id) => ({
    id,
    name: "report.pdf",
    status: "pending",
    detail: null,
  }));
  const sent: V2.BrowserDownloadParams[] = [];
  const center = fixture({
    downloads: async () => ({ downloads: files }),
    download: async (p) => {
      sent.push(p);
      const file = files.find((f) => f.id === p.downloadId)!;
      file.status = p.action === "save" ? "saved" : "cancelled";
      return { downloads: files };
    },
  });
  await center.refresh("owner");
  await center.downloads("task");
  await center.download(session, "first", "save");
  await center.download(session, "second", "cancel");
  assert.deepEqual(
    sent.map((p) => [p.action, p.taskId, p.generation, p.downloadId]),
    [
      ["save", "task", 2, "first"],
      ["cancel", "task", 2, "second"],
    ],
  );
  assert.deepEqual(
    center.state.downloads.task.map((f) => f.status),
    ["saved", "cancelled"],
  );
});
