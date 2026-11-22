import assert from "node:assert/strict";
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import vue from "@vitejs/plugin-vue";
import { createServer } from "vite";

const executablePath = process.env.JEEVES_UI_CHROMIUM || chromium.executablePath();
const screenshotDir = process.env.JEEVES_UI_SCREENSHOT_DIR;
test("ordinary task timeline and approval decisions are bilingual, including the row's latest event", {
  skip: !existsSync(executablePath) ? "Optional Chromium not installed" : false,
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
        const labels = lang === "zh" ? ["本次执行开始", "请求确认", "确认结果", "本次执行结束"] : ["Run started", "Approval requested", "Approval decision", "Run ended"];
        const page = await browser.newPage({ viewport: { width: 390, height: 844 }, locale: lang === "zh" ? "zh-CN" : "en-US", colorScheme: "dark", reducedMotion: "reduce" });
        await page.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/task-timeline.html?lang=${lang}`);
        const row = page.locator(".row-open");
        await row.waitFor();
        assert.equal(await row.locator(".phrase").innerText(), labels[0]);
        await row.focus();
        await page.keyboard.press("Enter");
        const dialog = page.getByRole("dialog");
        await dialog.waitFor();
        assert.deepEqual(await dialog.locator(".tl-kind").allTextContents(), [labels[0], labels[1], labels[2], labels[2], labels[3], labels[0]]);
        assert.deepEqual(await dialog.locator(".tl-text").allTextContents(), lang === "zh"
          ? ["email_send · ap-fixture", "email_send · 已拒绝 · fixture-person", "email_send · 已超时", "本次执行完成"]
          : ["email_send · ap-fixture", "email_send · Denied · fixture-person", "email_send · Timed out", "Run completed"]);
        assert.equal(await dialog.evaluate(el => el.scrollWidth > el.clientWidth), false);
        assert.equal(await dialog.evaluate(el => getComputedStyle(el).position), "fixed", "fixture loads the real drawer stylesheet");
        assert.equal(await dialog.evaluate(el => getComputedStyle(el).animationName), "none");
        if (screenshotDir && lang === "zh") {
          for (const colorScheme of ["light", "dark"] as const) {
            await page.emulateMedia({ colorScheme, reducedMotion: "reduce" });
            for (const [size, viewport] of [["desktop", { width: 1280, height: 900 }], ["narrow", { width: 390, height: 844 }]] as const) {
              await page.setViewportSize(viewport);
              await page.screenshot({ path: join(screenshotDir, `task-timeline-${colorScheme}-${size}.png`) });
            }
          }
        }
        await page.keyboard.press("Escape");
        await dialog.waitFor({ state: "hidden" });
        assert.equal(await row.evaluate(el => document.activeElement === el), true);
        await page.close();
        for (const checkpoint of ["active", "queued"]) {
          const active = await browser.newPage();
          await active.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/task-timeline.html?checkpoint=${checkpoint}&lang=${lang}`);
          await active.locator(".row-open").click();
          const section = active.locator(".td-checkpoint");
          await section.waitFor();
          await section.locator(".checkpoint-policy-status").waitFor();
          assert.equal(await section.getByRole("button").count(), 0, "running or queued policy is read-only");
          await active.close();
        }
        const idle = await browser.newPage({ viewport: { width: 390, height: 844 }, colorScheme: "dark", reducedMotion: "reduce" });
        await idle.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/task-timeline.html?checkpoint=idle&lang=${lang}`);
        await idle.locator(".row-open").click();
        const section = idle.locator(".td-checkpoint");
        await section.locator(".checkpoint-head button").focus();
        await idle.keyboard.press("Enter");
        const form = section.locator("form.checkpoint-form:not(.checkpoint-policy-form)");
        await form.waitFor();
        const paths = form.locator("textarea[name=paths]");
        const confirmed = form.locator("input[name=confirmed]");
        const save = form.locator("button[type=submit]");
        assert.equal(await save.isDisabled(), true);
        await paths.fill("../outside.txt");
        await confirmed.check();
        assert.equal(await save.isDisabled(), true, "only explicit relative paths are allowed");
        await paths.fill("blocked.txt");
        assert.equal(await confirmed.isChecked(), false, "changing files revokes confirmation");
        await confirmed.check();
        await save.click();
        await form.getByRole("alert").waitFor();
        assert.equal(await paths.inputValue(), "blocked.txt", "refusal retains exact user input");
        await paths.fill("src/main.rs\nREADME.md");
        assert.equal(await save.isDisabled(), true);
        await confirmed.check();
        if (screenshotDir && lang === "zh") {
          for (const colorScheme of ["light", "dark"] as const) {
            await idle.emulateMedia({ colorScheme, reducedMotion: "reduce" });
            for (const [size, viewport] of [["desktop", { width: 1280, height: 900 }], ["narrow", { width: 390, height: 844 }]] as const) {
              await idle.setViewportSize(viewport);
              await form.scrollIntoViewIfNeeded();
              await idle.screenshot({ path: join(screenshotDir, `git-checkpoint-${colorScheme}-${size}.png`) });
            }
          }
        }
        await save.click();
        await form.waitFor({ state: "detached" });
        const commit = "1234567890abcdef1234567890abcdef12345678";
        await section.locator(".checkpoint-result").waitFor();
        assert.match(await section.locator(".checkpoint-result").innerText(), new RegExp(commit));
        assert.equal(await idle.locator(".tl-kind").last().innerText(), lang === "zh" ? "Git 检查点" : "Git checkpoint");
        assert.equal(await idle.locator(".tl-text").last().innerText(), commit);
        assert.deepEqual(await idle.evaluate(() => (window as unknown as { fixtureCheckpoints: unknown[] }).fixtureCheckpoints), [
          { taskId: "fixture", paths: ["blocked.txt"], confirmed: true },
          { taskId: "fixture", paths: ["src/main.rs", "README.md"], confirmed: true },
        ]);
        const policy = section.locator(".checkpoint-policy");
        const status = policy.locator(".checkpoint-policy-status");
        await status.waitFor();
        assert.match(await status.innerText(), lang === "zh" ? /已关闭/ : /Off/);
        assert.deepEqual(await idle.evaluate(() => (window as unknown as { fixturePolicyWrites: unknown[] }).fixturePolicyWrites), [], "opening task and manually committing cannot enable automatic checkpoints");
        await policy.locator("button[data-policy=enable]").click();
        const policyForm = policy.locator("form");
        await policyForm.waitFor();
        const policyPaths = policyForm.locator("textarea[name=paths]");
        const policyConfirmed = policyForm.locator("input[name=confirmed]");
        const policySave = policyForm.locator("button[type=submit]");
        await policyPaths.fill("blocked.txt");
        await policyConfirmed.check();
        await policySave.click();
        await policyForm.getByRole("alert").waitFor();
        assert.equal(await policyPaths.inputValue(), "blocked.txt");
        assert.equal(await policyConfirmed.isChecked(), false);
        assert.match(await status.innerText(), lang === "zh" ? /已关闭/ : /Off/);
        await policyPaths.fill("src/main.rs\nREADME.md");
        assert.equal(await policySave.isDisabled(), true);
        await policyConfirmed.check();
        await policyPaths.fill("src/main.rs\nREADME.md\nnotes.md");
        assert.equal(await policyConfirmed.isChecked(), false);
        await policyConfirmed.check();
        if (screenshotDir && lang === "zh") {
          for (const colorScheme of ["light", "dark"] as const) {
            await idle.emulateMedia({ colorScheme, reducedMotion: "reduce" });
            for (const [size, viewport] of [["desktop", { width: 1280, height: 900 }], ["narrow", { width: 390, height: 844 }]] as const) {
              await idle.setViewportSize(viewport);
              await policyForm.scrollIntoViewIfNeeded();
              await idle.screenshot({ path: join(screenshotDir, `git-auto-checkpoint-${colorScheme}-${size}.png`) });
            }
          }
        }
        await policySave.click();
        await policyForm.waitFor({ state: "detached" });
        assert.match(await status.innerText(), lang === "zh" ? /已开启/ : /On/);
        assert.deepEqual(await policy.locator(".checkpoint-policy-paths li").allTextContents(), ["src/main.rs", "README.md", "notes.md"]);
        await policy.locator("button[data-policy=disable]").click();
        await policyForm.waitFor();
        assert.equal(await policySave.isDisabled(), true);
        await policyConfirmed.check();
        await policySave.click();
        await policyForm.waitFor({ state: "detached" });
        assert.match(await status.innerText(), lang === "zh" ? /已关闭/ : /Off/);
        assert.deepEqual(await idle.evaluate(() => (window as unknown as { fixturePolicyWrites: unknown[] }).fixturePolicyWrites), [
          { taskId: "fixture", paths: ["blocked.txt"], enabled: true, confirmed: true },
          { taskId: "fixture", paths: ["src/main.rs", "README.md", "notes.md"], enabled: true, confirmed: true },
          { taskId: "fixture", paths: [], enabled: false, confirmed: true },
        ]);
        assert.equal(await idle.getByRole("dialog").evaluate(el => el.scrollWidth > el.clientWidth), false);
        await idle.close();
        const failed = await browser.newPage();
        await failed.goto(`http://127.0.0.1:${address.port}/ui-fixtures/connections/task-timeline.html?checkpoint=idle&failPolicy&lang=${lang}`);
        await failed.locator(".row-open").click();
        await failed.locator(".checkpoint-policy").getByRole("alert").waitFor();
        assert.equal(await failed.locator(".checkpoint-policy button[data-policy=enable]").count(), 0);
        assert.deepEqual(await failed.evaluate(() => (window as unknown as { fixturePolicyWrites: unknown[] }).fixturePolicyWrites), []);
        await failed.close();
      }
    } finally {
      await browser.close();
    }
  } finally {
    await server.close();
  }
});
