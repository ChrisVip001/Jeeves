import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Accounts } from "../src/accounts.ts";
import { totp } from "../src/factors.ts";

const example = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
test("authenticator codes match the six-digit suffix of RFC 6238 SHA-1 vectors", () => {
  assert.equal(totp(example, 1), "287082");
  assert.equal(totp(example, Math.floor(1111111109 / 30)), "081804");
  assert.equal(totp(example, Math.floor(20000000000 / 30)), "353130");
});

test("mail credentials are bound to mailbox, purpose, expiry and current password", (t) => {
  t.mock.timers.enable({ apis: ["Date"], now: new Date("2026-01-01") });
  const store = new Accounts(
    join(mkdtempSync(join(tmpdir(), "identity-")), "state"),
    "test-key",
  );
  const email = "owner@example.test";
  const signup = store.mailToken(email, "verify");
  assert.ok(store.validMail(email, "verify", signup));
  assert.ok(!store.validMail("another@example.test", "verify", signup));
  assert.ok(!store.validMail(email, "reset", signup));
  const account = store.register(email, "first-example-password", true);
  assert.ok(!store.validMail(email, "verify", signup));
  const reset = store.mailToken(email, "reset");
  store.replacePassword(account, "second-example-password");
  assert.ok(!store.validMail(email, "reset", reset));
  const expires = store.mailToken(email, "reset");
  t.mock.timers.tick(15 * 60_000);
  assert.ok(!store.validMail(email, "reset", expires));
});

test("factor enrollment is session-bound, revokes other sessions and survives restart without code reuse", () => {
  const file = join(mkdtempSync(join(tmpdir(), "identity-")), "state");
  const store = new Accounts(file, "test-key");
  const account = store.register(
    "verified@example.test",
    "long-password",
    true,
  );
  const first = store.session(account.id);
  const other = store.session(account.id);
  const session = store.authenticate(first)!.id;
  const key = store.beginFactor(account, session)!;
  const code = totp(key, Math.floor(Date.now() / 30000));
  const pending = store.byId(account.id)!;
  assert.equal(
    store.enableFactor(pending, store.authenticate(other)!.id, code),
    undefined,
  );
  const backups = store.enableFactor(pending, session, code)!;
  const reopened = new Accounts(file, "test-key");
  assert.equal(reopened.authenticate(other), undefined);
  assert.ok(reopened.authenticate(first));
  const enabled = reopened.byId(account.id)!;
  assert.equal(reopened.useFactor(enabled, code), false);
  assert.ok(reopened.useFactor(enabled, backups[0]!));
  assert.equal(
    reopened.useFactor(reopened.byId(account.id)!, backups[0]!),
    false,
  );
  assert.ok(
    reopened.useFactor(reopened.byId(account.id)!, backups[1]!, session),
  );
  assert.equal(reopened.byId(account.id)!.factor, undefined);
});
