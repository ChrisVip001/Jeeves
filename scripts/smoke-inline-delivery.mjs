import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { chromium } from "../node_modules/playwright/index.mjs";

const [workspace, cookie, channel, shops, home, mode] = process.argv.slice(2);
const browser = await chromium.launch({headless: process.env.PW_HEADED !== "1", channel});
try {
  const context = await browser.newContext({serviceWorkers: "block"});
  const [name, value] = cookie.split("=");
  await context.addCookies([{name, value, url: new URL(workspace).origin}]);
  await context.addInitScript(() => {
    localStorage.setItem("jeeves.setup.dismissed", "1");
    localStorage.setItem("jeeves.lang", "zh");
  });
  const merchant = await context.newPage();
  const images = [];
  for (const key of ["a","b"]) {
    await merchant.goto(shops + "/shops/" + key);
    images.push({name: key + ".png", mimeType: "image/png", buffer: await merchant.screenshot()});
  }
  await merchant.close();
  const page = await context.newPage();
  await page.goto(workspace);
  async function submit(text, decision) {
    await page.locator("#input").fill(text);
    const stream = page.waitForResponse(response => response.url().endsWith("/api/interaction") && response.request().method() === "POST");
    await page.locator("#send").click();
    if (decision) {
      const card = page.locator(".approval-card:visible").filter({hasText: decision.destination}).last();
      await card.getByText(/shop\/deliver/).first().waitFor();
      assert.match(await card.textContent(), /到手 1720/);
      await card.getByRole("button", {name: decision.allow ? "只允许这次" : "拒绝", exact: true}).click();
    }
    const response = await stream;
    assert.equal(response.status(), 200);
    const events = (await response.body()).toString("utf8").split("\n").filter(line => line.startsWith("data: ")).map(line => JSON.parse(line.slice(6)));
    const done = events.find(e => e.type === "turnCompleted");
    assert.ok(done, JSON.stringify(events));
    const tasks = await context.request.get(workspace + "api/tasks");
    assert.deepEqual((await tasks.json()).tasks, []);
    return events.filter(e => e.type === "toolCompleted");
  }
  await page.locator("#composer .workspace-picker button").first().waitFor();
  await page.locator("#file").setInputFiles(images);
  await page.locator(".attach-thumb").nth(1).waitFor();
  await submit("这两款主要在家用，哪个更安静？");
  const messages = page.locator("#messages");
  await messages.getByText(/家用选 B/).last().waitFor();
  const prices = await submit("再查一下同规格到手价，算上运费。");
  assert.deepEqual(prices.map(e => e.status), ["ok","ok"]);
  for (const [key, summary] of [["a","到手 1720"], ["b","运费待核实"]]) {
    const link = messages.locator('a[href="' + shops + '/shops/' + key + '"]').last();
    await link.waitFor();
    assert.ok((await link.locator("..").textContent()).includes(summary));
    assert.match(await link.locator("..").textContent(), /查询时间 \d{4}-\d\d-\d\d/);
  }
  if (mode === "watch") {
    const stream = page.waitForResponse(response => response.url().endsWith("/api/interaction") && response.request().method() === "POST");
    await page.locator("#input").fill("第二款帮我盯三天，低于1800再告诉我，先别买。");
    await page.locator("#send").click();
    const response = await stream;
    const body = await response.body();
    assert.ok(body.includes("1800.00 CNY"), body.toString());
    await messages.getByText(/每小时核对含运费到手价/).last().waitFor();
    const retry = await context.request.post(response.url(), {data:response.request().postDataJSON()});
    assert.ok((await retry.body()).includes("1800.00 CNY"));
    const {tasks} = await (await context.request.get(workspace + "api/tasks")).json();
    assert.equal(tasks.length, 1);
    const task = tasks[0];
    assert.equal(task.budget.tools, 72);
    console.log(JSON.stringify({taskId:task.id, request:response.request().postDataJSON()}));
  } else {
    const [delivered] = await submit("把比价发送到 家庭收件箱", {allow:true, destination:"家庭收件箱"});
    assert.equal(delivered.status, "ok");
    assert.match(delivered.output, /^Error:/);
    await messages.getByText(/已收录 #R1/).last().waitFor();
    const receipt = await readFile(join(home,"receiver.jsonl"), "utf8");
    const received = JSON.parse(receipt);
    assert.equal(received.destination, "家庭收件箱");
    assert.match(received.body, /到手 1720/);
    assert.match(received.body, /运费待核实/);
    for (const key of ["a", "b"]) assert.ok(received.body.includes(shops + "/shops/" + key));
    const [denied] = await submit("把比价发送到 备用收件箱", {allow:false, destination:"备用收件箱"});
    assert.equal(denied.status, "error");
    await messages.getByText(/本次没有发送/).last().waitFor();
    assert.equal(await readFile(join(home,"receiver.jsonl"), "utf8"), receipt);
  }
} finally {
  await browser.close();
}
