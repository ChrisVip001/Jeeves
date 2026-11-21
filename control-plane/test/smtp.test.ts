import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { smtpMailer } from "../src/smtp.ts";

test("the account SMTP transport requires TLS and rejects injected mailbox headers", async () => {
  const create = (url: string) =>
    smtpMailer(
      "https://example.test",
      "me@example.test",
      url,
      "fixture",
      "test-only",
    );
  assert.throws(() => create("smtp://localhost"));
  assert.throws(() => create("smtps://user:pass@example.test"));
  await assert.rejects(
    create("smtps://localhost")(
      "me@example.test\r\nCc: other@example.test",
      "reset",
      "test",
    ),
  );
});

test("SMTP sends a MIME message with a private action link and returns its Message-ID", async () => {
  const dir = mkdtempSync(join(tmpdir(), "account-smtp-"));
  const bin = join(dir, "curl");
  writeFileSync(
    bin,
    `#!/bin/sh
printf '%s\\n' "$@" > "${dir}/args"
/bin/cat > "${dir}/message"
`,
    { mode: 0o700 },
  );
  const path = process.env.PATH;
  process.env.PATH = dir;
  try {
    const send = smtpMailer(
      "https://jeeves.example.test",
      "sender@example.test",
      "smtps://mail.example.test:465",
      "fixture",
      "secret-value",
      "LOGIN",
    );
    const receipt = await send("owner@example.test", "reset", "test-token");
    assert.ok(receipt);
    const wire = readFileSync(join(dir, "message"), "utf8");
    assert.ok(wire.includes("Message-ID: " + receipt.messageId + "\r\n"));
    assert.ok(wire.includes("To: owner@example.test\r\n"));
    const body = Buffer.from(wire.split("\r\n\r\n")[1]!, "base64").toString();
    const link = new URL(body.split("\n\n")[1]!);
    assert.equal(link.origin, "https://jeeves.example.test");
    assert.equal(
      new URLSearchParams(link.hash.slice(1)).get("token"),
      "test-token",
    );
    const args = readFileSync(join(dir, "args"), "utf8");
    assert.ok(!args.includes("secret-value") && !args.includes("test-token"));
    assert.ok(args.includes("=smtps"));
    assert.ok(args.includes("--login-options\nAUTH=LOGIN\n"));
  } finally {
    process.env.PATH = path;
  }
});
