import assert from "node:assert/strict";
import test from "node:test";
import { setImmediate } from "node:timers/promises";
import { jsonEvents } from "../../../../protocol/typescript/event-stream.mts";
import { HttpTransport } from "../src/http-transport.ts";
import { CODES } from "../src/protocol.ts";

test("HTTP replies wait for SSE completion and reuse the authenticated runtime", async (t) => {
  let events!: ReadableStreamDefaultController<Uint8Array>;
  let calls = 0;
  const headers: Array<Headers> = [];
  const mock: typeof fetch = async (url, init) => {
    headers.push(new Headers(init?.headers));
    if (String(url).endsWith("/api/notifications")) {
      const stream = new ReadableStream<Uint8Array>({ start(controller) { events = controller; } });
      init?.signal?.addEventListener("abort", () => events.close(), { once: true });
      return new Response(stream, { headers: { "Content-Type": "text/event-stream" } });
    }
    calls++;
    assert.ok(events, "Subscribe before submitting requests");
    return Response.json({ jsonrpc: "2.0", id: 1, result: { turnId: "turn-1" } });
  };
  const transport = new HttpTransport({ url: "http://127.0.0.1:7777", token: "test-token", fetch: mock });
  t.after(() => transport.close());
  const lines: Array<Record<string, unknown>> = [];
  transport.onLine(line => lines.push(JSON.parse(line)));
  transport.write(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "turn/start", params: {} }));
  await setImmediate();
  assert.equal(calls, 1);
  assert.equal(lines.length, 0);
  const payload = new TextEncoder().encode('data: {"type":"turnCompleted",\r\ndata: "turnId":"turn-1"}\r\n\r\n');
  events.enqueue(payload.slice(0, 12));
  events.enqueue(payload.slice(12));
  await setImmediate();
  assert.equal(lines[0]?.method, "turn/notification");
  assert.equal(lines[1]?.id, 1);
  assert.ok(headers.every(h => h.get("Authorization") === "Bearer test-token"));
  transport.close();
});

test("An SSE drop reconnects and never closes the client", async () => {
  const controllers: Array<ReadableStreamDefaultController<Uint8Array>> = [];
  let notifies = 0;
  let closes = 0;
  const mock: typeof fetch = async (url, init) => {
    if (String(url).endsWith("/api/notifications")) {
      notifies++;
      const stream = new ReadableStream<Uint8Array>({ start(c) { controllers.push(c); } });
      init?.signal?.addEventListener("abort", () => controllers[controllers.length - 1]?.close(), { once: true });
      return new Response(stream, { headers: { "Content-Type": "text/event-stream" } });
    }
    return Response.json({ jsonrpc: "2.0", id: 9, result: {} });
  };
  const transport = new HttpTransport({ url: "http://127.0.0.1:7777", fetch: mock });
  transport.onClose(() => { closes++; });
  await setImmediate();
  assert.equal(notifies, 1);

  controllers[0]!.close();
  await setImmediate();
  assert.equal(closes, 0, "a recoverable SSE drop must not fire onClose");

  await new Promise(r => setTimeout(r, 650));
  assert.ok(notifies >= 2, `expected a reconnect, saw ${notifies} subscriptions`);
  assert.equal(closes, 0, "client stays usable across the reconnect");

  const lines: string[] = [];
  transport.onLine(line => lines.push(line));
  transport.write('{"jsonrpc":"2.0","id":9,"method":"turn/start","params":{}}');
  await setImmediate();
  assert.equal(JSON.parse(lines[0]!).id, 9);
  transport.close();
  assert.equal(closes, 1, "only an explicit close fires onClose");
});

test("An HTTP failure is returned once without replay or credential leakage", async () => {
  let events!: ReadableStreamDefaultController<Uint8Array>;
  let calls = 0;
  const mock: typeof fetch = async (url, init) => {
    if (String(url).endsWith("/api/notifications")) {
      const stream = new ReadableStream<Uint8Array>({ start(c) { events = c; } });
      init?.signal?.addEventListener("abort", () => events.close(), { once: true });
      return new Response(stream);
    }
    calls++;
    return new Response("secret-reflected-by-upstream", { status: 401 });
  };
  const transport = new HttpTransport({ url: "http://localhost:7777", fetch: mock });
  const lines: string[] = [];
  transport.onLine(line => lines.push(line));
  transport.write('{"id":9,"method":"turn/start"}');
  await setImmediate();
  assert.equal(calls, 1);
  assert.equal(JSON.parse(lines[0]!).id, 9);
  assert.equal(JSON.parse(lines[0]!).error.code, CODES.INTERNAL_ERROR);
  assert.ok(!lines[0]!.includes("secret-reflected"));
  transport.close();
});

test("a disconnected notification stream refuses a turn before posting it", async (t) => {
  let posts = 0;
  const transport = new HttpTransport({ url: "http://fixture", fetch: async (url) => {
    if (String(url).endsWith("/api/notifications")) throw new Error("offline");
    posts++;
    return Response.json({ id: 2, result: {} });
  } });
  t.after(() => transport.close());
  const reply = new Promise<string>(resolve => transport.onLine(resolve));
  transport.write('{"id":2,"method":"turn/start"}');
  assert.ok(JSON.parse(await reply).error);
  assert.equal(posts, 0);
});

test("leaving or rejecting an event cancels the underlying subscription and unlocks it", async () => {
  for (const malformed of [false, true]) {
    let cancelled = 0;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        const data = malformed ? "bad json" : '{"type":"turnCompleted"}';
        controller.enqueue(new TextEncoder().encode("data: " + data + "\n\n"));
      },
      cancel() { cancelled++; },
    });
    const events = jsonEvents(body);
    if (malformed) {
      await assert.rejects(events.next(), SyntaxError);
    } else {
      assert.deepEqual((await events.next()).value, {type: "turnCompleted"});
      await events.return(undefined);
    }
    assert.equal(cancelled, 1);
    assert.equal(body.locked, false);
  }
});
