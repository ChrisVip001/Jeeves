import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Accounts } from "../src/accounts.ts";
import { MemoryCluster, Kubectl } from "../src/cluster.ts";
import { userManifests } from "../src/manifests.ts";
import { Tally } from "../src/proxy.ts";
import { createControlPlane } from "../src/server.ts";

const fresh = () => new Accounts(join(mkdtempSync(join(tmpdir(), "cp-")), "db.json"), "s3cret");

test("passwords are checked, sessions are signed and expire", () => {
  const a = fresh();
  const u = a.register("A@x.io", "long-enough-pw");
  assert.throws(() => a.register("a@x.io", "long-enough-pw"), /already/);
  assert.throws(() => a.login("a@x.io", "wrong-password"), /wrong/);
  assert.equal(a.login("a@x.io", "long-enough-pw").id, u.id);
  const cookie = a.session(u.id);
  assert.equal(a.fromSession(cookie)?.id, u.id);
  assert.equal(a.fromSession(cookie.slice(0, -1) + (cookie.endsWith("0") ? "1" : "0")), undefined);
  assert.equal(a.fromSession(a.session(u.id, -1)), undefined);
});

test("a Pod never receives the platform model key, only its own tokens", () => {
  const docs = JSON.stringify(
    userManifests({ id: "u-1", image: "jeeves", opsToken: "o", modelToken: "m", modelBaseUrl: "http://cp", awake: true }),
  );
  assert.ok(!/api[_-]?key/i.test(docs));
  assert.ok(docs.includes("secretKeyRef"));
  assert.ok(docs.includes('"Recreate"'));
});

test("usage tally reads input first and output last across chunk edges", () => {
  const t = new Tally();
  t.feed('data: {"usage":{"input_tok');
  t.feed('ens":120}}\n\ndata: {"usage":{"output_tokens":30}}');
  assert.equal(t.total, 150);
});

test("register provisions a sleeping Pod; the app route wakes it; idle sweep sleeps it", async () => {
  let secondPageStatus = "running";
  let taskListAvailable = true;
  const pod = createServer((req, res) => {
    if (req.url === "/healthz") return res.end("ok");
    if (req.url === "/api/tasks" && !taskListAvailable) { res.statusCode = 503; return res.end(); }
    if (req.url === "/api/tasks") return res.end(JSON.stringify({ tasks: [], nextCursor: "next" }));
    if (req.url === "/api/tasks?cursor=next") return res.end(JSON.stringify({ tasks: [{ status: secondPageStatus }] }));
    res.end(JSON.stringify({ auth: req.headers.authorization, path: req.url }));
  });
  await new Promise<void>((r) => pod.listen(0, r));
  const podPort = (pod.address() as AddressInfo).port;
  const accounts = fresh();
  const cluster = new MemoryCluster();
  const { server, sweep } = createControlPlane(accounts, cluster, {
    image: "jeeves", namespace: "jeeves", publicModelUrl: "http://cp", idleMinutes: 1,
    proxy: { upstream: "http://x", key: "platform-key", monthlyTokens: 10 },
    podAddress: () => `http://127.0.0.1:${podPort}`,
  });
  await new Promise<void>((r) => server.listen(0, r));
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const reg = await fetch(`${base}/api/register`, { method: "POST", body: JSON.stringify({ email: "b@x.io", password: "long-enough-pw" }) });
  assert.equal(reg.status, 200);
  const cookie = reg.headers.get("set-cookie")!.split(";")[0]!;
  const [{ pod: spec, replicas }] = [...cluster.pods.values()];
  assert.equal(replicas, 0);
  assert.ok(!JSON.stringify(spec).includes("platform-key"));
  assert.equal((await fetch(`${base}/app/x`)).status, 401);
  const got = await (await fetch(`${base}/app/api/ping`, { headers: { cookie } })).json() as { auth: string; path: string };
  assert.equal(got.path, "/api/ping");
  assert.equal(got.auth, `Bearer ${accounts.all()[0]!.opsToken}`);
  assert.equal([...cluster.pods.values()][0]!.replicas, 1);
  await sweep(Date.now() + 2 * 60_000);
  assert.equal([...cluster.pods.values()][0]!.replicas, 1);
  secondPageStatus = "done";
  taskListAvailable = false;
  await sweep(Date.now() + 2 * 60_000);
  assert.equal([...cluster.pods.values()][0]!.replicas, 1);
  taskListAvailable = true;
  await sweep(Date.now() + 2 * 60_000);
  assert.equal([...cluster.pods.values()][0]!.replicas, 0);
  assert.equal(accounts.all()[0]!.awake, false);
  accounts.addUsage(accounts.all()[0]!.id, 10);
  const q = await fetch(`${base}/v1/messages`, { method: "POST", headers: { authorization: `Bearer ${accounts.all()[0]!.modelToken}` }, body: "{}" });
  assert.equal(q.status, 429);
  server.close();
  pod.close();
});

