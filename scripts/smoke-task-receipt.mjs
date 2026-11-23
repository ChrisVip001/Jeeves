import { mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import assert from "node:assert/strict";
import { chromium } from "../node_modules/playwright/index.mjs";

const [workspace, cookie, taskId, channel, decisionId] = process.argv.slice(2);
const [name, value] = cookie.split("=");
const browser = await chromium.launch({headless: process.env.PW_HEADED !== "1",  channel });
try {
  for (const width of [1280, 390]) {
    const context = await browser.newContext({ viewport: { width, height: 850 }, serviceWorkers: "block" });
    try {
      await context.addCookies([{ name, value, url: new URL(workspace).origin }]);
      const history = await context.request.get(workspace + "api/session?sessionId=jeeves-main");
      const receipt = (await history.json()).data.find(m => m.source === "task_completion" && m.taskId === taskId);
      assert.equal(receipt?.receiptStatus, "produced_unverified");
      await context.addInitScript(() => {
        localStorage.setItem("jeeves.setup.dismissed", "1");
        localStorage.setItem("jeeves.lang", "zh");
      });
      const page = await context.newPage();
      await page.goto(workspace);
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
      await work.locator(".cowork-detail").getByText("已生成，待验收", {exact:true}).waitFor();
      const taskComposer = work.locator("#cowork-input");
      await taskComposer.fill("unsent task draft");
      await page.reload();
      await taskComposer.waitFor();
      assert.equal(await taskComposer.inputValue(), "unsent task draft");
      if (!decisionId) {
        const file = work.locator(".work-file").filter({hasText:"report.txt"});
        await file.click();
        const review = work.getByRole("complementary", {name: /report.txt/});
        assert.equal(await review.locator("pre").textContent(), "task-alpha-fixture");
        if (process.env.JEEVES_TEST_SCREENSHOTS) {
          const dir = process.env.JEEVES_TEST_SCREENSHOTS;
          await mkdir(dir, {recursive:true});
          await page.screenshot({path:join(dir, width === 390 ? "phone.png" : "desktop.png")});
        }
        const downloading = page.waitForEvent("download");
        await review.getByRole("button", {name:"保存文本副本"}).click();
        const download = await downloading;
        assert.equal(download.suggestedFilename(), "report.txt");
        assert.equal(await readFile(await download.path(), "utf8"), "task-alpha-fixture");
        await review.getByRole("button", {name:"关闭文件审阅"}).click();
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
      if (!decisionId && width === 1280) {
        const meta = "**/api/session/meta?sessionId=jeeves-main";
        await page.route(meta, async route => {
          const response = await route.fetch();
          const body = await response.json();
          body.session.cwd = "/example/project-r622";
          await route.fulfill({response, json:body});
        });
        await page.reload();
        await page.getByRole("button", {name:"project-r622"}).waitFor({timeout:5000});
        await page.unroute(meta);
        await page.reload();
      }
      const folder = page.locator("#composer .workspace-picker").getByRole("button", { name: "自动工作区", exact: true });
      await folder.click();
      await page.getByRole("dialog", { name: "当前工作目录" }).getByRole("button", { name: "选择已有项目" }).waitFor();
      await folder.press("Escape");
      assert.equal(await page.locator("#composer .model-picker, #composer .permissions").count(), 0);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      if (decisionId) {
        const decision = page.locator(".chat-decisions");
        if (!await decision.locator("pre").first().isVisible()) await decision.locator("summary").click();
        await decision.locator("pre").filter({ hasText: "report.txt" }).waitFor();
        if (width === 390) {
          const sent = page.waitForResponse(r => r.url() === workspace + "api/approval" && r.request().method() === "POST");
          await decision.getByRole("button", { name: "允许一次" }).click();
          const response = await sent;
          const payload = response.request().postDataJSON();
          assert.equal(payload.requestId, decisionId);
          assert.equal(payload.remember, "once");
          assert.ok(payload.version >= 1);
          assert.equal((await response.json()).accepted, true);
          const receiptCard = page.locator(".task_completion").filter({has: page.locator(".receipt-decisions").getByText(decisionId, {exact:true})});
          await receiptCard.waitFor();
          const actual = await receiptCard.locator(".receipt-decisions li").allTextContents();
          assert.deepEqual(actual, [`已允许 · write_file · ${decisionId}`]);
          assert.equal(await receiptCard.locator("details").evaluate(el => el.open), false);
          await receiptCard.getByRole("button", {name:"继续这件事"}).click();
          await work.locator(".receipt-decisions").getByText(decisionId, {exact:true}).waitFor();
          assert.deepEqual(await work.locator(".receipt-decisions li").allTextContents(), actual);
        }
        continue;
      }
      const card = page.locator(`[data-notice-id="${receipt.id}"]`);
      await card.locator("strong").filter({hasText:"task-alpha-fixture · 已生成，待验收"}).waitFor();
      await card.getByRole("button", { name: "继续这件事" }).click();
      await work.getByRole("heading",{name:"task-alpha-fixture", level:2}).waitFor();
      assert.equal(await taskComposer.inputValue(), "unsent task draft");
      assert.equal(await page.getByRole("dialog").count(), 0);
      if (width === 1280) {
        await page.route("**/api/tasks", async route => {
          const response = await route.fetch();
          const body = await response.json();
          const target = body.tasks.find(task => task.id === taskId);
          target.status = "needs_attention";
          await route.fulfill({response, json: body});
        });
        await page.reload();
        await work.getByRole("button", {name:"核对操作"}).click({timeout:5000});
        const review = page.getByRole("dialog", {name:"核对操作"});
        await review.getByText("task-alpha-fixture", {exact:true}).waitFor();
        await review.getByRole("button", {name:"关闭"}).click();
        await work.getByRole("button", {name:"继续任务"}).waitFor();
      }
      if (width === 390) {
        await taskComposer.fill("task-follow-fixture");
        const sent = page.waitForResponse(r => r.url() === workspace + "api/interaction" && r.request().method() === "POST");
        await work.locator("#cowork-send").click();
        const response = await sent;
        assert.equal(response.request().postDataJSON().taskId, taskId);
        assert.ok(response.ok());
        assert.equal(await response.finished(), null);
      }
    } finally { await context.close(); }
  }
} finally { await browser.close(); }
