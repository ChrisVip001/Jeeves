import { LarkInbox } from "../src/lark-inbox.ts";
import { mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import assert from "node:assert/strict";
import test from "node:test";

import { LarkAdapter } from "../src/lark.ts";
import type { LarkAdapterOptions, LarkSocket, LarkSocketFactory } from "../src/lark.ts";
import { METHOD_CONTROL, METHOD_DATA, decodeFrame, encodeFrame, headerValue } from "../src/lark-proto.ts";
import type { LarkFrame } from "../src/lark-proto.ts";
import type { Responder } from "../src/bridge.ts";

/**
 * The fake connection.
 *
 * It sends and receives **with the real codec** — frames are truly encoded to binary and decoded
 * back, so "a frame we can't decode ourselves" blows up on the spot, not at a real server.
 */
class FakeSocket implements LarkSocket {
  readonly sent: LarkFrame[] = [];
  closed = false;
  binaryType: string | undefined;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;

  send(data: string | Uint8Array): void {
    assert.ok(data instanceof Uint8Array, "only binary travels on the Feishu wire, never text");
    const frame = decodeFrame(data);
    assert.ok(frame !== null, "a frame we built ourselves does not come apart");
    this.sent.push(frame);
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.onclose?.();
  }

  /** Connected. The first frame can only go out after this. */
  open(): void {
    this.onopen?.();
  }

  /** The server pushes one frame in. */
  push(frame: LarkFrame | Uint8Array): void {
    this.onmessage?.({ data: frame instanceof Uint8Array ? frame : encodeFrame(frame) });
  }

  /** Push something over that is **not binary**. */
  pushRaw(data: unknown): void {
    this.onmessage?.({ data });
  }

  /** Sent frames whose `type` equals this value. */
  frames(type: string): LarkFrame[] {
    return this.sent.filter((frame) => headerValue(frame, "type") === type);
  }
}

class FakeSockets {
  readonly all: FakeSocket[] = [];

  readonly factory: LarkSocketFactory = () => {
    const socket = new FakeSocket();
    this.all.push(socket);
    return socket;
  };

  get latest(): FakeSocket {
    const socket = this.all.at(-1);
    assert.ok(socket !== undefined, "it never connected at all");
    return socket;
  }
}

/** The fake Lark HTTP side. Requests have the same shape as real ones. */
class FakeLark {
  /** The bodies sent out (in order). */
  readonly said: string[] = [];
  readonly asked: string[] = [];
  /** Every message posted: where, what kind, and the decoded content. */
  readonly posts: Array<{ url: string; kind: string; content: Record<string, unknown> }> = [];
  /** The body of the address-trade request. */
  handshake: unknown;

  pingInterval: number | undefined;
  selfId: string | undefined = "ou_jeeves";
  /** Script a path's answers, consumed in order. */
  readonly #scripted = new Map<string, Array<{ code: number; msg?: string }>>();

  script(path: string, response: { code: number; msg?: string }): void {
    const queue = this.#scripted.get(path) ?? [];
    queue.push(response);
    this.#scripted.set(path, queue);
  }

  readonly fetch = async (
    input: string | URL | Request,
    init?: RequestInit,
  ): Promise<Response> => {
    await new Promise((resolve) => setImmediate(resolve));

    const url = String(input);
    this.asked.push(url);
    const body = JSON.parse(String(init?.body ?? "{}")) as Record<string, unknown>;

    if (url.endsWith("/callback/ws/endpoint")) {
      this.handshake = body;
      return reply(200, {
        code: 0,
        msg: "ok",
        data: {
          URL: `wss://fake.feishu.cn/connect?device_id=d&service_id=1&app_id=${String(body["AppID"])}`,
          ClientConfig: this.pingInterval === undefined ? {} : { PingInterval: this.pingInterval },
        },
      });
    }

    const scripted = this.#scripted.get(new URL(url).pathname)?.shift();
    if (scripted !== undefined) return reply(200, scripted);

    if (url.includes("/auth/v3/tenant_access_token/internal")) {
      return reply(200, { code: 0, tenant_access_token: "t-1", expire: 7200 });
    }
    if (url.includes("/bot/v3/info")) {
      return reply(200, { code: 0, bot: this.selfId === undefined ? {} : { open_id: this.selfId } });
    }
    if (url.includes("/im/v1/messages")) {
      const content = JSON.parse(String(body["content"])) as Record<string, unknown>;
      this.posts.push({ url, kind: String(body["msg_type"]), content });
      if (body["msg_type"] === "text") this.said.push(content["text"] as string);
      return reply(200, { code: 0 });
    }
    throw new Error(`the fake Feishu does not handle this path: ${url}`);
  };
}

function reply(status: number, value: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => value,
    text: async () => JSON.stringify(value),
  } as unknown as Response;
}

