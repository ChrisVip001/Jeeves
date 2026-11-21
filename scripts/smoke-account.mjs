import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { request } from "node:http";
import { createServer } from "node:https";
import { setTimeout as delay } from "node:timers/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { accountMailer } from "../control-plane/src/mail.ts";
import { totp } from "../control-plane/src/factors.ts";
import { Accounts } from "../control-plane/src/accounts.ts";
import { createControlPlane } from "../control-plane/src/server.ts";
import { singleRuntime } from "../control-plane/src/single-runtime.ts";
import { MemoryCluster } from "../control-plane/src/cluster.ts";

const root = fileURLToPath(new URL("../", import.meta.url));
process.env.PLAYWRIGHT_BROWSERS_PATH ??= join(
  root,
  "desktop/src-tauri/browser-runtime/browsers",
);
const { chromium, devices } =
  await import("../jeeves-ts/node_modules/playwright/index.mjs");
const dir = mkdtempSync(join(tmpdir(), "jeeves-account-smoke-"));
const child = spawn("python3", ["scripts/smoke-web.py", "--hold"], {
  cwd: root,
  stdio: ["pipe", "pipe", "inherit"],
});
const done = once(child, "exit");
let browser, gateway, hostedGateway, edge;
async function listen(server) {
  await new Promise((resolve, reject) =>
    server.once("error", reject).listen(0, "127.0.0.1", resolve),
  );
  return server.address().port;
}
try {
  let runtime;
  for await (const line of createInterface({ input: child.stdout })) {
    const ready = line.match(/^Web smoke passed; (http:\/\/127\.0\.0\.1:\d+)/);
    if (ready) {
      runtime = ready[1];
      break;
    }
  }
  assert.ok(runtime, "fixture Runtime did not start");
  execFileSync(
    "openssl",
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-days",
      "1",
      "-subj",
      "/CN=localhost",
      "-keyout",
      join(dir, "test.key"),
      "-out",
      join(dir, "test.crt"),
    ],
    { stdio: "ignore" },
  );
  let gatewayPort;
  edge = createServer(
    {
      key: readFileSync(join(dir, "test.key")),
      cert: readFileSync(join(dir, "test.crt")),
    },
    (req, res) => {
      const up = request(
        {
          hostname: "127.0.0.1",
          port: gatewayPort,
          path: req.url,
          method: req.method,
          headers: { ...req.headers, "x-forwarded-proto": "https" },
        },
        (reply) => {
          res.writeHead(reply.statusCode, reply.headers);
          reply.pipe(res);
        },
      );
      res.once("close", () => up.destroy());
      up.on("error", () => res.destroy());
      req.pipe(up);
    },
  );
  const origin = "https://127.0.0.1:" + (await listen(edge));
  const inbox = new Map();
  const mail = accountMailer(
    origin,
    "jeeves@example.test",
    "test-only",
    async (url, options) => {
      assert.equal(url, "https://api.resend.com/emails");
      const message = JSON.parse(options.body);
      const link = new URL(message.text.split("\n\n")[1]);
      assert.equal(link.origin, origin);
      assert.equal(link.pathname, "/account/");
      inbox.set(message.to[0], link.href);
      return Response.json({ id: "fixture-mail" });
    },
  );
  const email = "owner@example.test";
  const password = "browser-fixture-password";
  const accounts = new Accounts(join(dir, "db"), "test-secret");
  const owner = singleRuntime(accounts, email, password);
  gateway = createControlPlane(accounts, owner.cluster, {
    image: "fixture",
    namespace: "fixture",
    publicModelUrl: "",
    idleMinutes: 1,
    proxy: { upstream: "", key: "", monthlyTokens: 0 },
    mail,
    publicOrigin: origin,
    podAddress: () => runtime,
    single: { ownerId: owner.ownerId, token: "web-fixture-token" },
  }).server;
  gatewayPort = await listen(gateway);
  browser = await chromium.launch({channel: process.env.JEEVES_TEST_BROWSER_CHANNEL});
  async function login(options) {
    const ctx = await browser.newContext({
      ...options,
      locale: "en-US",
      ignoreHTTPSErrors: true,
    });
    const page = await ctx.newPage();
    await page.goto(origin);
    await page.getByLabel("Email address").fill(email);
    await page.getByLabel("Password", { exact: true }).fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    if (!accounts.byId(owner.ownerId).verified) {
      await page.getByRole("heading", { name: "Check your email" }).waitFor();
      await page.goto(inbox.get(email));
      await page.getByLabel("Password", { exact: true }).fill(password);
      await page
        .getByRole("button", { name: "Verify email", exact: true })
        .click();
    }
    await page.getByRole("link", { name: "Open my assistant" }).waitFor();
    assert.equal(
      (await ctx.cookies()).find((c) => c.name === "__Host-jeeves_session")
        ?.httpOnly,
      true,
    );
    return page;
  }
  const mac = await login({
    viewport: { width: 1180, height: 800 },
    userAgent: "Jeeves test Macintosh",
  });
  const phone = await login(devices["iPhone 13"]);
  const app = origin + "/app/" + owner.ownerId + "/";
  const loaded = mac.waitForResponse(
    (r) => r.url().startsWith(app + "api/data") && r.ok(),
  );
  await mac.getByRole("link", { name: "Open my assistant" }).click();
  await loaded;
  const task = await (
    await mac.request.post(app + "api/task", {
      data: { action: "create", title: "Cross-device account fixture" },
    })
  ).json();
  assert.ok(task.task?.id);
  const phoneReady = phone.waitForResponse(
    (r) => r.url().startsWith(app + "api/data") && r.ok(),
  );
  await phone.getByRole("link", { name: "Open my assistant" }).click();
  await phoneReady;
  const tasks = await (await phone.request.get(app + "api/tasks")).json();
  assert.ok(tasks.tasks.some((t) => t.id === task.task.id));
  await phone.locator(".mobile-menu").click();
  await phone
    .getByRole("link", { name: "Account", exact: true })
    .first()
    .click();
  await phone.locator(".account-security > summary").click();
  await phone
    .getByRole("listitem")
    .filter({ hasText: "Mac" })
    .getByRole("button", { name: "Sign out" })
    .click();
  await phone
    .getByRole("listitem")
    .filter({ hasText: "Mac" })
    .waitFor({ state: "detached" });
  assert.equal((await mac.request.get(app + "api/tasks")).status(), 401);
  assert.equal((await phone.request.get(app + "api/tasks")).status(), 200);
  await phone.getByRole("button", { name: "Sign out", exact: true }).click();
  await phone.getByRole("heading", { name: "Sign in to Jeeves" }).waitFor();
  assert.equal((await phone.request.get(app + "api/tasks")).status(), 401);
  const ownerPage = await login({});
  await ownerPage.locator(".account-security > summary").click();
  await ownerPage.getByText("Change password", { exact: true }).click();
  await ownerPage
    .locator(".account-security > details[open]")
    .getByLabel("Current password")
    .fill(password);
  await ownerPage
    .getByLabel("New password")
    .fill("updated-browser-fixture-password");
  await ownerPage.getByRole("button", { name: "Update password" }).click();
  await ownerPage.getByRole("heading", { name: "Sign in to Jeeves" }).waitFor();
  await ownerPage.getByLabel("Email address").fill(email);
  await ownerPage
    .getByLabel("Password", { exact: true })
    .fill("updated-browser-fixture-password");
  await ownerPage.getByRole("button", { name: "Sign in", exact: true }).click();
  await ownerPage.getByRole("link", { name: "Open my assistant" }).waitFor();
  const hostedAccounts = new Accounts(join(dir, "hosted-db"), "test-secret");
  hostedGateway = createControlPlane(hostedAccounts, new MemoryCluster(), {
    image: "fixture",
    namespace: "fixture",
    publicModelUrl: "http://model",
    idleMinutes: 20,
    proxy: {
      upstream: "https://model.example.test",
      key: "fixture",
      monthlyTokens: 100,
    },
    mail,
    publicOrigin: origin,
  }).server;
  gatewayPort = await listen(hostedGateway);
  const signup = await (
    await browser.newContext({ locale: "en-US", ignoreHTTPSErrors: true })
  ).newPage();
  await signup.goto(origin);
  await signup.getByRole("button", { name: "Create an account" }).click();
  await signup.getByLabel("Email address").fill("new@example.test");
  await signup
    .getByRole("button", { name: "Create account", exact: true })
    .click();
  await signup.getByRole("heading", { name: "Check your email" }).waitFor();
  const address = "new@example.test";
  const first = "signup-fixture-password";
  const second = "reset-fixture-password";
  const verify = inbox.get(address);
  await signup.goto(verify);
  await signup.getByLabel("Password", { exact: true }).fill(first);
  await signup
    .getByRole("button", { name: "Verify email", exact: true })
    .click();
  await signup.getByText(address, { exact: true }).waitFor();
  const verifyToken = new URLSearchParams(new URL(verify).hash.slice(1)).get(
    "token",
  );
  assert.equal(
    (
      await signup.request.post(origin + "/api/verify", {
        data: { email: address, password: first, token: verifyToken },
      })
    ).status(),
    400,
  );
  await signup.locator(".account-security > summary").click();
  await signup
    .locator("summary")
    .filter({ hasText: "Two-factor authentication" })
    .click();
  await signup
    .locator(".account-security > details[open]")
    .getByLabel("Current password")
    .fill(first);
  await signup.getByRole("button", { name: "Set up authenticator" }).click();
  const key = await signup.locator("details code").innerText();
  const setupStep = Math.floor(Date.now() / 30000);
  await signup
    .getByLabel("Authenticator or recovery code")
    .fill(totp(key, setupStep));
  await signup.getByRole("button", { name: "Confirm enrollment" }).click();
  const backups = (await signup.locator("details pre").innerText()).split("\n");
  assert.equal(backups.length, 8);
  const mobile = await (
    await browser.newContext({
      ...devices["iPhone 13"],
      locale: "en-US",
      ignoreHTTPSErrors: true,
    })
  ).newPage();
  await mobile.goto(origin);
  await mobile.getByRole("button", { name: "Forgot password" }).click();
  await mobile.getByLabel("Email address").fill(address);
  await mobile.getByRole("button", { name: "Send reset email" }).click();
  await mobile.getByRole("heading", { name: "Check your email" }).waitFor();
  const reset = inbox.get(address);
  await mobile.goto(reset);
  await mobile.getByLabel("New password").fill(second);
  await mobile.getByRole("button", { name: "Update password" }).click();
  await mobile.getByRole("heading", { name: "Sign in to Jeeves" }).waitFor();
  assert.equal(
    (await signup.request.get(origin + "/api/account")).status(),
    401,
  );
  const resetToken = new URLSearchParams(new URL(reset).hash.slice(1)).get(
    "token",
  );
  assert.equal(
    (
      await mobile.request.post(origin + "/api/reset", {
        data: { email: address, password: first, token: resetToken },
      })
    ).status(),
    400,
  );
  assert.equal(
    (
      await mobile.request.post(origin + "/api/login", {
        data: { email: address, password: first },
      })
    ).status(),
    401,
  );
  await mobile.getByLabel("Password", { exact: true }).fill(second);
  await mobile.getByRole("button", { name: "Sign in", exact: true }).click();
  await mobile
    .getByRole("heading", { name: "Two-factor authentication" })
    .waitFor();
  assert.equal(
    (await mobile.request.get(origin + "/api/account")).status(),
    401,
  );
  await mobile.getByLabel("Authenticator or recovery code").fill(backups[0]);
  await mobile.getByRole("button", { name: "Sign in", exact: true }).click();
  await mobile.getByText(address, { exact: true }).waitFor();
  await signup.goto(origin);
  await signup.getByLabel("Email address").fill(address);
  await signup.getByLabel("Password", { exact: true }).fill(second);
  await signup.getByRole("button", { name: "Sign in", exact: true }).click();
  await signup.getByLabel("Authenticator or recovery code").fill(backups[0]);
  await signup.getByRole("button", { name: "Sign in", exact: true }).click();
  await signup.getByRole("alert").waitFor();
  await delay(Math.max(0, (setupStep + 1) * 30000 - Date.now()));
  await signup
    .getByLabel("Authenticator or recovery code")
    .fill(totp(key, Math.floor(Date.now() / 30000)));
  await signup.getByRole("button", { name: "Sign in", exact: true }).click();
  await signup.getByText(address, { exact: true }).waitFor();
  assert.equal(
    (await (await signup.request.get(origin + "/api/account")).json()).app,
    (await (await mobile.request.get(origin + "/api/account")).json()).app,
  );
  await mobile.locator(".account-security > summary").click();
  await mobile
    .locator("summary")
    .filter({ hasText: "Two-factor authentication" })
    .click();
  await mobile
    .locator(".account-security > details[open]")
    .getByLabel("Current password")
    .fill(second);
  await mobile.getByLabel("Authenticator or recovery code").fill(backups[1]);
  await mobile
    .getByRole("button", { name: "Disable two-factor authentication" })
    .click();
  await mobile.getByRole("button", { name: "Set up authenticator" }).waitFor();
  assert.equal(
    (await signup.request.get(origin + "/api/account")).status(),
    401,
  );
  assert.equal(hostedAccounts.all().length, 1);
  console.log(
    "Account smoke passed: registration, HTTPS login, desktop/mobile workspace, shared task, session revocation, password change, verified signup, email reset, authenticator login and one-use recovery.",
  );
} finally {
  await browser?.close();
  for (const server of [gateway, hostedGateway, edge]) {
    server?.closeAllConnections();
    server?.close();
  }
  if (!child.stdin.destroyed) child.stdin.end("\n");
  const [exit] = await done;
  rmSync(dir, { recursive: true, force: true });
  assert.equal(exit, 0, "fixture Runtime failed");
}
