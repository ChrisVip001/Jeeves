import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import http from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, type Page } from "playwright";
import { BrowserSession } from "../src/session.ts";
import type {
  BrowserActInput,
  BrowserObservation,
} from "../../../protocol/schema/typescript/v2/index.ts";
const installed = existsSync(chromium.executablePath());
if (process.env.JEEVES_BROWSER_REQUIRED && !installed)
  throw Error("Install the optional browser runtime before smoke-browser");

test("attached Chrome shares sign-in and leaves existing tabs untouched", {
  skip: !installed ? "Optional Chromium not installed" : false,
  timeout: 30000,
}, async () => {
  const data = await mkdtemp(join(tmpdir(), "jeeves-chrome-test-"));
  const fixture = http.createServer((request, response) => {
    response.setHeader("Content-Type", "text/html");
    response.end(request.headers.cookie === "signed=in"
      ? "<h1>Signed-in workspace</h1><input type=file>"
      : "<h1>Sign in</h1>");
  });
  const other = http.createServer((_request, response) => response.end("Unrelated tab"));
  await Promise.all([fixture, other].map(server => new Promise<void>(resolve =>
    server.listen(0, "127.0.0.1", resolve))));
  const address = (server: http.Server) => {
    const address = server.address();
    assert.ok(address && typeof address !== "string");
    return `http://127.0.0.1:${address.port}`;
  };
  const context = await chromium.launchPersistentContext(data, {
    headless: true,
    args: ["--remote-debugging-port=0"],
  });
  const session = new BrowserSession();
  const heartbeat = setInterval(() => void session.command({ kind: "heartbeat" }), 1000);
  try {
    const url = address(fixture);
    const prior = context.pages()[0];
    await context.addCookies([{ url, name: "signed", value: "in" }]);
    await prior.goto(address(other));
    const [port, path] = (await readFile(join(data, "DevToolsActivePort"), "utf8")).trim().split("\n");
    const start = await session.command({
      kind: "launch", url, origins: [url], allowLocalhost: true,
      headless: true, generation: 1, storageState: null,
      cdpEndpoint: `ws://127.0.0.1:${port}${path}`, nativeChrome: true,
    });
    assert.match(start.state.observation!.text, /Signed-in workspace/);
    await prior.reload();
    assert.equal(await prior.locator("body").innerText(), "Unrelated tab");
    assert.equal(context.pages().length, 2);
    const takeover = await session.command({ kind: "control", generation: 2, action: "takeover", origin: null });
    assert.equal(takeover.state.status, "human");
    const returned = await session.command({ kind: "control", generation: 3, action: "return", origin: null });
    assert.equal(returned.state.status, "agent");
    assert.match(returned.state.observation!.text, /Signed-in workspace/);
    await assert.rejects(session.command({
      kind: "act", generation: 3, expiresAt: new Date(Date.now() + 5000).toISOString(),
      input: {
        kind: "navigate", observationId: returned.state.observation!.id, url: address(other),
        reference: null, expectedName: null, text: null,
      },
    }), /outside|allow|scope/i);
    await session.close();
    assert.equal(context.pages().length, 1);
    await prior.reload();
    assert.equal(await prior.locator("body").innerText(), "Unrelated tab");
  } finally {
    clearInterval(heartbeat);
    await session.close();
    await context.close();
    await Promise.all([fixture, other].map(server => new Promise<void>(resolve => server.close(() => resolve()))));
    await rm(data, { recursive: true, force: true });
  }
});

