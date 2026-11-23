import assert from "node:assert/strict";
import { test } from "node:test";

import { Allowlist, Dedupe, approvalPrompt, parseApprovalReply, pairingCode, sourceKey } from "../src/shared/index.ts";
import type { ApprovalPromptInput } from "../src/shared/index.ts";

const request: ApprovalPromptInput = {
  turnId: "t1",
  requestId: "r1",
  version: 3,
  tool: "run_command",
  argsPreview: "ls -la",
  reason: "not on the allow list",
  expiresInMs: 300_000,
};

test("allowlist: empty allows everyone, listed ids are exact", () => {
  assert.equal(new Allowlist().allows("anyone"), true);
  const list = new Allowlist([" a ", "b"]);
  assert.equal(list.allows("a"), true);
  assert.equal(list.allows("c"), false);
  assert.equal(list.restricted, true);
});

test("allowlist: claim mode answers nobody and only when nobody is listed", () => {
  assert.equal(new Allowlist([], true).claiming, true);
  assert.equal(new Allowlist(["a"], true).claiming, false);
});

test("dedupe: first claim wins, second is a duplicate, expiry frees it", () => {
  let now = 1000;
  const seen = new Dedupe({ ttlMs: 100, max: 10, now: () => now });
  assert.equal(seen.claim("lark:m1"), true);
  assert.equal(seen.claim("lark:m1"), false);
  assert.equal(seen.claim("telegram:m1"), true, "other channel, same id");
  now += 101;
  assert.equal(seen.claim("lark:m1"), true);
});

test("dedupe: evicts the oldest past max", () => {
  const seen = new Dedupe({ ttlMs: 1e9, max: 2 });
  seen.claim("a");
  seen.claim("b");
  seen.claim("c");
  assert.equal(seen.claim("a"), true, "a was evicted");
});

test("sourceKey is <channel>:<messageId>", () => {
  assert.equal(sourceKey("lark", "om_1"), "lark:om_1");
});

test("approval prompt with buttons keeps the ids for the buttons", () => {
  const out = approvalPrompt(request, { buttons: true });
  assert.equal(out.kind, "buttons");
  assert.match(out.text, /run_command/);
  assert.match(out.text, /ls -la/);
});

test("approval prompt degrades to numbered reply with id and version", () => {
  const out = approvalPrompt(request, { buttons: false });
  assert.equal(out.kind, "reply");
  assert.match(out.text, /1/);
  assert.match(out.text, /2/);
  assert.match(out.text, /r1/);
  assert.match(out.text, /v3/);
  assert.match(out.text, /5 min/);
});

test("parseApprovalReply reads 1 / 2 with an id and version", () => {
  assert.deepEqual(parseApprovalReply("1 r1 v3"), { requestId: "r1", version: 3, approved: true });
  assert.deepEqual(parseApprovalReply(" 2 r1 v3 "), { requestId: "r1", version: 3, approved: false });
  assert.equal(parseApprovalReply("1"), null, "no id: ambiguous, never guessed");
  assert.equal(parseApprovalReply("yes r1 v3"), null);
  assert.equal(parseApprovalReply("1 r1"), null, "no version");
});

test("pairing code is short, unambiguous and differs per call", () => {
  const a = pairingCode();
  assert.match(a, /^[A-HJ-NP-Z2-9]{6}$/);
  assert.notEqual(a, pairingCode());
});
