import assert from "node:assert/strict";
import { chromium, devices } from "../node_modules/playwright/index.mjs";

const [channel, workspace, cookie, task] = process.argv.slice(2);
const browser = await chromium.launch({channel});
try {
  const context = await browser.newContext({...devices["iPhone 13"], serviceWorkers:"block"});
  const [name, value] = cookie.split("=");
  await context.addCookies([{name, value, url:new URL(workspace).origin}]);
  await context.addInitScript(() => {
    localStorage.setItem("jeeves.setup.dismissed", "1");
    localStorage.setItem("jeeves.lang", "zh");
  });
  const page = await context.newPage();
  await page.goto(workspace);
  await page.getByRole("button", {name:"工作", exact:true}).first().click();
  await page.locator(`[data-cowork-task="${task}"]`).click();
  const follow = page.getByRole("button", {name:"在主聊天跟进", exact:true});
  await follow.waitFor({state:"visible"});
  assert.equal(await page.locator("#cowork-input, #cowork-send").count(), 0);
  await follow.click();
  const message = "改成低于1700才提醒，其他照旧。";
  await page.locator("#input").fill(message);
  const [response] = await Promise.all([
    page.waitForResponse(r=>r.url().endsWith("/api/interaction") && r.request().method()==="POST"),
    page.locator("#send").click(),
  ]);
  assert.equal(response.request().postDataJSON().taskId, task);
  assert.equal(response.request().postDataJSON().message, message);
  assert.ok((await response.body()).includes("1700.00"));
} finally {
  await browser.close();
}
