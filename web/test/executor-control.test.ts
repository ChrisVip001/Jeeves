import assert from "node:assert/strict";
import { after, test } from "node:test";
import {
  executor,
  refreshExecutor,
  changeExecutor,
} from "../src/executor-control.ts";

const fetchImpl = globalThis.fetch;
const storage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: { getItem: () => null },
});
after(() => {
  globalThis.fetch = fetchImpl;
  if (storage) Object.defineProperty(globalThis, "localStorage", storage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

test("a late read cannot undo a confirmed emergency stop", async () => {
  let finish!: (response: Response) => void;
  globalThis.fetch = async () =>
    new Promise((resolve) => {
      finish = resolve;
    });
  const reading = refreshExecutor();
  globalThis.fetch = async (path, options) => {
    assert.equal(path, "/api/executor/stop");
    assert.equal(options?.method, "POST");
    assert.deepEqual(JSON.parse(String(options.body)), {});
    return Response.json({ stopped: true, generation: 1 });
  };
  await changeExecutor("stop");
  finish(Response.json({ stopped: false, generation: 0 }));
  await reading;
  assert.equal(executor.view?.stopped, true);
  assert.equal(executor.view?.generation, 1);
});

test("a stale release reloads the current stop and never repeats the release", async () => {
  let reads = 0;
  let releases = 0;
  globalThis.fetch = async (path, options) => {
    if (path === "/api/executor/control") {
      return Response.json({
        stopped: true,
        generation: ++reads === 1 ? 3 : 5,
      });
    }
    assert.equal(path, "/api/executor/release");
    assert.deepEqual(JSON.parse(String(options?.body)), {
      expectedGeneration: 3,
    });
    releases++;
    return Response.json({ error: "stale generation" }, { status: 400 });
  };
  await refreshExecutor();
  await changeExecutor("release");
  assert.equal(releases, 1);
  assert.equal(reads, 2);
  assert.equal(executor.view?.generation, 5);
  assert.equal(executor.view?.stopped, true);
  assert.ok(executor.message);
});

test("when status is unavailable, a user can still stop but cannot release", async () => {
  globalThis.fetch = async () => new Response(null, { status: 503 });
  await refreshExecutor();
  assert.equal(executor.readFailed, true);
  assert.equal(executor.view, null);
  let requests = 0;
  globalThis.fetch = async (path) => {
    assert.equal(path, "/api/executor/stop");
    requests++;
    return Response.json({ stopped: true, generation: 7 });
  };
  await changeExecutor("release");
  assert.equal(requests, 0);
  await changeExecutor("stop");
  assert.equal(requests, 1);
  assert.equal(executor.readFailed, false);
});
