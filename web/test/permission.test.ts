import assert from "node:assert/strict";
import { test } from "node:test";
import { usePermission } from "../src/ui/usePermission.ts";
import type { PermissionMode, SessionPermissionResponse } from "../src/ui/types.ts";

function policy(
  mode: PermissionMode | null,
  revision: number,
  sessionId = "main",
): SessionPermissionResponse {
  return {
    sessionId,
    mode,
    revision,
    execEnabled: true,
    writeEnabled: true,
    delegateEnabled: true,
  };
}

test("trusting a session writes full access against the revision it just read", async () => {
  const written: { mode: PermissionMode; expectedRevision: number }[] = [];
  let stored = policy("standard", 7);
  const permission = usePermission({
    read: async () => stored,
    write: async (sessionId, mode, expectedRevision) => {
      written.push({ mode, expectedRevision });
      stored = policy(mode, expectedRevision + 1, sessionId);
      return stored;
    },
  });
  await permission.load("main");
  assert.equal(permission.state.mode, "standard");
  assert.equal(permission.state.revision, 7);

  stored = policy("standard", 9);
  assert.equal(await permission.trust("main"), true);
  assert.deepEqual(written, [{ mode: "fullAccess", expectedRevision: 9 }]);
  assert.equal(permission.state.mode, "fullAccess");
  assert.equal(permission.state.revision, 10);
  assert.equal(permission.state.busy, false);
  assert.equal(permission.state.error, "");
});

test("a refused switch keeps the old preset and says why", async () => {
  const permission = usePermission({
    read: async () => policy("standard", 1),
    write: async () => {
      throw new Error("revision moved on");
    },
  });
  await permission.load("main");
  assert.equal(await permission.trust("main"), false);
  assert.equal(permission.state.mode, "standard");
  assert.match(permission.state.error, /revision moved on/);
  assert.equal(permission.state.busy, false);
});

test("a preset changed in another window moves the switch without a write", async () => {
  const permission = usePermission({
    read: async () => policy("standard", 2),
    write: async () => {
      throw new Error("a notification must never write a preset");
    },
  });
  await permission.load("main");
  permission.changed(policy("fullAccess", 3));
  assert.equal(permission.state.mode, "fullAccess");
  assert.equal(permission.state.revision, 3);
  permission.changed(policy("standard", 4, "another-chat"));
  assert.equal(permission.state.mode, "fullAccess");
});
