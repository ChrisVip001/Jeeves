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
  await page.getByRole("button", {name:"我的事项", exact:true}).first().click();
  await page.locator(`[data-cowork-task="${task}"]`).click();
  await page.locator("#cowork-input").fill("改成低于1700才提醒，其他照旧。");
  const stream = page.waitForResponse(r=>r.url().endsWith("/api/interaction") && r.request().method()==="POST");
  await page.locator("#cowork-send").click();
  const response = await stream;
  assert.ok((await response.body()).includes("1700.00"));
} finally {
  await browser.close();
}
