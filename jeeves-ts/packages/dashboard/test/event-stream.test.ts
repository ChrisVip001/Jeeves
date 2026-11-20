import assert from "node:assert/strict";
import { test } from "node:test";
import { followEvents } from "../src/api.ts";

test("event streams preserve split UTF-8 and accept CRLF, CR and LF frame endings", async (t) => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: { getItem: () => null } });
  t.after(() => {
    if (previous) Object.defineProperty(globalThis, "localStorage", previous);
    else Reflect.deleteProperty(globalThis, "localStorage");
  });
  const wire = ': heartbeat\r\n\r\ndata: {"kind":\r\ndata: "完成"}\r\n\r\ndata: {"kind":"second"}\r\rdata: {"kind":"third"}\n\ndata: {"kind":"unfinished"}';
  const bytes = new TextEncoder().encode(wire);
  let at = 0;
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (at === bytes.length) controller.close();
      else controller.enqueue(bytes.subarray(at, ++at));
    },
  });
  t.mock.method(globalThis, "fetch", async () => new Response(body));
  const events = [];
  for await (const event of followEvents()) events.push(event);
  assert.deepEqual(events, [{ kind: "完成" }, { kind: "second" }, { kind: "third" }]);
});
