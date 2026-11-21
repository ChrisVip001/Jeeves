import assert from "node:assert/strict";
import test from "node:test";

import { CODES, JeevesClient, JeevesError } from "@jeeves/client";
import type { ServerNotification, Transport } from "@jeeves/client";

import { JeevesBridge, explain } from "../src/bridge.ts";
import type { ProgressNote } from "../src/bridge.ts";

interface SentRequest {
  id: number;
  method: string;
  params: Record<string, unknown>;
}

/** A fake transport on the server's side: take requests, emit notifications, return replies. */
class FakeServer implements Transport {
  readonly written: SentRequest[] = [];
  closeCalled = false;

  #onLine?: (line: string) => void;

  write(line: string): void {
    this.written.push(JSON.parse(line) as SentRequest);
  }

  onLine(handler: (line: string) => void): void {
    this.#onLine = handler;
  }

  onLog(): void {}
  onClose(): void {}

  close(): void {
    this.closeCalled = true;
  }

  /** Reply to request number `index`. */
  reply(index: number, result: unknown): void {
    const request = this.written[index];
    assert.ok(request !== undefined, `request ${index} was never written out`);
    this.#onLine?.(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
  }

  /** Return an error for request number `index`. */
  fail(index: number, code: number, message: string): void {
    const request = this.written[index];
    assert.ok(request !== undefined, `request ${index} was never written out`);
    this.#onLine?.(
      JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code, message } }),
    );
  }

  notify(notification: ServerNotification): void {
    this.#onLine?.(
      JSON.stringify({
        jsonrpc: "2.0",
        method: "turn/notification",
        params: notification,
      }),
    );
  }
}

function setup(options: { onProgress?: (note: ProgressNote) => void } = {}): {
  bridge: JeevesBridge;
  server: FakeServer;
} {
  const server = new FakeServer();
  const client = new JeevesClient({ transport: server, onLog: () => {} });
  const bridge = new JeevesBridge({ client, onProgress: options.onProgress });
  return { bridge, server };
}

function meta(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    gate: null,
    graph: null,
    iterations: 1,
    latencyMs: 12,
    tools: [],
    model: "small-model",
    provider: "deepseek",
    ...overrides,
  };
}

function startFor(server: FakeServer, index: number, turnId?: string): string {
  const request = server.written[index]!;
  const id = turnId ?? `channel-turn:${request.params["messageId"]}`;
  server.notify({
    type: "turnStarted",
    turnId: id,
    sessionId: "jeeves-main",
    userMessage: String(request.params["message"]),
    ts: "2026-09-19T00:00:00.000Z",
  });
  return id;
}

function completed(
  turnId: string,
  reply: string,
  metaOverrides: Record<string, unknown> = {},
): ServerNotification {
  return {
    type: "turnCompleted",
    turnId,
    reply,
    iterations: 1,
    meta: meta(metaOverrides) as never,
    usage: null,
  };
}

