import assert from "node:assert/strict";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Accounts } from "../src/accounts.ts";

test("independent logins can be revoked and private account state is encrypted on disk", () => {
  const file = join(mkdtempSync(join(tmpdir(), "jeeves-identity-")), "state");
  const store = new Accounts(file, "test-only-secret");
  const owner = store.register("owner@example.test", "example-password");
  const first = store.session(owner.id);
  const second = store.session(owner.id);
  assert.notEqual(first, second);
  const disk = readFileSync(file, "utf8");
  for (const value of [
    owner.email,
    owner.opsToken,
    owner.modelToken,
    first,
    second,
  ])
    assert.ok(!disk.includes(value));
  const reopened = new Accounts(file, "test-only-secret");
  const session = reopened.authenticate(first)!;
  assert.equal(reopened.revoke(owner.id, session.id), true);
  assert.equal(reopened.fromSession(first), undefined);
  assert.equal(reopened.fromSession(second)?.id, owner.id);
  assert.throws(() => new Accounts(file, "a-different-key"));
});

import { writeFileSync } from "node:fs";
import { createServer, type Server } from "node:http";
import { type AddressInfo } from "node:net";
import { createControlPlane } from "../src/server.ts";
import { MemoryCluster } from "../src/cluster.ts";
import { singleRuntime } from "../src/single-runtime.ts";

async function listen(server: Server) {
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  return "http://127.0.0.1:" + (server.address() as AddressInfo).port;
}

test("legacy accounts migrate explicitly, keep passwords and quotas, and require a new login", () => {
  const file = join(mkdtempSync(join(tmpdir(), "account-upgrade-")), "state");
  const store = new Accounts(file, "test-key");
  const owner = store.register("test@example.test", "account-password");
  const usage = { [owner.id]: { "2026-01": 18 } };
  writeFileSync(file, JSON.stringify({ accounts: [owner], usage }));
  assert.throws(() => new Accounts(file, "test-key"), /migrate/);
  const migrated = new Accounts(file, "test-key", "unrelated-state-key", true);
  assert.equal(migrated.login(owner.email, "account-password").id, owner.id);
  assert.equal(migrated.usage(owner.id, "2026-01"), 18);
  assert.equal(migrated.sessions(owner.id).length, 0);
  assert.ok(!readFileSync(file, "utf8").includes(owner.email));
  assert.equal(
    singleRuntime(migrated, owner.email, undefined).ownerId,
    owner.id,
  );
  assert.throws(() =>
    singleRuntime(migrated, "different@example.test", "example-password"),
  );
});

