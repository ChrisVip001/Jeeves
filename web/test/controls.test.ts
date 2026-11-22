import assert from "node:assert/strict";
import { after, test } from "node:test";
import {
  refreshPolicy,
  saveWorkspace,
  webState,
  workspaceActions,
} from "../src/controls.ts";

const originalFetch = globalThis.fetch;
const originalStorage = Object.getOwnPropertyDescriptor(
  globalThis,
  "localStorage",
);
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: { getItem: () => null },
});
after(() => {
  globalThis.fetch = originalFetch;
  if (originalStorage)
    Object.defineProperty(globalThis, "localStorage", originalStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

test("an older server leaves an actionable permission error and can recover on retry", async () => {
  webState.sessionId = "session-a";
  globalThis.fetch = async () => new Response("Not Found", { status: 404 });
  await refreshPolicy();
  assert.equal(webState.policy, undefined);
  assert.match(webState.policyError, /重启/);

  globalThis.fetch = async () =>
    Response.json({ sessionId: "session-a", mode: "standard", revision: 1 });
  await refreshPolicy();
  assert.equal(webState.policy?.mode, "standard");
  assert.equal(webState.policyError, "");
});

test("a folder selection persists to its originating session, never a directory as session id", async () => {
  webState.sessionId = "session-b";
  let request: unknown;
  let changed = false;
  workspaceActions.cwdChanged = () => {
    changed = true;
  };
  workspaceActions.refresh = async () => {};
  globalThis.fetch = async (_url, init) => {
    request = JSON.parse(String(init?.body));
    return Response.json({ session: { id: "session-a", cwd: "/tmp/project" } });
  };
  assert.equal(await saveWorkspace("/tmp/project", "session-a"), true);
  assert.deepEqual(request, { sessionId: "session-a", cwd: "/tmp/project" });
  assert.equal(
    changed,
    false,
    "a late response must not replace another session's directory",
  );

  webState.sessionId = "session-a";
  assert.equal(await saveWorkspace("/tmp/project"), true);
  assert.equal(changed, true);
});

test("a refused directory change preserves the previous directory", async () => {
  webState.sessionId = "session-a";
  let changed = false;
  workspaceActions.cwdChanged = () => {
    changed = true;
  };
  globalThis.fetch = async () =>
    Response.json(
      { code: -32602, message: "folder unavailable" },
      { status: 400 },
    );
  assert.equal(await saveWorkspace("/missing"), false);
  assert.equal(changed, false);
  assert.match(webState.message, /folder unavailable/);
});
