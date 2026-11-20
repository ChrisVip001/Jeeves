import assert from "node:assert/strict";
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import vue from "@vitejs/plugin-vue";
import { createServer } from "vite";

const executablePath = process.env.JEEVES_UI_CHROMIUM || chromium.executablePath();
const installed = existsSync(executablePath);
const screenshotDir = process.env.JEEVES_UI_SCREENSHOT_DIR;
test("connection cards render all wire states and no secret, with bilingual keyboard actions", {
  skip: !installed ? "Optional Chromium not installed" : false,
}, async () => {
  const root = fileURLToPath(new URL("../", import.meta.url));
  const server = await createServer({ root, configFile: false, plugins: [vue()], server: { host: "127.0.0.1", port: 0, hmr: false }, optimizeDeps: { noDiscovery: true, include: ["vue", "lucide-vue-next"] } });
  await server.listen();
  const address = server.httpServer?.address();
  assert.ok(address && typeof address !== "string");
  try {
    const browser = await chromium.launch({ executablePath });
    try {
      if (screenshotDir) mkdirSync(screenshotDir, { recursive: true });
    for (const lang of ["zh", "en"] as const) {
      const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: lang === "zh" ? "zh-CN" : "en-US", colorScheme: "dark", reducedMotion: "reduce" });
      await page.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/connections.html?lang=${lang}`);
      const cards = page.locator(".capability-card");
      await cards.first().waitFor({ timeout: 5000 });
      assert.equal(await cards.count(), 9);
      assert.ok(!(await page.locator("body").innerText()).includes("FAKE-SECRET-DO-NOT-RENDER"));
      assert.ok(!(await page.content()).includes("FAKE-SECRET-DO-NOT-RENDER"));
      assert.match(await page.locator("body").innerText(), /FIXTURE_API_KEY/);
      assert.ok(await cards.filter({ hasText: "Email" }).getByText(lang === "zh" ? "凭据库（位置未确认）" : "Credential vault (location unconfirmed)", { exact: true }).count());
      assert.ok(await cards.filter({ hasText: "Feishu / Lark" }).getByText(lang === "zh" ? "运行时内存（临时）" : "Runtime memory (temporary)", { exact: true }).count());
      const states = lang === "zh" ? ["可用", "未授权", "已过期", "权限不足", "策略限制", "无法连接", "未知"] : ["Ready", "Unauthorized", "Expired", "Missing scope", "Blocked by policy", "Unreachable", "Unknown"];
      for (const state of states) assert.ok(await cards.getByText(state, { exact: true }).count(), state);
      for (const card of await cards.all()) assert.equal(await card.locator(".capability-footer button").count(), 1);
      for (const button of await cards.getByRole("button").all()) assert.ok((await button.boundingBox())!.height >= 44);
      const expired = cards.filter({ hasText: "Calendar MCP" });
      await expired.locator(".capability-footer button").focus();
      await page.keyboard.press("Enter");
      const oauth = expired.getByRole("link");
      await oauth.waitFor();
      assert.equal(await oauth.getAttribute("href"), "https://auth.example.test/fixture?state=fixture");
      assert.equal(await oauth.getAttribute("target"), "_blank");
      assert.match((await expired.locator(".capability-instruction").innerText()), /Jeeves/);
      await page.locator(".connections-page > .connections-summary").getByRole("button").click();
      await oauth.waitFor({ state: "detached" });
      assert.equal(await expired.locator(".capability-instruction").count(), 0, "refresh must not show a completed authorization as still preparing");
      const missing = cards.filter({ hasText: "Mail MCP" });
      await missing.locator(".capability-footer button").focus();
      await page.keyboard.press("Enter");
      await missing.getByRole("alert").waitFor();
      assert.match(await missing.locator(".capability-instruction").innerText(), /jeeves mcp login/);
      assert.equal(await missing.getByRole("link").count(), 0);
      assert.ok(!(await page.content()).includes("FAKE-SECRET-DO-NOT-RENDER"));
      assert.deepEqual(await page.evaluate(() => (window as unknown as { fixtureAuth: string[] }).fixtureAuth), ["mcp:calendar", "mcp:mail"]);
      const blocked = cards.filter({ hasText: "Feishu sender" });
      await blocked.getByRole("button").focus();
      await page.keyboard.press("Enter");
      assert.equal(await blocked.locator(".capability-instruction").count(), 1);
      const policy = await blocked.locator(".capability-instruction").innerText();
      assert.match(policy, /connectors\.json/);
      assert.match(policy, lang === "zh" ? /重启/ : /restart/i);
      assert.equal(await page.locator(".connections-page").count(), 1, "connector allowlist is not a global permission control");
      await page.keyboard.press("Space");
      const retry = cards.filter({ hasText: "Feishu / Lark" }).getByRole("button");
      await retry.focus();
      await page.keyboard.press("Enter");
      await cards.filter({ hasText: "Feishu / Lark" }).getByText(states[0], { exact: true }).waitFor();

      const connectorRows = page.locator(".local-connector-row");
      await connectorRows.filter({ hasText: lang === "zh" ? "日历" : "Calendar" }).getByRole("button").click();
      const calendar = page.locator("form[data-connector=calendar]");
      await calendar.waitFor();
      await calendar.locator("input[name=url]").fill("http://calendar.example.test/fixture");
      assert.equal(await calendar.locator("button[type=submit]").isDisabled(), true, "calendar URL must use HTTPS");
      await calendar.locator("input[name=url]").fill("https://calendar.example.test/fixture?key=fixture");
      assert.equal(await calendar.locator("button[type=submit]").isDisabled(), true, "calendar URL must not carry a query");
      await calendar.locator("input[name=url]").fill("https://calendar.example.test/fixture");
      assert.equal(await page.locator(".local-connectors datalist option[value='vault:CALENDAR_FIXTURE']").count(), 1);
      await calendar.locator("input[name=timezone]").fill("Bad/Zone");
      await calendar.locator("button[type=submit]").click();
      await calendar.getByRole("alert").waitFor();
      assert.ok(!(await page.content()).includes("FAKE-SECRET-DO-NOT-RENDER"));
      await calendar.locator("input[name=timezone]").fill("Asia/Shanghai");
      await calendar.locator("button[type=submit]").click();
      await calendar.waitFor({ state: "detached" });

      await connectorRows.filter({ hasText: lang === "zh" ? "邮件" : "Email" }).getByRole("button").click();
      const email = page.locator("form[data-connector=email]");
      await email.waitFor();
      assert.equal(await email.locator("h4").evaluate(el => el === document.activeElement), true, "focus moves to the editor heading");
      assert.equal(await email.locator("input[name=credentialSource]").inputValue(), "vault:MAIL_FIXTURE");
      assert.equal(await email.locator("select[name=imapSecurity]").inputValue(), "plain");
      assert.equal(await email.locator("button[type=submit]").isDisabled(), true, "legacy plaintext security requires correction");
      await email.locator("select[name=imapSecurity]").selectOption("tls");
      await email.locator("input[name=imapPort]").fill("993");
      assert.equal(await email.locator("input[name=live]").isChecked(), false);
      assert.equal(await email.locator("input[name=live]").isDisabled(), true, "live requires explicit recipients");
      await email.locator("textarea[name=allowRecipients]").fill("recipient@example.test");
      await email.locator("input[name=live]").check();
      await email.locator("button[type=submit]").click();
      await email.waitFor({ state: "detached" });
      const writes = await page.evaluate(() => (window as unknown as { fixtureWrites: unknown[] }).fixtureWrites);
      assert.deepEqual(writes, [
        { calendar: { url: "https://calendar.example.test/fixture", username: null, credentialSource: null, timezone: "Bad/Zone" }, email: null },
        { calendar: { url: "https://calendar.example.test/fixture", username: null, credentialSource: null, timezone: "Asia/Shanghai" }, email: null },
        { calendar: null, email: { username: "reader@example.test", credentialSource: "vault:MAIL_FIXTURE", from: "reader@example.test", imap: { host: "imap.example.test", port: 993, security: "tls" }, smtp: { host: "smtp.example.test", port: 465, security: "tls" }, allowRecipients: ["recipient@example.test"], live: true } },
      ]);
      const catalog = page.locator(".connector-catalog");
      await catalog.locator("summary").focus();
      await page.keyboard.press("Enter");
      assert.equal(await catalog.locator(".catalog-row").count(), 3);
      assert.equal(await catalog.locator(".catalog-row").filter({ hasText: "Provider example" }).getByRole("button").count(), 0);
      assert.equal(await catalog.locator(".catalog-row").filter({ hasText: "Built-in example" }).getByRole("button").count(), 0);
      await catalog.locator(".catalog-row").filter({ hasText: "Catalog example" }).getByRole("button").click();
      const form = page.locator("form[data-connector=mcp]");
      await form.waitFor();
      assert.equal(await form.locator("h4").evaluate(el => el === document.activeElement), true);
      assert.equal(await form.locator("input[name=url]").inputValue(), "https://mcp.example.test/fixture");
      assert.equal(await form.locator("input[name=url]").getAttribute("readonly"), "");
      await form.locator("input[name=authEnv]").fill("bad-secret");
      assert.equal(await form.locator("button[type=submit]").isDisabled(), true);
      await form.locator("input[name=authEnv]").fill("FIXTURE_TOKEN");
      await form.locator("button[type=submit]").click();
      await form.waitFor({ state: "detached" });
      await cards.filter({ hasText: "catalog-fixture" }).waitFor();
      await page.locator("[data-add-mcp]").click();
      await form.waitFor();
      await form.locator("input[name=name]").fill("calendar");
      await form.locator("input[name=url]").fill("https://custom.example.test/mcp");
      assert.equal(await form.locator("button[type=submit]").isDisabled(), true, "existing name cannot be replaced by add");
      await form.locator("input[name=name]").fill("fail");
      await form.locator("input[name=url]").fill("https://custom.example.test/mcp?credential=test");
      assert.equal(await form.locator("button[type=submit]").isDisabled(), true);
      await form.locator("input[name=url]").fill("https://custom.example.test/mcp");
      await form.locator("select[name=auth]").selectOption("oauth");
      await form.locator("button[type=submit]").click();
      await form.getByRole("alert").waitFor();
      await form.locator("input[name=name]").fill("custom-oauth");
      await form.locator("button[type=submit]").click();
      await form.waitFor({ state: "detached" });
      await page.locator("[data-add-mcp]").click();
      await form.locator("input[name=name]").fill("stdio-fixture");
      await form.locator("select[name=transport]").selectOption("stdio");
      assert.equal(await form.locator("select[name=auth]").count(), 0);
      await form.locator("input[name=command]").fill("fixture-command");
      await form.locator("textarea[name=args]").fill("--label\narg with spaces");
      await form.locator("button[type=submit]").click();
      await form.waitFor({ state: "detached" });
      assert.deepEqual(await page.evaluate(() => (window as unknown as { fixtureMcp: unknown[] }).fixtureMcp), [
        { name: "catalog-fixture", createOnly: true, url: "https://mcp.example.test/fixture", auth_env: "FIXTURE_TOKEN", manifest: { category: "notes", tools: { read_note: { effect: "read", disabled: false } }, verbs: {} } },
        { name: "fail", createOnly: true, url: "https://custom.example.test/mcp", oauth: true },
        { name: "custom-oauth", createOnly: true, url: "https://custom.example.test/mcp", oauth: true },
        { name: "stdio-fixture", createOnly: true, command: "fixture-command", args: ["--label", "arg with spaces"] },
      ]);
      const toolPanel = expired.locator(".tool-policy");
      await toolPanel.locator("summary").focus();
      await page.keyboard.press("Enter");
      const read = toolPanel.getByRole("checkbox").first();
      await read.focus();
      await page.keyboard.press("Space");
      await page.waitForFunction(() => (window as unknown as { fixtureToolChanges: unknown[] }).fixtureToolChanges.length === 1);
      assert.equal(await read.isChecked(), false);
      const send = toolPanel.getByRole("checkbox").last();
      await send.click();
      await toolPanel.getByRole("alert").waitFor();
      assert.equal(await send.isChecked(), false, "failed change rolls back to the runtime state");
      const pack = missing.locator(".tool-policy");
      await pack.locator("summary").click();
      await pack.getByText("read_mail", { exact: true }).waitFor();
      assert.equal(await pack.getByRole("checkbox").count(), 0);
      assert.ok(!(await page.content()).includes("FAKE-SECRET-DO-NOT-RENDER"));
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
      assert.equal(await page.locator(".preferences-page").evaluate(el => el.scrollWidth > el.clientWidth), false);
      assert.equal(await retry.evaluate(el => getComputedStyle(el).transitionDuration), "0s");
      for (const colorScheme of ["dark", "light"] as const) {
        await page.emulateMedia({ colorScheme, reducedMotion: "reduce" });
        const ratios = await cards.first().evaluate(el => {
        const color = (value: string) => value.match(/[\d.]+/g)!.slice(0, 3).map(Number);
        const luminance = (value: string) => color(value).reduce((sum, component, i) => {
          const c = component / 255;
          return sum + [0.2126, 0.7152, 0.0722][i]! * (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
        }, 0);
        const bg = luminance(getComputedStyle(el).backgroundColor);
        return ["h4", ".capability-state", ".capability-explanation", ".capability-meta"].map(selector => {
          const fg = luminance(getComputedStyle(el.querySelector(selector)!).color);
          return (Math.max(bg, fg) + 0.05) / (Math.min(bg, fg) + 0.05);
        });
        });
        assert.ok(ratios.every(r => r >= 4.5), String(ratios));
        console.log(`${lang} ${colorScheme} connection card contrast (title, state, body, metadata): ${ratios.map(r => r.toFixed(2)).join(", ")}`);
      }
      await page.close();
    }
    if (screenshotDir) {
      for (const colorScheme of ["light", "dark"] as const) {
        for (const [size, viewport] of [["desktop", { width: 1280, height: 900 }], ["narrow", { width: 390, height: 844 }]] as const) {
          const page = await browser.newPage({ viewport, locale: "zh-CN", colorScheme, reducedMotion: "reduce" });
          await page.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/connections.html?lang=zh`);
          await page.locator(".capability-card").first().waitFor();
          await page.screenshot({ path: join(screenshotDir, `m2-09-${colorScheme}-${size}.png`) });
          await page.locator(".capability-card").last().scrollIntoViewIfNeeded();
          await page.screenshot({ path: join(screenshotDir, `m2-09-${colorScheme}-${size}-lower.png`) });
          await page.locator(".local-connector-row").filter({ hasText: "日历" }).getByRole("button").click();
          await page.locator("form[data-connector=calendar]").scrollIntoViewIfNeeded();
          await page.screenshot({ path: join(screenshotDir, `m2-09-${colorScheme}-${size}-calendar.png`) });
          await page.locator("form[data-connector=calendar]").getByRole("button", { name: "取消" }).click();
          await page.locator(".local-connector-row").filter({ hasText: "邮件" }).getByRole("button").click();
          await page.locator("form[data-connector=email]").scrollIntoViewIfNeeded();
          await page.locator("form[data-connector=email] h4").scrollIntoViewIfNeeded();
          await page.screenshot({ path: join(screenshotDir, `m2-09-${colorScheme}-${size}-email.png`) });
          await page.locator("form[data-connector=email] button[type=submit]").scrollIntoViewIfNeeded();
          await page.screenshot({ path: join(screenshotDir, `m2-09-${colorScheme}-${size}-email-lower.png`) });
          await page.locator("form[data-connector=email]").getByRole("button", { name: "取消" }).click();
          const catalog = page.locator(".connector-catalog");
          await catalog.locator("summary").click();
          await catalog.locator(".catalog-row").last().waitFor();
          await catalog.scrollIntoViewIfNeeded();
          await page.screenshot({ path: join(screenshotDir, `m3-04-${colorScheme}-${size}-catalog.png`) });
          await catalog.locator(".catalog-row").filter({ hasText: "Catalog example" }).getByRole("button").click();
          const mcpForm = page.locator("form[data-connector=mcp]");
          await mcpForm.scrollIntoViewIfNeeded();
          await page.screenshot({ path: join(screenshotDir, `m3-04-${colorScheme}-${size}-form.png`) });
          await mcpForm.getByRole("button", { name: "取消" }).click();
          await catalog.locator("summary").click();
          const policy = page.locator(".capability-card").filter({ hasText: "Calendar MCP" }).locator(".tool-policy");
          await policy.locator("summary").click();
          await policy.getByRole("checkbox").first().waitFor();
          await policy.scrollIntoViewIfNeeded();
          await page.screenshot({ path: join(screenshotDir, `m3-04-${colorScheme}-${size}-tools.png`) });
          await page.close();
        }
      }
    }
    const failed = await browser.newPage();
    await failed.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/connections.html?failSettings`);
    await failed.locator(".local-connectors").getByRole("alert").waitFor();
    assert.equal(await failed.locator(".local-connector-row").count(), 0);
    assert.ok(!(await failed.content()).includes("FAKE-SECRET-DO-NOT-RENDER"));
    await failed.close();
    const inbox = await browser.newPage();
    await inbox.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/connections.html?lang=en&failInbox`);
    await inbox.getByRole("button", { name: "Inbox", exact: true }).click();
    await inbox.getByRole("alert").filter({ hasText: "Outbox read failed" }).waitFor();
    assert.equal(await inbox.getByText("Nothing is waiting.").count(), 0);
    await inbox.getByRole("button", { name: "Refresh", exact: true }).click();
    await inbox.getByText("Nothing is waiting.").waitFor();
    assert.equal(await inbox.getByText("Outbox read failed", { exact: false }).count(), 0);
    await inbox.close();
    const legacy = await browser.newPage();
    await legacy.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/connections.html?legacyPointer`);
    await legacy.locator(".local-connector-row").first().getByRole("button").click();
    const legacyCalendar = legacy.locator("form[data-connector=calendar]");
    await legacyCalendar.getByText("旧凭据来源未显示，请重新填写受支持的凭据引用。").waitFor();
    assert.equal(await legacyCalendar.locator("input[name=credentialSource]").inputValue(), "");
    assert.equal(await legacyCalendar.locator("button[type=submit]").isDisabled(), true);
    await legacy.locator(".local-connector-row").last().getByRole("button").click();
    const legacyEmail = legacy.locator("form[data-connector=email]");
    await legacyEmail.getByText("旧凭据来源未显示，请重新填写受支持的凭据引用。").waitFor();
    assert.equal(await legacyEmail.locator("input[name=credentialSource]").inputValue(), "");
    await legacy.close();
    const badOAuth = await browser.newPage();
    await badOAuth.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/connections.html?badOAuth`);
    const badCard = badOAuth.locator(".capability-card").filter({ hasText: "Calendar MCP" });
    await badCard.locator(".capability-footer button").click();
    await badCard.getByRole("alert").waitFor();
    assert.equal(await badCard.getByRole("link").count(), 0);
    assert.ok(!(await badOAuth.content()).includes("FAKE-SECRET-DO-NOT-RENDER"));
    await badOAuth.close();
    } finally {
      await browser.close();
    }
  } finally {
    await server.close();
  }
});
