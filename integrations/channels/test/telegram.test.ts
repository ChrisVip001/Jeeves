import assert from "node:assert/strict";
import test from "node:test";

import { TelegramAdapter } from "../src/telegram.ts";
import type { TelegramOptions } from "../src/telegram.ts";

interface Call {
  method: string;
  body: Record<string, unknown>;
}

/** A fake Bot API: queued `getUpdates` batches, every call recorded, stops the adapter when drained. */
function fakeApi(batches: unknown[][]) {
  const calls: Call[] = [];
  let adapter: TelegramAdapter | undefined;
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const method = String(input).split("/").pop() ?? "";
    const body = JSON.parse(String(init?.body ?? "{}")) as Record<string, unknown>;
    calls.push({ method, body });
    if (method === "getUpdates") {
      const next = batches.shift();
      if (next === undefined) {
        adapter?.stop();
        return Response.json({ ok: true, result: [] });
      }
      return Response.json({ ok: true, result: next });
    }
    return Response.json({ ok: true, result: { message_id: 900 + calls.length } });
  };
  return {
    calls,
    fetch,
    attach(a: TelegramAdapter) {
      adapter = a;
    },
  };
}

const message = (updateId: number, from: number, text: string, chat = from) => ({
  update_id: updateId,
  message: { message_id: updateId * 10, from: { id: from }, chat: { id: chat, type: "private" }, text },
});

async function run(
  batches: unknown[][],
  options: Partial<TelegramOptions> = {},
  respond: (conversation: string, text: string) => Promise<string> = async (_c, t) => `echo ${t}`,
) {
  const api = fakeApi(batches);
  const logs: string[] = [];
  const adapter = new TelegramAdapter(
    { token: "T", fetch: api.fetch, onLog: (l) => logs.push(l), pollTimeoutSec: 0, ...options },
    respond,
  );
  api.attach(adapter);
  await adapter.run();
  return { api, logs, adapter };
}

const sent = (calls: Call[]) => calls.filter((c) => c.method === "sendMessage").map((c) => c.body);

test("a repeated update_id is answered once", async () => {
  const asked: string[] = [];
  const { api } = await run([[message(1, 7, "hi"), message(1, 7, "hi")]], {}, async (_c, t) => {
    asked.push(t);
    return "ok";
  });
  assert.deepEqual(asked, ["hi"]);
  assert.equal(sent(api.calls).length, 1);
});

test("the offset moves past the last update and is persisted", async () => {
  const saved: number[] = [];
  let current = 0;
  const { api } = await run([[message(5, 7, "a"), message(6, 7, "b")]], {
    offset: { read: () => current, write: (n) => (saved.push(n), (current = n)) },
  });
  assert.equal(saved.at(-1), 7);
  const polls = api.calls.filter((c) => c.method === "getUpdates");
  assert.equal(polls[1]?.body["offset"], 7);
});

test("a long reply is split under 4096 characters", async () => {
  const { api } = await run([[message(1, 7, "go")]], {}, async () => "x".repeat(9000));
  const texts = sent(api.calls).map((b) => String(b["text"]));
  assert.equal(texts.length, 3);
  assert.ok(texts.every((t) => t.length <= 4096));
});

test("outside the allowlist: no answer", async () => {
  const { api, logs } = await run([[message(1, 99, "hi")]], { allow: ["7"] });
  assert.equal(sent(api.calls).length, 0);
  assert.ok(logs.some((l) => l.includes("allowlist")));
});

test("an approval is asked with buttons and a click answers it once", async () => {
  const answered: unknown[] = [];
  const click = {
    update_id: 2,
    callback_query: { id: "cb1", from: { id: 7 }, data: "ap:1:1", message: { message_id: 55, chat: { id: 7 } } },
  };
  const api = fakeApi([[], [click]]);
  const adapter = new TelegramAdapter(
    {
      token: "T",
      fetch: api.fetch,
      pollTimeoutSec: 0,
      allow: ["7"],
      onApproval: async (action, actor) => {
        answered.push([action, actor]);
        return true;
      },
    },
    async () => "",
  );
  api.attach(adapter);
  await adapter.sendApproval("7", {
    turnId: "t1",
    requestId: "r1",
    version: 4,
    tool: "run_command",
    argsPreview: "ls",
    reason: "why",
    expiresInMs: 60_000,
  } as never);
  const ask = sent(api.calls)[0]!;
  assert.ok(JSON.stringify(ask["reply_markup"]).includes("inline_keyboard"));
  await adapter.run();
  assert.deepEqual(answered, [[{ turnId: "t1", requestId: "r1", version: 4, approved: true }, "7"]]);
  assert.ok(api.calls.some((c) => c.method === "answerCallbackQuery"));
});

test("a stale or foreign click changes nothing", async () => {
  const stale = { update_id: 2, callback_query: { id: "c2", from: { id: 7 }, data: "ap:1:1", message: { message_id: 5, chat: { id: 7 } } } };
  const foreign = { update_id: 3, callback_query: { id: "c3", from: { id: 99 }, data: "ap:1:1", message: { message_id: 5, chat: { id: 7 } } } };
  let calls = 0;
  const api = fakeApi([[stale, foreign]]);
  const adapter = new TelegramAdapter(
    { token: "T", fetch: api.fetch, pollTimeoutSec: 0, allow: ["7"], onApproval: async () => (calls++, false) },
    async () => "",
  );
  api.attach(adapter);
  await adapter.sendApproval("7", { turnId: "t", requestId: "r", version: 1, tool: "x", argsPreview: "", reason: "", expiresInMs: 1000 } as never);
  await adapter.run();
  assert.equal(calls, 1, "only the allowed click reached the Runtime, and it was too late");
  const answers = api.calls.filter((c) => c.method === "answerCallbackQuery").map((c) => String(c.body["text"]));
  assert.ok(answers.some((t) => /already|expired/i.test(t)));
});

test("a second channel answering the same approval is told who got there first", async () => {
  const click = { update_id: 2, callback_query: { id: "c9", from: { id: 7 }, data: "ap:1:1", message: { message_id: 5, chat: { id: 7 } } } };
  const api = fakeApi([[click]]);
  const adapter = new TelegramAdapter(
    {
      token: "T",
      fetch: api.fetch,
      pollTimeoutSec: 0,
      allow: ["7"],
      onApproval: async () => ({ accepted: false, answeredBy: "lark:ou_1" }),
    },
    async () => "",
  );
  api.attach(adapter);
  await adapter.sendApproval("7", { turnId: "t", requestId: "r", version: 1, tool: "x", argsPreview: "", reason: "", expiresInMs: 1000 } as never);
  await adapter.run();
  const said = api.calls.filter((c) => c.method === "answerCallbackQuery" || c.method === "editMessageText").map((c) => String(c.body["text"]));
  assert.ok(said.length >= 2 && said.every((t) => t.includes("Already answered by lark:ou_1")), said.join(" | "));
});