/**
 * Wait for a condition. The fake network is still async; never assume the next step is done.
 *
 * The deadline is 5 seconds: the reconnect case walks a 2-second backoff, which genuinely waits.
 */
async function waitFor(condition: () => boolean, what = "the condition", timeoutMs = 5_000): Promise<void> {
  const until = Date.now() + timeoutMs;
  while (Date.now() < until) {
    if (condition()) return;
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.fail(`waited in vain for ${what}`);
}

/** Drain the queued microtasks. */
async function settle(): Promise<void> {
  for (let round = 0; round < 6; round += 1) {
    await new Promise((resolve) => setImmediate(resolve));
  }
}

interface Harness {
  adapter: LarkAdapter;
  sockets: FakeSockets;
  lark: FakeLark;
  running: Promise<void>;
  /** Everything the adapter said. Some behaviors are only visible in the logs. */
  logs: string[];
  /** Connect and let it start (the first frame must wait for onopen). */
  connect: () => Promise<FakeSocket>;
  stop: () => Promise<void>;
}

function harness(options: Partial<LarkAdapterOptions>, respond: Responder): Harness {
  const sockets = new FakeSockets();
  const lark = new FakeLark();
  const logs: string[] = [];
  const adapter = new LarkAdapter(
    {
      inbox: options.inbox ?? new LarkInbox(),
      appId: "cli_test",
      appSecret: "secret",
      socket: sockets.factory,
      fetch: lark.fetch,
      onLog: (text) => logs.push(text),
      ...options,
    },
    respond,
  );

  const running = adapter.run();

  return {
    adapter,
    sockets,
    lark,
    running,
    logs,
    connect: async () => {
      await waitFor(() => sockets.all.length > 0, "the connection");
      const socket = sockets.latest;
      socket.open();
      await settle();
      return socket;
    },
    stop: async () => {
      adapter.stop();
      await running;
    },
  };
}

/** The JSON of one message event. */
function eventBody(options: {
  openId?: string;
  senderType?: string;
  chatId?: string;
  chatType?: string;
  messageType?: string;
  text?: string;
  mentions?: Array<{ open_id: string; key?: string }>;
  eventType?: string;
}): string {
  return JSON.stringify({
    schema: "2.0",
    header: { event_type: options.eventType ?? "im.message.receive_v1", event_id: "ev_1" },
    event: {
      sender: {
        sender_type: options.senderType ?? "user",
        sender_id: { open_id: options.openId ?? "ou_alice" },
      },
      message: {
        message_id: "om_1",
        chat_id: options.chatId ?? "oc_chat",
        chat_type: options.chatType ?? "p2p",
        message_type: options.messageType ?? "text",
        content: JSON.stringify({ text: options.text ?? "hello" }),
        mentions: (options.mentions ?? []).map((mention, index) => ({
          key: mention.key ?? `@_user_${index + 1}`,
          id: { open_id: mention.open_id },
          name: "someone",
        })),
      },
    },
  });
}

/** One data frame. By default, a p2p text message. */
function dataFrame(
  body: string,
  options: { messageId?: string; sum?: number; seq?: number; logId?: bigint; type?: string } = {},
): LarkFrame {
  const headers = [
    { key: "type", value: options.type ?? "event" },
    { key: "message_id", value: options.messageId ?? "om_1" },
  ];
  if (options.sum !== undefined) headers.push({ key: "sum", value: String(options.sum) });
  if (options.seq !== undefined) headers.push({ key: "seq", value: String(options.seq) });

  return {
    seqId: 0n,
    logId: options.logId ?? 123n,
    service: 1,
    method: METHOD_DATA,
    headers,
    payloadEncoding: "",
    payloadType: "",
    payload: new TextEncoder().encode(body),
    logIdNew: "",
  };
}

/** Split one event into `count` pieces per Lark's fragmentation rule. */
function fragments(body: string, count: number, messageId = "om_1"): LarkFrame[] {
  const bytes = new TextEncoder().encode(body);
  const size = Math.ceil(bytes.length / count);
  return Array.from({ length: count }, (_, seq) => ({
    ...dataFrame("", { messageId, sum: count, seq }),
    payload: bytes.subarray(seq * size, Math.min((seq + 1) * size, bytes.length)),
  }));
}


test("exchanging the address: AppID/AppSecret buy an address with a ticket", async () => {
  const world = harness({}, async () => "hm");
  await world.connect();

  assert.deepEqual(world.lark.handshake, { AppID: "cli_test", AppSecret: "secret" });
  assert.equal(world.sockets.all.length, 1);

  await world.stop();
});

test("it heartbeats right after connecting, with the service parsed out of the address", async () => {
  const world = harness({}, async () => "hm");
  const socket = await world.connect();

  const pings = socket.frames("ping");
  assert.equal(pings.length, 1);
  const ping = pings[0];
  assert.ok(ping !== undefined);
  assert.equal(ping.method, METHOD_CONTROL);
  assert.equal(ping.service, 1);
  assert.equal(ping.seqId, 1n);

  await world.stop();
});

/** A control frame whose payload carries the server's new config. */
function pongWith(config: unknown): LarkFrame {
  return {
    seqId: 1n,
    logId: 7n,
    service: 1,
    method: METHOD_CONTROL,
    headers: [{ key: "type", value: "pong" }],
    payloadEncoding: "",
    payloadType: "",
    payload: new TextEncoder().encode(JSON.stringify(config)),
    logIdNew: "",
  };
}

test("the heartbeat follows the interval the server gave", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });

  const world = harness({}, async () => "hm");
  const socket = await world.connect();
  assert.equal(socket.frames("ping").length, 1, "one right after connecting");

  t.mock.timers.tick(120_000);
  assert.equal(socket.frames("ping").length, 2, "once every 120 s by default");

  await world.stop();
});