test("the gateway scopes account data, checks browser origins and disconnects a signed-out stream", async (t) => {
  const file = join(mkdtempSync(join(tmpdir(), "cp-browser-")), "state");
  const accounts = new Accounts(file, "session-secret");
  const seen: { auth?: string; cookie?: string }[] = [];
  const pod = createServer((req, res) => {
    if (req.url === "/healthz") return res.end("ok");
    seen.push({ auth: req.headers.authorization, cookie: req.headers.cookie });
    if (req.url === "/api/events") {
      res.writeHead(200, { "content-type": "text/event-stream" });
      res.write("data: ready\n\n");
    } else res.end("runtime response");
  });
  const podAddress = await listen(pod);
  const { server } = createControlPlane(accounts, new MemoryCluster(), {
    image: "fixture",
    namespace: "fixture",
    publicModelUrl: "http://model",
    idleMinutes: 20,
    podAddress: () => podAddress,
    proxy: { upstream: "http://model", key: "server-only", monthlyTokens: 100 },
  });
  const base = await listen(server);
  t.after(() => {
    server.closeAllConnections();
    server.close();
    pod.closeAllConnections();
    pod.close();
  });
  const post = (
    path: string,
    value: unknown,
    headers: Record<string, string> = {},
  ) =>
    fetch(base + path, {
      method: "POST",
      headers: { "content-type": "application/json", ...headers },
      body: JSON.stringify(value),
    });
  const login = async (email: string, path = "/api/login") => {
    const response = await post(path, {
      email,
      password: "isolated-test-password",
    });
    assert.equal(response.status, 200);
    const cookie = response.headers.get("set-cookie")!;
    assert.match(cookie, /^__Host-jeeves_session=/);
    for (const flag of ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"])
      assert.ok(cookie.includes(flag));
    return cookie.split(";")[0]!;
  };
  const details = async (cookie: string) =>
    (await fetch(base + "/api/account", { headers: { cookie } })).json();
  assert.equal((await fetch(base + "/api/account")).status, 401);
  assert.equal(
    (
      await post(
        "/api/register",
        { email: "x@test.io", password: "isolated-test-password" },
        { origin: "https://unrelated.invalid" },
      )
    ).status,
    403,
  );
  accounts.register("a@test.io", "isolated-test-password", true);
  accounts.register("b@test.io", "isolated-test-password", true);
  const a = await login("a@test.io");
  const second = await login("a@test.io");
  const b = await login("b@test.io");
  const me = await details(a);
  const peer = await details(b);
  assert.equal(me.sessions.length, 2);
  assert.ok(!JSON.stringify(me).includes(accounts.all()[0]!.opsToken));
  assert.equal(
    (await fetch(base + peer.app + "api/data", { headers: { cookie: a } }))
      .status,
    403,
  );
  assert.equal(
    (
      await fetch(base + me.app + "api/data", {
        headers: { cookie: a, authorization: "Bearer unused" },
      })
    ).status,
    200,
  );
  assert.deepEqual(seen.at(-1), {
    auth: "Bearer " + accounts.all()[0]!.opsToken,
    cookie: undefined,
  });
  const stream = await fetch(base + me.app + "api/events", {
    headers: { cookie: a },
  });
  const reader = stream.body!.getReader();
  assert.ok(!(await reader.read()).done);
  const revoke = (cookie: string, id: string) =>
    fetch(base + "/api/account/sessions/" + id, {
      method: "DELETE",
      headers: { cookie, "x-jeeves-client": "1" },
    });
  assert.equal((await revoke(b, me.current)).status, 404);
  assert.equal(
    (
      await post(
        "/api/logout",
        {},
        { cookie: a, origin: "https://unrelated.invalid" },
      )
    ).status,
    403,
  );
  assert.equal((await revoke(second, me.current)).status, 200);
  const closed = await reader.read().then(
    (r) => r.done,
    () => true,
  );
  assert.equal(closed, true);
  assert.equal(
    (await fetch(base + "/api/account", { headers: { cookie: a } })).status,
    401,
  );
  assert.equal((await details(second)).sessions.length, 1);
  assert.equal((await post("/api/logout", {}, { cookie: second })).status, 200);
  assert.equal(
    (await fetch(base + me.app + "api/data", { headers: { cookie: second } }))
      .status,
    401,
  );
  assert.equal((await details(b)).sessions.length, 1);
});

test("changing the password checks the previous password and invalidates all existing sessions atomically", () => {
  const file = join(mkdtempSync(join(tmpdir(), "account-password-")), "store");
  const accounts = new Accounts(file, "secret");
  const owner = accounts.register("change@example.test", "previous-password");
  const one = accounts.session(owner.id);
  const two = accounts.session(owner.id);
  assert.throws(() =>
    accounts.changePassword(owner.id, "wrong-password", "replacement-password"),
  );
  assert.ok(accounts.fromSession(one));
  accounts.changePassword(
    owner.id,
    "previous-password",
    "replacement-password",
  );
  const saved = new Accounts(file, "secret");
  assert.equal(saved.fromSession(one), undefined);
  assert.equal(saved.fromSession(two), undefined);
  assert.throws(() => saved.login(owner.email, "previous-password"));
  assert.equal(saved.login(owner.email, "replacement-password").id, owner.id);
});

test("an empty single Runtime can only be claimed by its designated mailbox", async (t) => {
  const accounts = new Accounts(
    join(mkdtempSync(join(tmpdir(), "owner-enrollment-")), "db"),
    "fixture",
  );
  const email = "owner@example.test";
  const single = singleRuntime(accounts, email, undefined);
  const inbox: string[] = [];
  const { server } = createControlPlane(accounts, single.cluster, {
    image: "",
    namespace: "",
    publicModelUrl: "",
    idleMinutes: 0,
    proxy: { upstream: "", key: "", monthlyTokens: 0 },
    single: {
      get ownerId() {
        return single.ownerId;
      },
      registrationEmail: single.registrationEmail,
      token: "test",
    },
    mail: async (recipient, _action, token) => {
      assert.equal(recipient, email);
      inbox.push(token);
    },
  });
  const origin = await listen(server);
  t.after(() => {
    server.closeAllConnections();
    server.close();
  });
  const post = (path: string, fields: object) =>
    fetch(origin + path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(fields),
    });
  const stranger = "other@example.test",
    password = "owner-test-password";
  assert.equal((await post("/api/register", { email: stranger })).status, 202);
  assert.equal(inbox.length, 0);
  assert.equal(
    (
      await post("/api/verify", {
        email: stranger,
        password,
        token: accounts.mailToken(stranger, "verify"),
      })
    ).status,
    400,
  );
  assert.equal((await post("/api/register", { email })).status, 202);
  assert.equal(inbox.length, 1);
  const response = await post("/api/verify", {
    email,
    password,
    token: inbox[0],
  });
  assert.equal(response.status, 200);
  const cookie = response.headers.get("set-cookie")!.split(";")[0]!;
  assert.equal(
    (await fetch(origin + "/api/account", { headers: { cookie } })).status,
    200,
  );
  assert.equal(single.ownerId, accounts.byEmail(email)?.id);
  assert.equal((await post("/api/register", { email: stranger })).status, 403);
  assert.equal(accounts.all().length, 1);
});
