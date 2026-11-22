// Run after building the dashboard and jeeves. Uses the existing isolated web smoke and browser runtime.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { webcrypto } from "node:crypto";

const root = fileURLToPath(new URL("../", import.meta.url));
const out = path.join(process.env.CARGO_TARGET_DIR ?? path.join(root, "target"), "acceptance/m4-05");
await mkdir(out, { recursive: true });
process.env.PLAYWRIGHT_BROWSERS_PATH ??= path.join(root, "desktop/src-tauri/browser-runtime/browsers");
const { chromium } = await import("../node_modules/playwright/index.mjs");
const system = spawn("python3", ["scripts/smoke-web.py", "--hold", ...(process.argv.includes("--offline-only") ? [] : ["--mail-followup"])], { cwd: root, stdio: ["pipe", "pipe", "inherit"] });
const done = once(system, "exit");
const pictures = [];
const ratios = new Map();
let browser;
let fixture;
let mailTasks;
let succeeded = false;
try {
  for await (const line of createInterface({ input: system.stdout })) {
    if (line.startsWith("Mail fixture tasks ")) mailTasks = JSON.parse(line.slice("Mail fixture tasks ".length));
    if (line.startsWith("Web smoke passed; ")) {
      fixture = JSON.parse(line.slice("Web smoke passed; ".length));
      break;
    }
  }
  assert.ok(fixture, "Isolated cockpit failed to start");
  browser = await chromium.launch({channel: process.env.JEEVES_TEST_BROWSER_CHANNEL});
  await coldOfflineNotice(browser, fixture);
  if (!process.argv.includes("--offline-only")) await captureViews(browser, fixture);
  succeeded = true;
} finally {
  await browser?.close();
  if (fixture) system.stdin.end(succeeded ? "verify\n" : "close\n");
  const [exit] = await done;
  if (succeeded) assert.equal(exit, 0, "isolated Runtime fixture failed");
}


async function coldOfflineNotice(browser, { origin, workspace, cookie }) {
  const [name, value] = cookie.split("=");
  const context = await browser.newContext({ serviceWorkers: "allow", locale: "en-US" });
  try {
    await context.addCookies([{ name, value, url: origin }]);
    const first = await context.newPage();
    await first.addInitScript(() => {
      localStorage.setItem("jeeves.lang", "en");
      localStorage.setItem("jeeves.setup.dismissed", "1");
    });
    await first.goto(workspace, { waitUntil: "domcontentloaded" });
    await first.waitForFunction(() => navigator.serviceWorker.controller?.state === "activated");
    await first.close();
    await context.setOffline(true);
    const cold = await context.newPage();
    // Discard the HTTP cache. The first, now-closed tab already downloaded the entry bundle.
    const network = await context.newCDPSession(cold);
    await network.send("Network.clearBrowserCache");
    // Context routing blocks transport; set the new tab's navigator.onLine signal as well.
    await network.send("Network.overrideNetworkState", {
      offline:true, latency:0, downloadThroughput:0, uploadThroughput:0,
    });
    const locator = new URLSearchParams({
      deviceId:"offline-fixture", grantId:"fixture-grant", generation:"1",
      taskId:"cold-task", turnId:"cold-turn", approvalRequestId:"cold-request", approvalVersion:"1",
    });
    const response = await cold.goto(workspace + "?" + locator, { waitUntil:"domcontentloaded" });
    assert.ok(response?.fromServiceWorker(), "The cold launch must receive its shell from the installed worker");
    assert.equal(await cold.evaluate(() => navigator.onLine), false);
    const sheet = cold.getByRole("dialog", {name: "Task approval"});
    await sheet.getByText("Jeeves is unreachable. Reconnect to review this request.", {exact:true}).waitFor({ timeout:5000 });
    assert.equal(await sheet.getByRole("button", {name:/Allow once|Deny/}).count(), 0);
    console.log("Freshly installed worker opened the approval notice offline without decision buttons");
  } finally { await context.close(); }
}