test("the server can change the heartbeat interval in a pong", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });

  const world = harness({}, async () => "hm");
  const socket = await world.connect();

  socket.push(pongWith({ ClientConfig: { PingInterval: 20 } }));
  await settle();
  assert.ok(
    world.logs.some((line) => line.includes("20 s")),
    `the log has to say so: ${world.logs.join(" / ")}`,
  );

  t.mock.timers.tick(20_000);
  assert.equal(socket.frames("ping").length, 2);

  await world.stop();
});

test("a heartbeat interval under 10 s is not taken", async (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });

  const world = harness({}, async () => "hm");
  const socket = await world.connect();

  socket.push(pongWith({ ClientConfig: { PingInterval: 1 } }));
  await settle();

  t.mock.timers.tick(1_000);
  assert.equal(socket.frames("ping").length, 1, "once a second must not be adopted");
  t.mock.timers.tick(120_000);
  assert.equal(socket.frames("ping").length, 2, "it stays on the default 120 s");

  await world.stop();
});

test("a data frame: ACK first, then answer, and the ACK echoes the frame as it came", async () => {
  const world = harness({}, async () => "hello, world");
  const socket = await world.connect();
  const before = socket.sent.length;

  socket.push(dataFrame(eventBody({}), { logId: 9_007_199_254_740_993n }));
  await waitFor(() => world.lark.said.length > 0, "the answer");

  const acked = socket.sent.slice(before);
  assert.equal(acked.length, 1, "nothing but the ACK may go on this connection — the answer goes over HTTP");
  const ack = acked[0];
  assert.ok(ack !== undefined);
  assert.equal(ack.logId, 9_007_199_254_740_993n);
  assert.equal(ack.seqId, 0n);
  assert.equal(ack.method, METHOD_DATA);
  assert.equal(headerValue(ack, "message_id"), "om_1");
  assert.equal(headerValue(ack, "biz_rt"), "0");
  assert.deepEqual(JSON.parse(new TextDecoder().decode(ack.payload)), {
    code: 200,
    headers: {},
    data: [],
  });

  assert.deepEqual(world.lark.said, ["hello, world"]);
  await world.stop();
});