test("the Pod's seeded model profile points at the proxy and names the token by env pointer", () => {
  const docs = JSON.stringify(
    userManifests({ id: "u-1", image: "jeeves", opsToken: "o", modelToken: "jeeves-secret-token", modelBaseUrl: "http://cp/v1", awake: true }),
  );
  assert.ok(docs.includes("env:JEEVES_MODEL_TOKEN") && docs.includes("base_url") && docs.includes("key_source"));
  assert.equal(docs.split("jeeves-secret-token").length - 1, 1);
});

test("an always-on account is never put to sleep by the idle sweep", async () => {
  const accounts = fresh();
  const cluster = new MemoryCluster();
  const { sweep } = createControlPlane(accounts, cluster, {
    image: "jeeves", namespace: "jeeves", publicModelUrl: "http://cp", idleMinutes: 1,
    proxy: { upstream: "http://x", key: "k", monthlyTokens: 1 },
  });
  const u = accounts.register("c@x.io", "long-enough-pw");
  u.alwaysOn = true;
  accounts.touch(u.id, true);
  await cluster.apply({ id: u.id, image: "jeeves", opsToken: "o", modelToken: "m", modelBaseUrl: "x", awake: true });
  await sweep(Date.now() + 3_600_000);
  assert.equal(cluster.pods.get(u.id)!.replicas, 1);
});

test("failed persistence leaves registration and usage retryable", () => {
  const a = fresh();
  const file = a.file;
  a.file = join(file, "unavailable", "db.json");
  assert.throws(() => a.register("retry@example.test", "fixture-only-pw"));
  assert.equal(a.all().length, 0);
  a.file = file;
  const u = a.register("retry@example.test", "fixture-only-pw");
  a.file = join(file, "unavailable");
  assert.throws(() => a.addUsage(u.id, 100));
  assert.equal(a.usage(u.id), 0);
  assert.throws(() => a.touch(u.id, true));
  assert.equal(a.byId(u.id)?.awake, false);
});

test("model quota counts only protocol usage and ignores generated content", () => {
  const t = new Tally();
  t.feed('data: {"type":"content_block_start","content_block":{"input":{"input_tokens":90000}}}\n\n');
  t.feed('data: {"type":"message_start","message":{"usage":{"input_tokens":12}}}\n');
  t.feed('data: {"type":"message_delta","usage":{"output_tokens":3}}');
  assert.equal(t.total, 15);
});

test("remove targets the resource names used by provisioning", {skip: process.platform === "win32"}, async () => {
  const dir = mkdtempSync(join(tmpdir(), "jeeves-kubectl-fixture-"));
  const log = join(dir, "calls");
  const executable = join(dir, "kubectl");
  writeFileSync(executable, '#!/bin/sh\nprintf "%s\\n" "$@" >> "$JEEVES_TEST_CLUSTER_LOG"\n', {mode: 0o700});
  const old = process.env.PATH;
  const oldLog = process.env.JEEVES_TEST_CLUSTER_LOG;
  process.env.PATH = dir;
  process.env.JEEVES_TEST_CLUSTER_LOG = log;
  try { await new Kubectl().remove("u-fixture"); } finally {
    process.env.PATH = old;
    if (oldLog === undefined) delete process.env.JEEVES_TEST_CLUSTER_LOG;
    else process.env.JEEVES_TEST_CLUSTER_LOG = oldLog;
  }
  assert.deepEqual(readFileSync(log, "utf8").trim().split("\n"), [
    "-n", "jeeves", "delete", "deployment/u-fixture", "service/u-fixture", "secret/u-fixture-env", "--ignore-not-found",
    "-n", "jeeves", "delete", "pvc", "u-fixture-home", "--ignore-not-found",
  ]);
  rmSync(dir, { recursive: true });
});
