import assert from "node:assert/strict";
import { test } from "node:test";
import { fetchDashboard, interruptTurn } from "../src/api.ts";

function withFetch(responses: (() => Response)[], body: () => Promise<void>) {
  const originalFetch = globalThis.fetch;
  const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => null } });
  const calls: string[] = [];
  globalThis.fetch = async (_input, init) => {
    calls.push((init?.method ?? "GET").toUpperCase());
    const next = responses.shift();
    assert.ok(next, "unexpected extra request");
    return next();
  };
  return body()
    .then(() => calls)
    .finally(() => {
      globalThis.fetch = originalFetch;
      if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage);
      else Reflect.deleteProperty(globalThis, "localStorage");
    });
}

const starting = () =>
  new Response(JSON.stringify({ error: "workspace is starting" }), {
    status: 503,
    headers: { "content-type": "application/json", "retry-after": "1" },
  });

test("a read waits out a starting workspace instead of failing", async () => {
  const calls = await withFetch(
    [starting, () => new Response(JSON.stringify({ sessions: [] }), { status: 200 })],
    async () => {
      const data = await fetchDashboard();
      assert.deepEqual(data, { sessions: [] });
    },
  );
  assert.deepEqual(calls, ["GET", "GET"]);
});

test("a write is never replayed and the gateway refusal reads as a sentence", async () => {
  const calls = await withFetch([starting], async () => {
    await assert.rejects(
      interruptTurn({ sessionId: "chat", turnId: "turn" } as never),
      (error: Error & { code?: number }) => {
        assert.equal(error.code, 503);
        assert.notEqual(error.message, "workspace is starting");
        assert.doesNotMatch(error.message, /^\{/);
        return true;
      },
    );
  });
  assert.deepEqual(calls, ["POST"]);
});

test("an unknown gateway refusal keeps its own text", async () => {
  await withFetch(
    [() => new Response(JSON.stringify({ error: "quota exceeded" }), { status: 429 })],
    async () => {
      await assert.rejects(fetchDashboard(), (error: Error) => error.message === "quota exceeded");
    },
  );
});