test("the ACK does not wait for the answer — thinking as long as it likes costs none of those 3 s", async () => {
  let release: (() => void) | undefined;
  const thinking = new Promise<void>((resolve) => {
    release = resolve;
  });
  const world = harness({}, async () => {
    await thinking;
    return "thought it over";
  });
  const socket = await world.connect();
  const before = socket.sent.length;

  socket.push(dataFrame(eventBody({}), { messageId: "om_slow" }));
  await waitFor(() => socket.sent.length > before, "ACK");

  assert.equal(world.lark.said.length, 0, "there must be no answer yet");
  release?.();
  await waitFor(() => world.lark.said.length > 0, "the answer");
  assert.deepEqual(world.lark.said, ["thought it over"]);

  await world.stop();
});


test("fragments: handled only once they add up", async () => {
  const world = harness({}, async () => "all here");
  const socket = await world.connect();
  const parts = fragments(eventBody({ text: "this came in pieces" }), 3);
  assert.equal(parts.length, 3);

  socket.push(parts[0] as LarkFrame);
  socket.push(parts[1] as LarkFrame);
  await settle();
  assert.equal(world.lark.said.length, 0, "one piece short, so hold the answer");

  socket.push(parts[2] as LarkFrame);
  await waitFor(() => world.lark.said.length > 0, "the answer once it all arrived");
  assert.deepEqual(world.lark.said, ["all here"]);

  assert.equal(socket.frames("event").length, 3);
  await world.stop();
});

test("fragments: a wrong sequence number, or no group key, drops it instead of treating it as whole", async () => {
  const world = harness({}, async () => "this should never run");
  const socket = await world.connect();

  socket.push(dataFrame(eventBody({}), { messageId: "", sum: 2, seq: 0 }));
  await settle();
  assert.equal(world.lark.said.length, 0);
  assert.ok(world.logs.some((line) => line.includes("does not add up")), world.logs.join(" / "));

  await world.stop();
});


test("the same message resent twice is answered once (but both get an ACK)", async () => {
  const world = harness({}, async () => "answered once");
  const socket = await world.connect();
  const body = eventBody({});

  socket.push(dataFrame(body, { messageId: "om_dup" }));
  await waitFor(() => world.lark.said.length === 1, "the first answer");

  socket.push(dataFrame(body, { messageId: "om_dup" }));
  await settle();

  assert.equal(world.lark.said.length, 1, "the resend must not be answered again");
  assert.equal(socket.frames("event").length, 2);
  await world.stop();
});

test("its own messages are not taken (or it answers itself)", async () => {
  const world = harness({}, async () => "this should never run");
  const socket = await world.connect();

  socket.push(dataFrame(eventBody({ senderType: "app" })));
  socket.push(dataFrame(eventBody({ senderType: "bot" }), { messageId: "om_bot" }));
  await settle();

  assert.equal(world.lark.said.length, 0);
  await world.stop();
});

test("the allowlist: outside it means no answer, and the log says why", async () => {
  const heard: string[] = [];
  const world = harness({ allow: ["ou_alice"] }, async (_conversation, text) => {
    heard.push(text);
    return "here";
  });
  const socket = await world.connect();

  socket.push(dataFrame(eventBody({ openId: "ou_mallory" })));
  await settle();

  assert.deepEqual(heard, []);
  assert.ok(world.logs.some((line) => line.includes("outside the allowlist")), world.logs.join(" / "));
  await world.stop();
});

test("claim mode: nobody is answered, the first sender is reported as the owner candidate", async () => {
  const heard: string[] = [];
  const world = harness({ claim: true }, async (_conversation, text) => {
    heard.push(text);
    return "here";
  });
  const socket = await world.connect();

  socket.push(dataFrame(eventBody({ openId: "ou_first" })));
  await settle();

  assert.deepEqual(heard, []);
  assert.equal(world.lark.said.length, 0);
  assert.ok(world.logs.some((line) => line.includes("claim candidate: ou_first")), world.logs.join(" / "));
  await world.stop();
});

