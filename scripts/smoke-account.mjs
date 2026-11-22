import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { createHmac } from "node:crypto";
import { once } from "node:events";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import { createServer as tcp } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { chromium, devices } from "../node_modules/playwright/index.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const directory = mkdtempSync(join(tmpdir(), "jeeves-cloud-account-"));
const children = [];
const inbox = new Map();
const mail = createServer(async (req, res) => {
  let raw = "";
  for await (const chunk of req) raw += chunk;
  const letter = JSON.parse(raw);
  const lines = letter.text.split("\n\n");
  const address = letter.to[0];
  inbox.set(address, { link: lines[1], code: lines[2] });
  res.writeHead(202).end();
});
let browser;
const stopChildren = () => {
  for (const child of children) if (child.exitCode === null) child.kill("SIGTERM");
};
process.on("SIGINT", stopChildren);
process.on("SIGTERM", stopChildren);

function authenticator(secret) {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  const bits = [...secret].map(letter => alphabet.indexOf(letter).toString(2).padStart(5, "0")).join("");
  const key = Buffer.from((bits.match(/.{8}/g) ?? []).map(value => parseInt(value, 2)));
  const time = Buffer.alloc(8);
  time.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000)));
  const hash = createHmac("sha1", key).update(time).digest();
  const start = hash.at(-1) & 15;
  return String((hash.readUInt32BE(start) & 0x7fffffff) % 1000000).padStart(6, "0");
}

async function freePort() {
  const server = tcp();
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const address = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return address;
}

function run(program, args, env = {}) {
  const child = spawn(program, args, { cwd: root, env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "inherit"] });
  children.push(child);
  return child;
}

async function poll(check, label, timeout = 20000) {
  const until = Date.now() + timeout;
  while (Date.now() < until) {
    try {
      const value = await check();
      if (value) return value;
    } catch {}
    await delay(120);
  }
  throw new Error("Timed out: " + label);
}

async function assertTealTheme(page, primary) {
  for (const [scheme, canvas, accent] of [
    ["light", "#f1f2f0", "rgb(29, 101, 99)"],
    ["dark", "#121615", "rgb(108, 181, 174)"],
  ]) {
    await page.emulateMedia({ colorScheme: scheme });
    assert.equal(await primary.evaluate(el => getComputedStyle(el).backgroundColor), accent);
    assert.equal(await page.locator("html").evaluate(el => getComputedStyle(el).getPropertyValue("--bg").trim()), canvas);
  }
  await page.emulateMedia({ colorScheme: "no-preference" });
}