test(
  "real browser: scope, references, verification, takeover fences and human preview",
  {
    skip: !installed ? "Optional Chromium not installed" : false,
    timeout: 60000,
  },
  async () => {
    let clicks = 0;
    const fixture = http.createServer((req, res) => {
      if (req.url === "/saved") {
        clicks++;
        res.end("<body>Saved successfully</body>");
        return;
      }
      res.setHeader("Content-Type", "text/html");
      res.end(
        `<body><h1>Fixture</h1><label>Name<input placeholder="Name"></label><input type="password" aria-label="Password"><input autocomplete="one-time-code" aria-label="OTP"><button onclick="location.href='/saved'">Save</button><a href="http://169.254.169.254/">Blocked</a><button onclick="setTimeout(()=>document.body.append(' changed'),150)">Change later</button><input type="file" id="image" style="position:absolute;top:100px;left:8px"></body>`,
      );
    });
    await new Promise<void>((resolve) =>
      fixture.listen(0, "127.0.0.1", resolve),
    );
    const address = fixture.address();
    assert.ok(address && typeof address !== "string");
    const origin = `http://127.0.0.1:${address.port}`;
    const browser = new BrowserSession();
    const heartbeat = setInterval(
      () => void browser.command({ kind: "heartbeat" }),
      1000,
    );
    let observation: BrowserObservation;
    const observe = async () => {
      const result = await browser.command({ kind: "observe" });
      observation = result.state.observation!;
      return observation;
    };
    const act = async (
      input: Partial<BrowserActInput> & Pick<BrowserActInput, "kind">,
      generation = 1,
    ) =>
      browser.command({
        kind: "act",
        generation,
        expiresAt: new Date(Date.now() + 5000).toISOString(),
        input: {
          reference: null,
          expectedName: null,
          url: null,
          text: null,
          observationId: observation.id,
          ...input,
        },
      });
    try {
      await browser.command({
        kind: "launch",
        generation: 1,
        url: origin,
        origins: [origin],
        allowLocalhost: true,
        headless: true,
        storageState: null,
        cdpEndpoint: null,
      });
      await observe();
      assert.equal(
        observation!.elements.some((item) =>
          ["Password", "OTP"].includes(item.name),
        ),
        false,
      );
      const name = observation!.elements.find((item) => item.name === "Name")!;
      await act({
        kind: "fill",
        reference: name.reference,
        expectedName: "Name",
        text: "Jeeves",
      });
      await assert.rejects(
        act({ kind: "click", reference: name.reference, expectedName: "Name" }),
        /stale/,
      );
      await observe();
      const frame = await browser.command({ kind: "frame" });
      assert.match(frame.frame!.dataUrl, /^data:image\/jpeg;base64,/);
      assert.equal(frame.state.observation!.id, observation!.id);
      await observe();
      const later = observation!.elements.find(
        (item) => item.name === "Change later",
      )!;
      await act({
        kind: "click",
        reference: later.reference,
        expectedName: later.name,
      });
      await observe();
      await new Promise((resolve) => setTimeout(resolve, 250));
      await assert.rejects(act({ kind: "scroll", text: "down" }), /changed/);
      await observe();
      await assert.rejects(
        act({ kind: "navigate", url: "http://169.254.169.254/" }),
        /outside/,
      );
      await browser.command({
        kind: "control",
        generation: 2,
        action: "takeover",
        origin: null,
      });
      await assert.rejects(
        browser.command({ kind: "observe" }),
        /human|person/,
      );
      const humanPage = (browser as unknown as { page: Page }).page;
      await humanPage.getByPlaceholder("Name").fill("Human fixture edit");
      assert.equal(
        await humanPage.getByPlaceholder("Name").inputValue(),
        "Human fixture edit",
      );
      const humanFrame = await browser.command({ kind: "frame" });
      assert.ok(humanFrame.frame);
      const chosen = await browser.command({ kind: "humanInput", generation: 2,
        frameId: humanFrame.frame!.frameId!, input: { kind: "click", x: 40, y: 110 } });
      assert.equal(chosen.frame?.upload?.origin, origin);
      const upload = {
        kind: "humanInput" as const,
        generation: 2,
        frameId: chosen.frame!.frameId!,
        input: {
          kind: "upload" as const, chooserId: chosen.frame!.upload!.chooserId,
          filename: "fixture.png",
          data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/e1EAAAAASUVORK5CYII=",
        },
      };
      await browser.command(upload);
      await assert.rejects(browser.command(upload), /refresh/i);
      const attached = await humanPage.locator("#image").evaluate(
        async (element: HTMLInputElement) => Array.from(new Uint8Array(await element.files![0].arrayBuffer())),
      );
      assert.deepEqual(attached, [...Buffer.from(upload.input.data, "base64")]);
      await browser.command({
        kind: "control",
        generation: 3,
        action: "return",
        origin: null,
      });
      await observe();
      await assert.rejects(
        act({ kind: "scroll", text: "down" }, 1),
        /authority/,
      );
      const save = observation!.elements.find((item) => item.name === "Save")!;
      await act(
        { kind: "click", reference: save.reference, expectedName: "Save" },
        3,
      );
      await observe();
      assert.match(observation!.text, /Saved successfully/);
      assert.equal(clicks, 1);
      await browser.command({
        kind: "control",
        generation: 4,
        action: "pause",
        origin: null,
      });
      await assert.rejects(
        act({ kind: "scroll", text: "down" }, 4),
        /authority/,
      );
      await browser.command({
        kind: "control",
        generation: 5,
        action: "close",
        origin: null,
      });
      assert.equal(browser.state().status, "closed");
    } finally {
      clearInterval(heartbeat);
      await browser.close();
      await new Promise<void>((resolve) => fixture.close(() => resolve()));
    }
  },
);

test(
  "unexpected Chromium exit reports attention instead of a clean close",
  {
    skip: !installed ? "Optional Chromium not installed" : false,
    timeout: 20000,
  },
  async () => {
    const fixture = http.createServer((_req, res) =>
      res.end("<body>Exit fixture</body>"),
    );
    await new Promise<void>((resolve) =>
      fixture.listen(0, "127.0.0.1", resolve),
    );
    const address = fixture.address();
    assert.ok(address && typeof address !== "string");
    const origin = `http://127.0.0.1:${address.port}`;
    const session = new BrowserSession();
    try {
      await session.command({
        kind: "launch",
        generation: 1,
        url: origin,
        origins: [origin],
        allowLocalhost: true,
        headless: true,
        storageState: null,
        cdpEndpoint: null,
      });
      await (
        session as unknown as { browser: import("playwright").Browser }
      ).browser.close();
      assert.equal(session.state().status, "needsAttention");
      await assert.rejects(session.command({ kind: "frame" }), /closed/);
    } finally {
      await session.close();
      await new Promise<void>((resolve) => fixture.close(() => resolve()));
    }
  },
);
