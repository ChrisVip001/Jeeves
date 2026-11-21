import { mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";
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
      await nav.getByRole("button", { name: "我的事项" }).click();
      const row = page.locator(`[data-cowork-task="${taskId}"]`);
      await row.waitFor();
      assert.equal(await page.locator("#input").isVisible(), false);
      await row.click();
      const work = page.locator("#cowork-view");
      await work.getByRole("heading", {name:"task-alpha-fixture", level:2}).waitFor();
      await work.getByText("已生成，待验收", {exact:true}).waitFor();
      const taskComposer = work.locator("#cowork-input");
      await taskComposer.fill("unsent task draft");
      await page.reload();
      await taskComposer.waitFor();
      assert.equal(await taskComposer.inputValue(), "unsent task draft");
      if (!decisionId) {
        const file = work.locator(".work-file").filter({hasText:"report.txt"});
        await file.locator("summary").click();
        assert.equal(await file.locator("pre").textContent(), "task-alpha-fixture");
        if (process.env.JEEVES_TEST_SCREENSHOTS) {
          const dir = process.env.JEEVES_TEST_SCREENSHOTS;
          await mkdir(dir, {recursive:true});
          await page.screenshot({path:join(dir, width === 390 ? "phone.png" : "desktop.png")});
        }
        const downloading = page.waitForEvent("download");
        await file.getByRole("button", {name:"保存文本副本"}).click();
        const download = await downloading;
        assert.equal(download.suggestedFilename(), "report.txt");
        assert.equal(await readFile(await download.path(), "utf8"), "task-alpha-fixture");
      }
      await nav.getByRole("button", {name:"我的事项"}).click();
      const search = page.getByRole("searchbox", {name:"搜索事项"});
      await search.fill("no-match-001");
      await page.getByText("没有找到匹配的事项。").waitFor();
      await search.fill("alpha");
      await row.waitFor();
      assert.equal(await page.locator(".activity-task").count(), 1);
      await page.goBack();
      await taskComposer.waitFor();
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
        if (!await decision.locator("pre").first().isVisible()) await decision.locator("summary").click();
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
      await card.locator("strong").filter({hasText:"task-alpha-fixture · 已生成，待验收"}).waitFor();
      await card.getByRole("button", { name: "继续这件事" }).click();
      await work.getByRole("heading",{name:"task-alpha-fixture", level:2}).waitFor();
      assert.equal(await taskComposer.inputValue(), "unsent task draft");
      assert.equal(await page.getByRole("dialog").count(), 0);
      if (width === 390) {
        await taskComposer.fill("task-follow-fixture");
        const sent = page.waitForResponse(r => new URL(r.url()).pathname === "/api/interaction" && r.request().method() === "POST");
        await work.locator("#cowork-send").click();
        const response = await sent;
        assert.equal(response.request().postDataJSON().taskId, taskId);
        assert.ok(response.ok());
        assert.equal(await response.finished(), null);
      }
    } finally { await context.close(); }
  }
} finally { await browser.close(); }