/** Let queued microtasks run — used to observe "has the second request gone out yet". */
function tick(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

test("a private Feishu turn routes its own approval and ignores identical local input", async () => {
  const { bridge, server } = setup();
  const origin = { channel: "lark", chatId: "oc_private", messageId: "om_1", senderId: "ou_owner" };
  const outcome = bridge.ask("lark:oc_private", "ls", undefined, "om_1", origin);
  await tick();
  assert.equal(server.written[0]?.method, "interaction/start");
  assert.equal(server.written[0]?.params["sessionId"], "jeeves-main");
  startFor(server, 0, "another-turn");
  const unrelated = bridge.locateApproval("another-turn", "ap-unrelated");
  server.reply(1, { approvals: [] });
  assert.equal(await unrelated, null);
  const turnId = startFor(server, 0);
  assert.deepEqual(await bridge.locateApproval(turnId, "ap-1"), {
    source: origin, task: null, bindToSender: true,
  });
  server.notify(completed(turnId, "finished"));
  server.reply(0, { turnId });
  await outcome;
});

test("platform message identities are valid, stable and scoped to their source", async () => {
  const { bridge, server } = setup();
  const submit = async (conversation: string, index: number) => {
    const promise = bridge.ask(conversation, "hello", undefined, "om.1/x");
    await tick();
    const messageId = server.written[index]!.params["messageId"];
    assert.match(String(messageId), /^[a-f0-9]{64}$/);
    const turnId = startFor(server, index);
    server.notify(completed(turnId, "ok"));
    server.reply(index, { turnId });
    await promise;
    return messageId;
  };
  const first = await submit("lark:oc_1", 0);
  assert.equal(await submit("lark:oc_1", 1), first);
  assert.notEqual(await submit("lark:oc_2", 2), first);
});

test("one turn: the request is right, the reply and the telemetry come back as they were", async () => {
  const { bridge, server } = setup();

  const promise = bridge.ask("telegram:42", "hello");
  await tick();
  assert.equal(server.written.length, 1);

  const request = server.written[0]!;
  assert.equal(request.method, "interaction/start");
  assert.equal(request.params["sessionId"], "jeeves-main");
  assert.equal(request.params["message"], "hello");
  assert.equal((request.params["source"] as {senderId: string}).senderId, "42");

  const firstTurn = startFor(server, 0);
  server.notify({ type: "toolStarted", turnId: firstTurn, toolUseId: "tu-1", tool: "memory_search", args: {} });
  server.notify({
    type: "toolCompleted",
    turnId: firstTurn,
    toolUseId: "tu-1",
    tool: "memory_search",
    output: "three",
    status: "ok",
    durationMs: 3,
  });
  server.notify(completed(firstTurn, "hello there", { model: "big-model", iterations: 2 }));
  server.reply(0, { turnId: firstTurn });

  const outcome = await promise;
  assert.equal(outcome.sessionId, "jeeves-main");
  assert.equal(outcome.reply, "hello there");
  assert.deepEqual(outcome.tools, ["memory_search"]);
  assert.equal(outcome.model, "big-model");
  assert.equal(outcome.iterations, 2);
});

test("two messages in one conversation: the second waits for the first to finish", async () => {
  const { bridge, server } = setup();

  const first = bridge.ask("telegram:42", "the first");
  const second = bridge.ask("telegram:42", "the second");

  await tick();
  assert.equal(server.written.length, 1, "two messages must not hold the server at once");

  const firstTurn = startFor(server, 0);
  server.notify(completed(firstTurn, "the first answer"));
  server.reply(0, { turnId: firstTurn });
  assert.equal((await first).reply, "the first answer");

  await tick();
  assert.equal(server.written.length, 2, "only once the first is back may the second go out");
  assert.equal(server.written[1]!.params["message"], "the second");

  const secondTurn = startFor(server, 1);
  server.notify(completed(secondTurn, "the second answer"));
  server.reply(1, { turnId: secondTurn });
  assert.equal((await second).reply, "the second answer");
});

test("different channel conversations share the Runtime entry but keep distinct origins", async () => {
  const { bridge, server } = setup();
  const alice = bridge.ask("telegram:1", "I am alpha");
  const bob = bridge.ask("telegram:2", "I am beta");
  await tick();
  assert.equal(server.written.length, 1);
  const aliceTurn = startFor(server, 0);
  server.notify(completed(aliceTurn, "hello alpha"));
  server.reply(0, { turnId: aliceTurn });
  assert.equal((await alice).reply, "hello alpha");
  await tick();
  assert.equal(server.written.length, 2);
  for (const [index, request] of server.written.entries()) {
    assert.equal(request.params["sessionId"], "jeeves-main");
    assert.equal((request.params["source"] as {chatId: string}).chatId, String(index + 1));
  }
  const bobTurn = startFor(server, 1);
  server.notify(completed(bobTurn, "hello beta"));
  server.reply(1, { turnId: bobTurn });
  assert.equal((await bob).reply, "hello beta");
});

test("a failed turn: the error comes through and no debris reaches the next turn", async () => {
  const { bridge, server } = setup();

  const first = bridge.ask("telegram:42", "this one will fail");
  await tick();
  const firstTurn = startFor(server, 0);
  server.notify(completed(firstTurn, "the first turn's answer"));
  server.fail(0, CODES.INTERNAL_ERROR, "the write failed");
  await assert.rejects(first, /the write failed/);

  const second = bridge.ask("telegram:42", "this one will work");
  await tick();
  const secondTurn = startFor(server, 1);
  server.notify(completed(secondTurn, "the second turn's answer"));
  server.reply(1, { turnId: secondTurn });

  assert.equal((await second).reply, "the second turn's answer", "it must not pick up the previous turn's answer");
});

test("the answer came but the notification chain broke: report it plainly, do not invent an empty reply", async () => {
  const { bridge, server } = setup();

  const promise = bridge.ask("telegram:42", "hello");
  await tick();
  const firstTurn = startFor(server, 0);
  server.reply(0, { turnId: firstTurn });

  await assert.rejects(promise, /never saw turnCompleted/);
});

test("progress: say when a tool starts, and say when one fails too", async () => {
  const notes: string[] = [];
  const { bridge, server } = setup({
    onProgress: (note) => notes.push(note.text),
  });

  const promise = bridge.ask("telegram:42", "look this up for me");
  await tick();
  const firstTurn = startFor(server, 0);
  server.notify({ type: "toolStarted", turnId: firstTurn, toolUseId: "tu-read_file", tool: "read_file", args: {} });
  server.notify({
    type: "toolCompleted",
    turnId: firstTurn,
    toolUseId: "tu-read_file",
    tool: "read_file",
    output: "ENOENT",
    status: "error",
    durationMs: 1,
  });
  server.notify({ type: "toolStarted", turnId: firstTurn, toolUseId: "tu-http_get", tool: "http_get", args: {} });
  server.notify({
    type: "toolCompleted",
    turnId: firstTurn,
    toolUseId: "tu-http_get",
    tool: "http_get",
    output: "200",
    status: "ok",
    durationMs: 20,
  });
  server.notify({ type: "consolidationCompleted", newFacts: 2 });
  server.notify(completed(firstTurn, "found it"));
  server.reply(0, { turnId: firstTurn });

  await promise;
  assert.deepEqual(notes, [
    "using read_file…",
    "read_file did not succeed; it is trying another way",
    "using http_get…",
  ]);
});

test("errors are put into plain words: no model, not built yet, cannot connect are three different things", () => {
  const provider = new JeevesError({
    code: CODES.PROVIDER_ERROR,
    message: "401 Unauthorized",
  });
  assert.match(explain(provider), /jeeves config/, "it has to tell the user where to fix it");

  const todo = new JeevesError({
    code: CODES.NOT_IMPLEMENTED,
    message: "'model/list' is not implemented",
  });
  assert.match(explain(todo), /not built yet/);

  assert.match(explain(new Error("spawn jeeves ENOENT")), /cannot reach Jeeves/);
  assert.match(explain("anything at all"), /cannot reach Jeeves/);
});

test("closing the gateway closes the app-server's stdin", () => {
  const { bridge, server } = setup();
  bridge.close();
  assert.ok(server.closeCalled);
});

test("a dispatched message goes through interaction/start with its provenance", async () => {
  const { bridge, server } = setup();
  const source = { channel: "lark", chatId: "oc_g", messageId: "om/1", senderId: "ou_owner" };
  const first = bridge.ask("lark:oc_g", "do the thing", source);
  await tick();
  const request = server.written[0]!;
  assert.equal(request.method, "interaction/start");
  assert.equal(request.params["sessionId"], "jeeves-main");
  assert.deepEqual(request.params["source"], source);
  const turnId = startFor(server, 0, `interaction-${request.params["messageId"]}`);
  server.notify(completed(turnId, "done"));
  server.reply(0, { turnId });
  assert.equal((await first).reply, "done");
});

test("an approval in a dispatched turn is located from the turn itself", async () => {
  const { bridge, server } = setup();
  const source = { channel: "lark", chatId: "oc_g", messageId: "om_1", senderId: "ou_owner" };
  const seen: string[] = [];
  bridge.onApprovalRequested((request) => seen.push(request.requestId));

  const turn = bridge.ask("lark:oc_g", "push it", source);
  await tick();
  const firstTurn = startFor(server, 0);
  server.notify({
    type: "approvalRequested",
    turnId: firstTurn,
    requestId: "ap-1",
    tool: "run_command",
    argsPreview: "git push",
    reason: "asks",
    expiresInMs: 1000,
    version: 1,
  });
  assert.deepEqual(seen, ["ap-1"]);
  assert.deepEqual(await bridge.locateApproval(firstTurn, "ap-1"), { source, task: null });
  assert.equal(server.written.length, 1, "no lookup needed for a turn in flight");

  server.notify(completed(firstTurn, "done"));
  server.reply(0, { turnId: firstTurn });
  await turn;
});

test("a task's approval is located through approval/pending and answered once", async () => {
  const { bridge, server } = setup();
  const source = { channel: "lark", chatId: "oc_g", messageId: "om_1", senderId: "ou_owner" };
  const located = bridge.locateApproval("t7", "ap-7");
  await tick();
  assert.equal(server.written[0]?.method, "approval/pending");
  assert.deepEqual(server.written[0]?.params, { sessionId: "jeeves-main" });
  const task = { title: "ship", source };
  server.reply(0, {
    approvals: [
      { sessionId: "s", ownerSessionId: "jeeves-main", task: null, request: { turnId: "t7", requestId: "other" } },
      { sessionId: "s", ownerSessionId: "jeeves-main", task, request: { turnId: "t7", requestId: "ap-7" } },
    ],
  });
  assert.deepEqual(await located, { source, task });

  const answered = bridge.answerApproval("t7", "ap-7", true, "lark:ou_owner");
  await tick();
  assert.equal(server.written[1]?.method, "approval/respond");
  assert.deepEqual(server.written[1]?.params, {
    turnId: "t7",
    requestId: "ap-7",
    approved: true,
    remember: "once",
    actor: "lark:ou_owner",
  });
  server.reply(1, { accepted: true });
  assert.equal(await answered, true);
});

test("a request no longer waiting locates to null", async () => {
  const { bridge, server } = setup();
  const located = bridge.locateApproval("t7", "ap-7");
  await tick();
  server.reply(0, { approvals: [] });
  assert.equal(await located, null);
});