try {
  await new Promise(resolve => mail.listen(0, "127.0.0.1", resolve));
  const fixture = run("python3", ["-u", "scripts/cloud-runtime-fixture.py"], { PYTHONDONTWRITEBYTECODE: "1" });
  let runtimeURL;
  for await (const line of createInterface({ input: fixture.stdout })) {
    if (line.startsWith("http://127.0.0.1:")) { runtimeURL = line; break; }
  }
  assert.ok(runtimeURL, "Rust Runtime failed to start");
  const control = join(directory, "control-plane");
  const gateway = join(directory, "gateway");
  execFileSync("go", ["build", "-o", control, "./control-plane"], { cwd: root, stdio: "inherit" });
  execFileSync("go", ["build", "-o", gateway, "./gateway"], { cwd: root, stdio: "inherit" });
  const key = join(directory, "tls.key");
  const cert = join(directory, "tls.crt");
  execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
    "-subj", "/CN=localhost", "-keyout", key, "-out", cert], { stdio: "ignore" });
  const controlPort = await freePort();
  const gatewayPort = await freePort();
  assert.equal((await fetch(runtimeURL + "/")).status, 404, "Runtime must not serve a Web entry");
  assert.equal((await fetch(runtimeURL + "/api/tasks")).status, 401, "Runtime must require a service credential");
  const controlOrigin = "http://127.0.0.1:" + controlPort;
  const origin = "https://127.0.0.1:" + gatewayPort;
  const serviceKey = "local-fixture-private-service-key-123";
  const database = name => {
    const supplied = process.env["JEEVES_TEST_" + name.toUpperCase() + "_DATABASE"];
    if (!supplied) return "sqlite:" + join(directory, name + ".sqlite");
    const url = new URL(supplied);
    assert.equal(url.protocol, "postgres:");
    assert.ok(["localhost", "127.0.0.1"].includes(url.hostname) && url.pathname === "/jeeves_test_" + name);
    return url.toString();
  };
  run(control, [], {
    CP_DEVELOPMENT: "1", CP_ADDR: "127.0.0.1:" + controlPort,
    CP_DATABASE_URL: database("control"), CP_STATE_KEY: "control-plane-fixture-storage-12345",
    CP_GATEWAY_TOKEN: serviceKey, CP_TEST_RUNTIME_URL: runtimeURL, CP_TEST_RUNTIME_TOKEN: "web-fixture-token-private",
  });
  await poll(async () => (await fetch(controlOrigin + "/healthz")).ok, "Go control plane");
  run(gateway, [], {
    GW_DEVELOPMENT: "1", GW_ADDR: "127.0.0.1:" + gatewayPort, GW_PUBLIC_ORIGIN: origin,
    GW_TLS_CERT: cert, GW_TLS_KEY: key,
    GW_DATABASE_URL: database("gateway"), GW_SESSION_KEY: "test-fixture-session-key-12345678",
    GW_STATE_KEY: "test-fixture-storage-key-12345678", GW_CONTROL_TOKEN: serviceKey, GW_CONTROL_URL: controlOrigin,
    GW_MAIL_FROM: "jeeves@example.test", GW_RESEND_API_KEY: "placeholder",
    GW_TEST_MAIL_URL: "http://127.0.0.1:" + mail.address().port,
  });
  browser = await chromium.launch({ channel: process.env.JEEVES_TEST_BROWSER_CHANNEL ?? "chrome" });
  const desktop = await browser.newContext({ ignoreHTTPSErrors: true, locale: "en-US", userAgent: "Test Macintosh" });
  const phone = await browser.newContext({ ...devices["iPhone 13"], ignoreHTTPSErrors: true, locale: "en-US" });
  await poll(async () => (await desktop.request.get(origin + "/healthz")).ok(), "Go gateway");
  const email = "new-user@example.test";
  let password = "new-account-smoke-password";
  const page = await desktop.newPage();
  page.setDefaultTimeout(10000);
  page.setDefaultNavigationTimeout(10000);
  page.on("pageerror", e => console.error("Dashboard browser error:", e.message));
  page.on("response", res => { if (res.url().endsWith("/api/interaction") || res.status() >= 500) console.error("Dashboard reply", res.status(), new URL(res.url()).pathname); });
  let starting = true;
  await page.route("**/api/workspace", async route => {
    if (starting) await route.fulfill({ json: { ready: false, managed: true } });
    else await route.continue();
  });

  await page.goto(origin);
  await assertTealTheme(page, page.locator("#account-submit"));
  await page.getByRole("button", { name: "Create an account" }).click();
  await page.getByLabel("Email address").fill(email);
  await page.locator("#account-submit").click();
  await page.getByRole("heading", { name: "Check your email" }).waitFor();
  const verification = await poll(() => inbox.get(email), "verification email");
  assert.equal(new URL(verification.link).origin, origin);
  await page.getByLabel("Email code").fill(verification.code);
  await page.getByLabel("Email code").press("Enter");
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.locator("#account-submit").click();
  await page.waitForURL(/\/app\/u-[a-f0-9]{12}\//);
  const workspace = new URL(page.url()).pathname;
  const api = origin + workspace + "api/";
  const landing = await desktop.request.get(origin, { maxRedirects: 0 });
  assert.equal(landing.status(), 303);
  assert.equal(landing.headers().location, workspace, "signed-in home must lead straight to the workbench");
  assert.equal((await desktop.request.get(origin + "/main.js")).status(), 404);
  assert.equal((await desktop.request.get(origin + workspace + "main.js")).status(), 200);
  await page.locator("#input").waitFor();
  await poll(() => page.locator("#input").isEnabled(), "workspace composer");
  await page.locator("#input").fill("task-alpha-fixture");
  assert.ok(await page.locator("#send").isDisabled());
  await page.reload();
  await poll(async () => await page.locator("#input").inputValue() === "task-alpha-fixture", "draft after reload during startup");
  starting = false;
  await poll(() => page.locator("#send").isEnabled(), "workspace ready to send");
  await assertTealTheme(page, page.locator("#send"));
  assert.equal(await page.locator("#input").inputValue(), "task-alpha-fixture");
  const policy = await desktop.request.post(api + "session/permission", { data: {
    sessionId: "jeeves-main", mode: "fullAccess", expectedRevision: 1, confirmFullAccess: true,
  } });
  assert.ok(policy.ok(), await policy.text());
  try {
    await Promise.all([
      page.waitForResponse(res => res.url().endsWith("/api/interaction"), {timeout: 12000}),
      page.locator("#send").click({timeout: 10000}),
    ]);
  } catch (error) {
    console.error((await page.locator("body").innerText()).slice(-1700));
    throw error;
  }
  const task = await poll(async () => {
    const payload = await (await desktop.request.get(api + "tasks")).json();
    return payload.tasks?.find(t => t.title === "task-alpha-fixture" && t.status === "done");
  }, "first task").catch(async error => { console.error(await (await desktop.request.get(api+"tasks")).text()); console.error((await page.locator("body").innerText()).slice(-1700)); throw error; });
  await page.locator("#mode-cowork").click();
  await page.locator(`[data-cowork-task="${task.id}"]`).click();
  await page.locator(".work-file").filter({ hasText: "report.txt" }).click();
  const review = page.getByRole("complementary", { name: /report.txt/ });
  assert.equal(await review.locator("pre").textContent(), "task-alpha-fixture");
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    review.getByRole("button", { name: "Save text copy" }).click(),
  ]);
  assert.equal(download.suggestedFilename(), "report.txt");
  assert.equal(readFileSync(await download.path(), "utf8"), "task-alpha-fixture");
  const mobile = await phone.newPage();
  mobile.setDefaultTimeout(10000);
  mobile.setDefaultNavigationTimeout(10000);
  async function login(target) {
    await target.goto(origin);
    await target.getByLabel("Email address").fill(email);
    await target.getByLabel("Password", { exact: true }).fill(password);
    await target.locator("#account-submit").click();
  }
  await login(mobile);
  await mobile.waitForURL(origin + workspace);
  await assertTealTheme(mobile, mobile.locator("#send"));
  await mobile.locator("#mode-cowork").click();
  await mobile.getByRole("button", { name: /task-alpha-fixture/ }).click();
  await mobile.locator("#cowork-input").fill("task-follow-fixture");
  await mobile.locator("#cowork-send").click();
  await poll(async () => {
    const payload = await (await phone.request.get(api + "tasks")).json();
    return payload.tasks?.find(t => t.id === task.id && t.runCount >= 2 && t.status === "done");
  }, "the same task completed after phone revision");
  assert.equal((await phone.request.get(origin + "/app/u-000000000000/api/session/main")).status(), 403);
  assert.equal((await phone.request.post(api + "gateway/rpc", { data: {} })).status(), 404);
  console.log("Registration, cold-start draft recovery, delivery preview/download and cross-client continuation passed");

  await mobile.goto(origin + "/account/");
  await mobile.locator(".account-security > summary").click();
  await mobile.getByRole("listitem").filter({ hasText: "Mac" }).getByRole("button", { name: "Sign out" }).click();
  assert.equal((await desktop.request.get(api + "tasks")).status(), 401);
  console.log("Remote session revocation passed");

  await page.goto(origin + "/account/");
  await page.locator("#account-forgot").click();
  await page.getByLabel("Email address").fill(email);
  await page.locator("#account-submit").click();
  const reset = await poll(() => inbox.get(email) !== verification && inbox.get(email), "password reset email");
  console.log("Reset email received");
  await page.locator("#account-mail-link").fill(reset.link);
  await page.locator("#account-open-link").click();
  password = "changed-account-smoke-password";
  await page.getByLabel("New password").fill(password);
  const [resetReply] = await Promise.all([
    page.waitForResponse(res => res.url().endsWith("/api/reset"), { timeout: 5000 }),
    page.locator("#account-submit").click(),
  ]);
  if (!resetReply.ok()) assert.fail(await resetReply.text());
  console.log("Reset accepted by gateway");
  await page.getByRole("heading", { name: "Sign in to Jeeves" }).waitFor();
  assert.equal((await phone.request.get(api + "tasks")).status(), 401);
  await login(page);
  await page.waitForURL(origin + workspace);
  const receipt = await (await desktop.request.get(api + "tasks")).json();
  assert.ok(receipt.tasks.some(t => t.id === task.id && t.runCount >= 2));
  const cookie = (await desktop.cookies()).find(c => c.name === "__Host-jeeves_session");
  assert.ok(cookie?.secure && cookie.httpOnly);
  console.log("Password recovery and task lookup passed");

  await page.goto(origin + "/account/");
  await page.locator(".account-security > summary").click();
  await page.locator("#account-factor-settings").click();
  await page.locator("#factor-password").fill(password);
  await page.locator("#factor-submit").click();
  const secret = await page.locator("#factor-setup-key").innerText();
  await page.locator("#factor-code").fill(authenticator(secret));
  await page.locator("#factor-submit").click();
  const recovery = (await page.locator(".account-security pre").innerText()).split("\n")[0];
  await page.locator("#account-logout").click();
  await page.getByRole("heading", { name: "Sign in to Jeeves" }).waitFor();
  await login(page);
  await page.getByRole("heading", { name: "Two-factor authentication" }).waitFor();
  await page.locator("#account-code").fill(recovery);
  await page.locator("#account-submit").click();
  await page.waitForURL(origin + workspace);
  assert.ok((await (await desktop.request.get(api + "tasks")).json()).tasks.some(t => t.id === task.id && t.runCount >= 2));
  const secondUse = await phone.request.post(origin + "/api/login", { data: { email, password, code: recovery } });
  assert.equal(secondUse.status(), 403);
  console.log("Go gateway → Go control plane → Rust Runtime passed the full isolated browser acceptance.");
} catch (error) {
  console.error(error);
  throw error;
} finally {
  const waiting = children.map(child => child.exitCode !== null ? Promise.resolve() : once(child, "exit"));
  stopChildren();
  if (browser) await Promise.race([browser.close(), delay(5000)]);
  await Promise.race([Promise.all(waiting), delay(12000)]);
  for (const child of children) if (child.exitCode === null) child.kill("SIGKILL");
  mail.close();
  rmSync(directory, { recursive: true, force: true });
}