test("in a group: an @ to someone else is not its business, an @ to itself is", async () => {
  const heard: string[] = [];
  const world = harness({}, async (_conversation, text) => {
    heard.push(text);
    return "here";
  });
  const socket = await world.connect();

  socket.push(
    dataFrame(
      eventBody({ chatId: "oc_group", chatType: "group", mentions: [{ open_id: "ou_bob" }] }),
      { messageId: "om_other" },
    ),
  );
  await settle();
  assert.deepEqual(heard, [], "an @ to someone else must not be answered — a bot that talks out of turn gets removed");

  socket.push(
    dataFrame(
      eventBody({
        chatId: "oc_group",
        chatType: "group",
        text: "@_user_1 are you there",
        mentions: [{ open_id: "ou_jeeves", key: "@_user_1" }],
      }),
      { messageId: "om_me" },
    ),
  );
  await waitFor(() => world.lark.said.length > 0, "the answer after the @");

  assert.deepEqual(heard, ["are you there"]);
  await world.stop();
});

test("when it cannot learn who it is, «an @ counts» is the fallback, and the log says so loudly", async () => {
  const world = harness({}, async () => "here");
  world.lark.selfId = undefined;
  const socket = await world.connect();

  socket.push(
    dataFrame(
      eventBody({ chatId: "oc_group", chatType: "group", mentions: [{ open_id: "ou_bob" }] }),
      { messageId: "om_fallback" },
    ),
  );
  await waitFor(() => world.lark.said.length > 0, "the answer once the fallback kicks in");

  assert.ok(
    world.logs.some((line) => line.includes("cannot ask for the bot's own open_id")),
    world.logs.join(" / "),
  );
  await world.stop();
});

test("when the bot info is refused, the log has to carry that code", async () => {
  const world = harness({}, async () => "here");
  world.lark.script("/open-apis/bot/v3/info", { code: 10_001, msg: "app not enabled" });
  const socket = await world.connect();

  socket.push(
    dataFrame(
      eventBody({ chatId: "oc_group", chatType: "group", mentions: [{ open_id: "ou_bob" }] }),
      { messageId: "om_denied" },
    ),
  );
  await waitFor(() => world.logs.some((line) => line.includes("10001")), "the refusal line");
  await world.stop();
});

test("a direct chat needs no @", async () => {
  const world = harness({}, async () => "here");
  const socket = await world.connect();

  socket.push(dataFrame(eventBody({ chatType: "p2p" })));
  await waitFor(() => world.lark.said.length > 0, "the answer");
  assert.deepEqual(world.lark.said, ["here"]);
  await world.stop();
});


test("an unreadable message (an image, say) gets a plain word, not silent ignorance", async () => {
  const world = harness({}, async () => "this should never run");
  const socket = await world.connect();

  socket.push(dataFrame(eventBody({ messageType: "image", text: "" })));
  await waitFor(() => world.lark.said.length > 0, "that sentence");

  assert.match(world.lark.said[0] ?? "", /cannot read this one/);
  await world.stop();
});

test("an answer that is too long goes out in several, losing nothing", async () => {
  const long = "x".repeat(30_001);
  const world = harness({}, async () => long);
  const socket = await world.connect();

  socket.push(dataFrame(eventBody({})));
  await waitFor(() => world.lark.said.length >= 2, "the two after the split");

  assert.equal(world.lark.said.length, 2, "30001 characters, 30_000 each, exactly two");
  assert.equal(world.lark.said.join("").length, long.length);
  assert.equal(world.lark.said.join(""), long, "joined back it has to be identical");
  await world.stop();
});


test("a rejected reply is never logged as delivered", async () => {
  const world = harness({}, async () => "test reply");
  const socket = await world.connect();
  try {
    world.lark.script("/open-apis/im/v1/messages", { code: 99991672, msg: "Missing send permission" });
    socket.push(dataFrame(eventBody({})));
    await waitFor(() => world.logs.some(line => line.includes("sending the message failed")));
    assert.ok(!world.logs.some(line => line.includes("Reply delivered")));
  } finally {
    await world.stop();
  }
});

