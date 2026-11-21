import assert from "node:assert/strict";
import { chromium } from "../jeeves-ts/node_modules/playwright/index.mjs";

const [origin, taskId, channel, decisionId] = process.argv.slice(2);
const headers = { Authorization: "Bearer web-fixture-token" };
const browser = await chromium.launch({ channel });
try {
  for (const width of [1280, 390]) {
    const context = await browser.newContext({ viewport: { width, height: 850 }, serviceWorkers: "block" });
    try {
      const history = await context.request.get(origin + "/api/session?sessionId=jeeves-main", { headers });
      const receipt = (await history.json()).data.find(m => m.source === "task_completion" && m.taskId === taskId);
      assert.equal(receipt?.receiptStatus, "produced_unverified");
      await context.addInitScript(() => {
        localStorage.setItem("jeeves-ops-token", "web-fixture-token");
        localStorage.setItem("jeeves.setup.dismissed", "1");
        localStorage.setItem("jeeves.lang", "zh");
      });
      const page = await context.newPage();
      await page.goto(origin);
      const nav = page.getByRole("navigation", { name: "主导航" });
      const composer = page.locator("#input");
      await composer.fill("unsent fixture draft");
      await nav.getByRole("button", { name: "动态" }).click();
      const row = page.locator(`[data-cowork-task="${taskId}"]`);
      await row.waitFor();
      assert.equal(await page.locator("#input").isVisible(), false);
      await row.click();
      if (width === 390) {
        const detail = page.getByRole("dialog", { name: "task-alpha-fixture", exact: true });
        await detail.getByRole("button", { name: "关闭", exact: true }).click();
      } else await page.locator("#cowork-view h2").getByText("task-alpha-fixture").waitFor();
      await nav.getByRole("button", { name: "对话", exact: true }).click();
      assert.equal(await composer.inputValue(), "unsent fixture draft");
      await composer.fill("");
      assert.equal(await page.locator("#composer-model").isVisible(), false);
      await page.locator(".compose-options summary").click();
      await page.locator("#composer-model").waitFor();
      await page.locator(".compose-options summary").click();
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      if (decisionId) {
        const decision = page.locator(".chat-decisions");
        await decision.locator("summary").click();
        await decision.locator("pre").filter({ hasText: "report.txt" }).waitFor();
        if (width === 390) {
          const sent = page.waitForResponse(r => new URL(r.url()).pathname === "/api/approval" && r.request().method() === "POST");
          await decision.getByRole("button", { name: "允许一次" }).click();
          const response = await sent;
          const payload = response.request().postDataJSON();
          assert.equal(payload.requestId, decisionId);
          assert.equal(payload.remember, "once");
          assert.ok(payload.version >= 1);
          assert.equal((await response.json()).accepted, true);
        }
        continue;
      }
      const card = page.locator(`[data-notice-id="${receipt.id}"]`);
      await card.getByText("已生成，待验收", { exact: true }).waitFor();
      await card.getByRole("button", { name: "继续这件事" }).click();
      await page.locator(".task-following").filter({ hasText: "task-alpha-fixture" }).waitFor();
      assert.equal(await page.getByRole("dialog").count(), 0);
      if (width === 390) {
        await page.locator("#input").fill("task-follow-fixture");
        const sent = page.waitForResponse(r => new URL(r.url()).pathname === "/api/interaction" && r.request().method() === "POST");
        await page.locator("#send").click();
        const response = await sent;
        assert.equal(response.request().postDataJSON().taskId, taskId);
        assert.ok(response.ok());
        assert.equal(await response.finished(), null);
      }
    } finally { await context.close(); }
  }
} finally { await browser.close(); }
