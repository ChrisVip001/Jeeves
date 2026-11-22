import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { chromium } from "../node_modules/playwright/index.mjs";

const [workspace, cookie, action, message, database] = process.argv.slice(2);
const browser = await chromium.launch({channel:"chrome"});
try {
  const context = await browser.newContext({serviceWorkers:"block", ignoreHTTPSErrors:workspace.startsWith("https://127.0.0.1:")});
  const [name, value] = cookie.split("=");
  await context.addCookies([{name, value, url:new URL(workspace).origin}]);
  await context.addInitScript(() => {
    localStorage.setItem("jeeves.setup.dismissed","1");
    localStorage.setItem("jeeves.lang","zh");
  });
  const page = await context.newPage();
  await page.goto(workspace);
  await page.locator("#composer .workspace-picker button").first().waitFor();
  if (action === "amend" || action === "pause") {
    const delivered = page.locator("#messages [data-notice-id]").filter({hasText:"(calendar meeting-"});
    await delivered.nth(action === "amend" ? 0 : 1).waitFor();
  }
  await page.locator("#input").fill(message);
  const stream = page.waitForResponse(r => r.url().endsWith("/api/interaction") && r.request().method()==="POST");
  await page.locator("#send").click();
  const response = await stream;
  assert.equal(response.status(),200);
  const events=(await response.text()).split("\n").filter(l=>l.startsWith("data: ")).map(l=>JSON.parse(l.slice(6)));
  const reply=events.find(e=>e.type==="turnCompleted")?.reply;
  assert.ok(reply,JSON.stringify(events));
  const card=page.locator(".duty-cards article:visible").filter({hasText:message});
  const pending=await (await context.request.get(workspace+"api/responsibilities/reviews")).json();
  assert.equal(pending.length,1,reply);
  const review=pending[0];
  assert.equal(review.action,action==="preview"?"create":action,reply);
  await card.waitFor();
  const samples=[];
  if (action==="preview") {
    const sampled = page.waitForResponse(r=>r.url().endsWith("/api/responsibilities/sample"));
    await card.getByRole("button",{name:"预览一次"}).click();
    samples.push(await (await sampled).json());
  }
  if(action==="create"){
    for(let i=0;i<2;i++){
      const sampled=page.waitForResponse(r=>r.url().endsWith("/api/responsibilities/sample"));
      await card.getByRole("button",{name:"预览一次"}).click();
      const sample=await (await sampled).json();
      await card.getByText(sample,{exact:true}).waitFor();
      samples.push(sample);
    }
    assert.equal(samples[0],samples[1]);
    const db = new DatabaseSync(database, {readOnly:true});
    try {
      for (const table of ["responsibilities", "wake_inbox", "responsibility_runs", "receipts", "pending_approvals", "approval_log", "action_grants"]) {
        assert.equal(db.prepare("SELECT count(*) AS total FROM " + table).get().total, 0, table);
      }
    } finally { db.close(); }
    assert.equal((await (await context.request.get(workspace+"api/responsibilities")).json()).responsibilities.length,0);
  }
  const decision=page.waitForResponse(r=>r.url().endsWith("/api/responsibilities/decide"));
  await card.getByRole("button",{name:{create:"启用",amend:"确认修改",pause:"暂停",preview:"取消"}[action],exact:true}).click();
  assert.equal((await decision).status(),200);
  await card.waitFor({state:"detached"});
  if (action === "pause") {
    await page.locator("#mode-cowork").click();
    const duty = page.locator(".duty-cards article").filter({hasText:review.contract.purpose});
    await duty.getByText(/已暂停/).waitFor();
    await duty.getByRole("button",{name:"运行记录"}).click();
    await duty.locator("pre").getByText(/meeting-/).waitFor();
  }
  const history = await (await context.request.get(workspace+"api/session?sessionId=jeeves-main")).json();
  const storedReply = history.data.find(m=>m.role==="assistant" && m.content.includes("请核对这张委托卡"));
  assert.ok(storedReply, JSON.stringify(history).slice(0,1000));
  console.log(JSON.stringify({review,reply:storedReply.content.split("[tools used:")[0].trim(),samples}));
} finally {
  await browser.close();
}