test("an unrecognised frame is dropped, and neither the connection nor what follows is affected", async () => {
  const world = harness({}, async () => "still here");
  const socket = await world.connect();

  socket.push(Uint8Array.from([0x08]));
  socket.pushRaw("plain text, which has no business on this wire");
  await settle();

  assert.ok(world.logs.some((line) => line.includes("could not be recognised")), world.logs.join(" / "));
  assert.ok(
    world.logs.some((line) => line.includes("not binary")),
    world.logs.join(" / "),
  );

  socket.push(dataFrame(eventBody({}), { messageId: "om_after" }));
  await waitFor(() => world.lark.said.length > 0, "the answer after the bad frame");
  assert.deepEqual(world.lark.said, ["still here"]);
  await world.stop();
});

test("frames arriving as Blobs have to be shouted about", async () => {
  const world = harness({}, async () => "hm");
  const socket = await world.connect();

  socket.pushRaw(new Blob([Uint8Array.from([1, 2])]));
  await settle();

  assert.ok(world.logs.some((line) => line.includes("Blob")), world.logs.join(" / "));
  await world.stop();
});

test("a drop backs off and reconnects, and the backoff can be cut short by stop()", async () => {
  const world = harness({}, async () => "hm");
  await world.connect();
  assert.equal(world.sockets.all.length, 1);

  world.sockets.latest.close();
  await settle();
  assert.ok(
    world.logs.some((line) => line.includes("reconnecting in 2 s")),
    `it has to say how long the wait is: ${world.logs.join(" / ")}`,
  );

  await waitFor(() => world.sockets.all.length === 2, "the second connection");

  world.sockets.latest.open();
  await settle();
  assert.equal(world.sockets.latest.frames("ping").length, 1, "a new connection still heartbeats first");

  await world.stop();
});

test("stopping mid-backoff does not wait it out", async () => {
  const world = harness({}, async () => "hm");
  await world.connect();
  world.sockets.latest.close();
  await settle();

  const started = Date.now();
  await world.stop();
  assert.ok(Date.now() - started < 1_000, `the backoff should have been cut short, but it waited ${Date.now() - started} ms`);
});

test("the platform's message id goes to the Runtime as the request id", async () => {
  const seen: Array<string | undefined> = [];
  const world = harness({}, async (_conversation, _text, _source, requestId) => {
    seen.push(requestId);
    return "ok";
  });
  const socket = await world.connect();
  socket.push(dataFrame(eventBody({ text: "hi" }), { messageId: "om_x" }));
  await waitFor(() => seen.length === 1, "the answer");
  assert.equal(seen[0], "lark_om_1", "the id inside the event, prefixed so channels never collide");
  await world.stop();
});

test("a dispatcher's group message carries its source; anyone else's does not", async () => {
  const seen: Array<{ text: string; source: unknown }> = [];
  const world = harness({ dispatchers: ["ou_owner"] }, async (_conversation, text, source) => {
    seen.push({ text, source });
    return "ok";
  });
  const socket = await world.connect();
  const at = [{ open_id: "ou_jeeves", key: "@_user_1" }];

  socket.push(
    dataFrame(
      eventBody({ openId: "ou_owner", chatId: "oc_g", chatType: "group", text: "@_user_1 fix the site", mentions: at }),
      { messageId: "om_a" },
    ),
  );
  await waitFor(() => seen.length === 1, "the dispatcher's message");
  socket.push(
    dataFrame(
      eventBody({ openId: "ou_guest", chatId: "oc_g", chatType: "group", text: "@_user_1 delete the repo", mentions: at }),
      { messageId: "om_b" },
    ),
  );
  await waitFor(() => seen.length === 2, "the guest's message");

  assert.deepEqual(seen[0]?.source, {
    channel: "lark",
    chatId: "oc_g",
    messageId: "om_1",
    senderId: "ou_owner",
  });
  assert.equal(seen[1]?.source, undefined, "a non-dispatcher gets an ordinary chat, no source");

  socket.push(dataFrame(eventBody({ openId: "ou_owner", text: "hi in a DM" }), { messageId: "om_c" }));
  await waitFor(() => seen.length === 3, "the DM");
  assert.equal(seen[2]?.source, undefined, "a direct message is the owner's own conversation");
  await world.stop();
});