async function captureViews(browser, { origin, workspace, cookie }) {
  const [name, value] = cookie.split("=");
  for (const width of [1280, 390]) for (const colorScheme of ["light", "dark"]) {
    const zh = (width === 1280) === (colorScheme === "light");
    const label = (chinese, english) => zh ? chinese : english;
    const ctx = await browser.newContext({
      viewport: { width, height: width === 390 ? 844 : 900 },
      colorScheme, reducedMotion: "reduce", serviceWorkers: "block",
    });
    try {
      await ctx.addCookies([{ name, value, url: origin }]);
      const page = await ctx.newPage();
      page.setDefaultTimeout(8000);
      const errors = [];
      page.on("pageerror", error => errors.push(error.message));
      await page.addInitScript(zh => {
        localStorage.setItem("jeeves.lang", zh ? "zh" : "en");
        localStorage.setItem("jeeves.setup.dismissed", "1");
        // Substitute only the external push transport. Pairing uses this context's disposable key.
        window.fixtureSubscriptions = 0;
        navigator.serviceWorker.getRegistration = async () => ({
          active: {}, pushManager: { subscribe: async () => {
            if (!navigator.userActivation.isActive) throw Error("Subscription lost its user gesture");
            window.fixtureSubscriptions++;
            return { endpoint: "https://fixture.invalid/disposable", toJSON: () => ({keys:{p256dh:"fixture-public",auth:"fixture-auth"}}) };
          } },
        });
        Object.defineProperty(Notification, "requestPermission", { value() { throw Error("Unexpected permission request"); } });
      }, zh);
      // Seed only this disposable browser context. The dashboard must prove possession on reads and decisions.
      await page.goto(workspace + "icon.svg");
      const id = await page.evaluate(async accountDB => {
        const pair = await crypto.subtle.generateKey("Ed25519", false, ["sign", "verify"]);
        const db = await new Promise((resolve, reject) => {
          const req = indexedDB.open(accountDB, 1);
          req.onupgradeneeded = () => req.result.createObjectStore("identity");
          req.onsuccess = () => resolve(req.result);
          req.onerror = () => reject(req.error);
        });
        try {
          await new Promise((resolve, reject) => {
            const tx = db.transaction("identity", "readwrite");
            tx.objectStore("identity").put(pair, "device");
            tx.oncomplete = resolve;
            tx.onerror = () => reject(tx.error);
          });
          return btoa(String.fromCharCode(...new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey))));
        } finally { db.close(); }
      }, new URL(workspace).pathname + "jeeves-approval-device");
      const verifyKey = await webcrypto.subtle.importKey("raw", Buffer.from(id,"base64"),"Ed25519",false,["verify"]);
      const locator = {
        deviceId: id,
        grantId: "fixture-grant", generation: "1", taskId: mailTasks[0], turnId: "fixture-turn",
        approvalRequestId: "fixture-ask", approvalVersion: "1",
      };
      async function verify(signature, decision) {
        const payload = ["jeeves.approval.notice.v1", id, locator.grantId,1,locator.taskId,locator.turnId,locator.approvalRequestId,1,decision];
        assert.equal(await webcrypto.subtle.verify("Ed25519",verifyKey,Buffer.from(signature,"base64"),new TextEncoder().encode(JSON.stringify(payload))), true);
      }
      const review = workspace + "?" + new URLSearchParams(locator);
      const request = {
        turnId: locator.turnId, requestId: locator.approvalRequestId, version: 1, expiresInMs: 120000,
        tool: "write_file", reason: label("任务要在工作目录创建文件。", "The task needs to create a file in its workspace."),
        argsPreview: label("周报/本周摘要.md", "Weekly report/summary.md"),
      };
      const approval = { sessionId: "fixture-session", ownerSessionId: "fixture-session", restored: false,
        request, task: { sessionId: "jeeves-main", id: locator.taskId, title: label("整理本周项目进展", "Summarize this week’s project updates"), cwd: "/Fixture/Reports" } };
      let status = "pending", posts = 0, reads = 0;
      let publicKey = "", paired = false, configured = false, subscriptions = 0;
      const extraKey = Buffer.alloc(32, 5).toString("base64");
      const mixed = {deviceId:Buffer.alloc(32, 9).toString("base64"), grantId:"existing",generation:4,lastSequence:1,revoked:false,capabilities:["approvalReview","desktopControl"]};
      let extra = null, pairs = 0, revokes = 0;
      await page.route("**/*", async route => {
        const url = new URL(route.request().url());
        assert.equal(url.origin, origin);
        const apiPath = "/" + url.pathname.slice(new URL(workspace).pathname.length);
        if (apiPath === "/api/device") {
          const devices = [mixed];
          if (paired) devices.push({deviceId:publicKey,grantId:"fixture",generation:1,lastSequence:0,revoked:false,capabilities:["approvalReview"]});
          if (extra) devices.push(extra);
          return route.fulfill({json:{devices}});
        }
        if (apiPath === "/api/device/pair") {
          assert.deepEqual(route.request().postDataJSON(), {deviceId:extraKey,expectedGeneration:0,capabilities:["approvalReview"]});
          pairs++;
          extra = {deviceId:extraKey,grantId:"extra",generation:1,lastSequence:0,revoked:false,capabilities:["approvalReview"]};
          return route.fulfill({json:extra});
        }
        if (apiPath === "/api/device/revoke") {
          assert.deepEqual(route.request().postDataJSON(), {deviceId:extraKey,expectedGeneration:1});
          revokes++; extra.revoked = true; extra.generation++;
          return route.fulfill({json:extra});
        }
        if (apiPath === "/api/approval/push-key") return route.fulfill({json:{publicKey:configured ? "BAAA" : null}});
        if (apiPath === "/api/approval/push-subscribe") {
          const body = route.request().postDataJSON();
          assert.equal(body.deviceId, publicKey);
          assert.equal(body.endpoint, "https://fixture.invalid/disposable");
          subscriptions++;
          return route.fulfill({json:{}});
        }
        if (apiPath !== "/api/approval/notice") {
          assert.ok(!(apiPath === "/api/approval" && route.request().method() === "POST"));
          return route.continue();
        }
        if (route.request().method() === "GET") {
          reads++;
          await verify(url.searchParams.get("signature"), null);
          url.searchParams.delete("signature");
          assert.deepEqual(Object.fromEntries(url.searchParams), locator);
          if (status === "offline") return route.abort("internetdisconnected");
          return route.fulfill({ json: status === "pending" ? { status, approval } : { status } });
        }
        posts++;
        const {signature, ...body} = route.request().postDataJSON();
        await verify(signature, true);
        assert.deepEqual(body, { ...locator, generation: 1, approvalVersion: 1, approved: true });
        return route.fulfill({ json: { accepted: true } });
      });
      const copied = new URL(review);
      copied.searchParams.set("deviceId", extraKey);
      await page.goto(copied.href, {waitUntil:"domcontentloaded"});
      await page.getByRole("dialog").getByText(label("此通知属于另一台设备，或设备授权已撤销。","This notice belongs to another device, or its grant was revoked.")).waitFor();
      assert.equal(reads,0); assert.equal(posts,0);
      const sheet = page.getByRole("dialog", { name: label("任务审批", "Task approval") });
      const allow = sheet.getByRole("button", { name: label("允许一次", "Allow once"), exact: true });
      const snapshot = async name => {
        const file = [width, colorScheme, label("zh", "en"), name].join("-") + ".png";
        await page.screenshot({ path: path.join(out, file) }); pictures.push(file);
      };
      await page.goto(review, {waitUntil: "domcontentloaded"});
      await allow.waitFor();
      await sheet.getByText(request.argsPreview, {exact: true}).waitFor();
      assert.equal(posts, 0);
      assert.equal(reads, 1);
      assert.ok(await sheet.evaluate(el => el.scrollWidth <= el.clientWidth + 1));
      assert.equal(await sheet.evaluate(el => el.contains(document.activeElement)), true);
      const contrast = await sheet.evaluate(el => {
        const sample = el.querySelector(".facts dd");
        const bg = getComputedStyle(el.querySelector("article")).backgroundColor;
        function luminance(color) {
          const channels = color.match(/[\d.]+/g).slice(0,3).map(Number).map(ch => ch / 255)
            .map(ch => ch <= 0.04045 ? ch / 12.92 : ((ch + .055) / 1.055) ** 2.4);
          return channels[0] * .2126 + channels[1] * .7152 + channels[2] * .0722;
        }
        const a = luminance(getComputedStyle(sample).color), b = luminance(bg);
        return (Math.max(a,b)+.05) / (Math.min(a,b)+.05);
      });
      assert.ok(contrast >= 4.5); ratios.set(colorScheme, contrast.toFixed(1));
      await snapshot("pending");
      await allow.click();
      await sheet.getByText(label("已允许这次操作。", "This action was allowed once."), { exact: true }).waitFor();
      assert.equal(posts, 1);
      status = width === 390 ? (zh ? "offline" : "revoked") : (zh ? "expired" : "revoked");
      await page.reload({waitUntil: "domcontentloaded"});
      const statusText = { offline: label("无法连接 Jeeves。连上后再查看这条审批。", "Jeeves is unreachable. Reconnect to review this request."),
        revoked: label("此通知属于另一台设备，或设备授权已撤销。", "This notice belongs to another device, or its grant was revoked."),
        expired: label("这条审批已过期或不再待处理。", "This request has expired or is no longer pending.") };
      await sheet.getByText(statusText[status], {exact:true}).waitFor();
      assert.equal(await allow.count(), 0);
      await snapshot(status);
      if (status === "offline") {
        status = "expired";
        await sheet.getByRole("button", {name:label("重新核对", "Check again")}).click();
        await sheet.getByText(statusText.expired, {exact:true}).waitFor();
      }
      await page.keyboard.press("Escape");
      await sheet.waitFor({state: "detached"});
      await page.waitForURL(url => !url.searchParams.has("approvalRequestId"));
      const composer = page.locator("#composer");
      await composer.locator(".workspace-picker button").first().waitFor({state:"visible"});
      assert.equal(await composer.locator(".model-picker, .permissions").count(), 0);
      if (!await page.locator("#settings-entry button").isVisible()) await page.locator(".mobile-menu").click();
      assert.ok(await page.locator('a[href="/account/"]').isVisible());
      assert.ok(await page.locator('a[href="/account/#devices"]').isVisible());
      await page.locator("#settings-entry button").click();
      const push = page.getByRole("region", {name:label("审批通知", "Approval notifications")});
      await push.getByRole("button", {name:label("显示本机配对公钥", "Show device pairing key")}).scrollIntoViewIfNeeded();
      await push.getByRole("button", {name:label("显示本机配对公钥", "Show device pairing key")}).click();
      const key = push.getByRole("textbox", {name:label("本机配对公钥","Device pairing public key")});
      publicKey = await key.inputValue();
      assert.equal(Buffer.from(publicKey, "base64").length, 32);
      const check = push.getByRole("button", {name:label("核对配对状态","Check device pairing")});
      await check.click();
      await push.getByText(label("请先在可信驾驶舱核对此公钥，为此设备授予审批权限。","Verify the public key in a trusted cockpit and grant this device approval access.")).waitFor();
      paired = true;
      await check.click();
      await push.getByText(label("此 Jeeves 服务尚未配置 Web Push。","Web Push has not been configured for this Runtime.")).waitFor();
      configured = true;
      await check.click();
      const enable = push.getByRole("button", {name:label("在本机启用审批通知", "Enable notifications on this device")});
      await enable.scrollIntoViewIfNeeded();
      assert.equal(await page.evaluate(() => window.fixtureSubscriptions), 0);
      await snapshot("settings");
      await enable.click();
      await push.getByText(label("已订阅这台设备的审批通知。","This device is subscribed to approval notifications.")).waitFor();
      assert.equal(await page.evaluate(() => window.fixtureSubscriptions), 1);
      assert.equal(subscriptions, 1);
      await push.locator("summary").click();
      const input = push.getByRole("textbox",{name:label("待配对设备的公钥","Public key of the device to pair")});
      const grant = push.getByRole("button",{name:label("授予审批权限","Grant approval access"), exact:true});
      await input.fill(mixed.deviceId);
      assert.ok(await grant.isDisabled());
      const mixedRow=push.getByRole("listitem").filter({hasText:mixed.deviceId});
      assert.equal(await mixedRow.getByRole("button").count(),0);
      await input.fill(extraKey);
      await grant.click();
      const row=push.getByRole("listitem").filter({hasText:extraKey});
      await row.getByRole("button",{name:label("撤销审批权限","Revoke approval access")}).click();
      assert.equal(revokes,0);
      await row.getByRole("button",{name:label("确认撤销","Confirm revocation")}).scrollIntoViewIfNeeded();
      await snapshot("pairing");
      await row.getByRole("button",{name:label("确认撤销","Confirm revocation")}).click();
      await row.waitFor({state:"detached"});
      assert.equal(pairs,1); assert.equal(revokes,1);
      assert.equal(posts, 1);
      if (width === 390 && zh) {
        status = "pending";
        locator.approvalRequestId = request.requestId = "fixture-mail";
        request.tool = "email_send";
        request.argsPreview = "To: fixture@example.invalid, Subject: travel";
        await page.goto(workspace + "?" + new URLSearchParams(locator), {waitUntil:"domcontentloaded"});
        await sheet.getByRole("button", { name: "在主聊天跟进" }).click();
        await page.locator("#composer-target").getByText(/Mail fixture travel/).waitFor();
        await page.locator("#input").fill("刚才那个先别发，改成草稿");
        await page.locator("#input").press("Enter");
        await page.locator("#messages").getByText(/Saved draft.*travel fixture body/s).last().waitFor();
        assert.equal(posts, 1);
        await page.setViewportSize({ width: 1280, height: 900 });
        await page.locator("#mode-cowork").click();
        await page.locator(`[data-cowork-task="${mailTasks[0]}"]`).click();
        await page.locator(".work-history summary").click();
        await page.locator("#cowork-view").getByText(/Saved draft.*travel fixture body/s).last().waitFor();
        console.log("Mail follow-up reached the original Cowork transcript");
      }
      assert.deepEqual(errors, []);
    } finally { await ctx.close(); }
  }
  await writeFile(path.join(out, "index.html"), `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>M4-05 审批通知</title><style>body{max-width:960px;margin:auto;padding:24px;font:16px/1.6 system-ui}figure{margin:24px 0}img{max-width:100%;height:auto}</style><h1>M4-05 审批通知</h1><p>回环夹具 · Chromium · 390 与 1280 像素。审批卡正文相对卡片背景对比度：亮色 ${ratios.get("light")}∶1、暗色 ${ratios.get("dark")}∶1。测试打开通知、核对范围、拒绝过期或已撤销的通知、离线重读及设置入口；未请求系统推送权限。</p>` + pictures.map(file => `<figure><figcaption>${file}</figcaption><a href="${file}"><img loading="lazy" width="700" src="${file}" alt="${file}"></a></figure>`).join(""));
  console.log("Approval notice fixture passed:", pictures.length, "screenshots in", out);
}