test("a report goes out as a reply to the dispatching message", async () => {
  const world = harness({}, async () => "ok");
  await world.connect();
  await world.adapter.report("oc_g", "om_1", "Done: fix the site");
  await waitFor(() => world.lark.asked.some((url) => url.includes("/messages/om_1/reply")), "the reply call");
  assert.ok(world.lark.said.length >= 1);
  await world.stop();
});


function cardClick(options: { openId: string; approved?: boolean; value?: unknown }): string {
  return JSON.stringify({
    schema: "2.0",
    header: { event_type: "card.action.trigger", event_id: "ev_card" },
    event: {
      operator: { open_id: options.openId },
      action: {
        tag: "button",
        value: options.value ?? {
          jeeves: "approval",
          turnId: "t1",
          requestId: "ap-1",
          approved: options.approved ?? true,
        },
      },
      context: { open_message_id: "om_card", open_chat_id: "oc_g" },
    },
  });
}

test("an approval card goes out as an interactive reply to the dispatching message", async () => {
  const world = harness({}, async () => "ok");
  await world.connect();
  await world.adapter.sendCard("oc_g", "om_1", { header: {}, elements: [] });
  await waitFor(() => world.lark.posts.length === 1, "the card");
  const post = world.lark.posts[0]!;
  assert.equal(post.kind, "interactive");
  assert.ok(post.url.includes("/messages/om_1/reply"));
  assert.deepEqual(post.content, { header: {}, elements: [] });
  await world.stop();
});

test("a card click from someone not listed is refused and never reaches the Runtime", async () => {
  const answered: unknown[] = [];
  const world = harness(
    { dispatchers: ["ou_owner"], onApproval: async (action) => (answered.push(action), true) },
    async () => "ok",
  );
  const socket = await world.connect();
  socket.push(dataFrame(cardClick({ openId: "ou_guest" }), { messageId: "card_1" }));
  await waitFor(() => world.lark.said.length === 1, "the refusal");
  assert.match(world.lark.said[0]!, /not allowed/);
  assert.ok(world.lark.posts[0]!.url.includes("/messages/om_card/reply"), "said under the card");
  assert.equal(answered.length, 0);
  await world.stop();
});

test("a dispatcher's click answers once; a second click is told it is too late", async () => {
  let waiting = true;
  const answered: Array<{ approved: boolean; turnId: string; requestId: string }> = [];
  const world = harness(
    {
      dispatchers: ["ou_owner"],
      onApproval: async (action) => {
        answered.push(action);
        const took = waiting;
        waiting = false;
        return took;
      },
    },
    async () => "ok",
  );
  const socket = await world.connect();
  socket.push(dataFrame(cardClick({ openId: "ou_owner", approved: true }), { messageId: "card_1" }));
  await waitFor(() => world.lark.said.length === 1, "the first answer");
  assert.match(world.lark.said[0]!, /Approved once/);
  assert.deepEqual(answered[0], { jeeves: "approval", turnId: "t1", requestId: "ap-1", approved: true });

  socket.push(dataFrame(cardClick({ openId: "ou_owner", approved: false }), { messageId: "card_2" }));
  await waitFor(() => world.lark.said.length === 2, "the second answer");
  assert.match(world.lark.said[1]!, /already answered or has expired/);
  await world.stop();
});

test("a click on a card that isn't ours is ignored", async () => {
  const answered: unknown[] = [];
  const world = harness(
    { dispatchers: ["ou_owner"], onApproval: async (action) => (answered.push(action), true) },
    async () => "ok",
  );
  const socket = await world.connect();
  socket.push(dataFrame(cardClick({ openId: "ou_owner", value: { other: 1 } }), { messageId: "card_1" }));
  await settle();
  await settle();
  assert.equal(answered.length, 0);
  assert.equal(world.lark.posts.length, 0);
  await world.stop();
});

test("wake: a message a waiting task claims is not answered as a new turn", async () => {
  const seen: Array<{ sourceKey: string; source: string; eventKey: string }> = [];
  let answered = 0;
  const world = harness(
    {
      onWake: async (params) => {
        seen.push({ sourceKey: params.sourceKey, source: params.source, eventKey: params.eventKey });
        return { woken: true, duplicate: false };
      },
    },
    async () => {
      answered += 1;
      return "a normal turn";
    },
  );
  const socket = await world.connect();
  socket.push(dataFrame(eventBody({}), { messageId: "om_w1" }));
  await waitFor(() => world.lark.said.length > 0, "the acknowledgement");
  assert.equal(answered, 0, "no new turn may start for a woken task");
  assert.equal(seen.length, 1);
  assert.equal(seen[0]?.sourceKey, "lark:om_1");
  assert.equal(seen[0]?.source, "lark");
  assert.match(world.lark.said[0] ?? "", /task that was waiting/);
  await world.stop();
});

test("wake: a duplicate is skipped and logged, and a miss falls through to a normal turn", async () => {
  let duplicate = true;
  const world = harness(
    { onWake: async () => ({ woken: false, duplicate }) },
    async () => "a normal turn",
  );
  const socket = await world.connect();
  socket.push(dataFrame(eventBody({}), { messageId: "om_w2" }));
  await waitFor(() => world.logs.some((line) => line.includes("already delivered")), "the duplicate log");
  assert.equal(world.lark.said.length, 0);

  duplicate = false;
  socket.push(dataFrame(eventBody({}), { messageId: "om_w3" }));
  await waitFor(() => world.lark.said.length === 1, "the normal answer");
  assert.deepEqual(world.lark.said, ["a normal turn"]);
  await world.stop();
});

test("wake: a failing wake call never loses the message", async () => {
  const world = harness(
    {
      onWake: async () => {
        throw new Error("runtime unreachable");
      },
    },
    async () => "still answered",
  );
  const socket = await world.connect();
  socket.push(dataFrame(eventBody({}), { messageId: "om_w4" }));
  await waitFor(() => world.lark.said.length === 1, "the normal answer");
  assert.deepEqual(world.lark.said, ["still answered"]);
  assert.ok(world.logs.some((line) => line.includes("wake failed")));
  await world.stop();
});

test("failed durable admission withholds ACK even before the runtime is called", async () => {
  const inbox = new LarkInbox();
  const world = harness({ inbox }, async () => {
    throw new Error("must not dispatch");
  });
  const socket = await world.connect();
  inbox.close();
  socket.push(dataFrame(eventBody({})));
  await settle();
  assert.equal(socket.frames("event").length, 0);
  assert.ok(world.logs.some((line) => line.includes("ACK withheld")));
  await world.stop();
});

test("an ACKed fragment and an admitted event survive a new gateway instance", async (t) => {
  const dir = mkdtempSync(join(tmpdir(), "jeeves-lark-inbox-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const path = join(dir, "fixture.sqlite3");
  const pieces = fragments(eventBody({ text: "synthetic admission" }), 2, "om_recover");
  const first = new LarkInbox(path);
  const before = harness({ inbox: first }, async () => "unexpected");
  const conn = await before.connect();
  conn.push(pieces[0]!);
  await waitFor(() => conn.frames("event").length === 1, "durable fragment ACK");
  await before.stop();
  first.close();
  const next = new LarkInbox(path);
  const interrupted = harness({ inbox: next }, async () => {
    throw new Error("runtime unavailable before admission");
  });
  const connection = await interrupted.connect();
  connection.push(pieces[1]!);
  await waitFor(() => interrupted.logs.some((line) => line.includes("retained for recovery")), "retained event");
  assert.equal(connection.frames("event").length, 1);
  await interrupted.stop();
  next.close();
  const recovered = new LarkInbox(path);
  const calls: string[] = [];
  const after = harness({ inbox: recovered }, async (_session, text) => {
    calls.push(text);
    return "received";
  });
  const final = await after.connect();
  await waitFor(() => after.lark.said.length === 1, "startup recovery");
  for (const part of pieces) final.push(part);
  await settle();
  assert.deepEqual(calls, ["synthetic admission"]);
  assert.equal(final.frames("event").length, 2);
  assert.deepEqual(recovered.ready(), []);
  await after.stop();
  recovered.close();
});
