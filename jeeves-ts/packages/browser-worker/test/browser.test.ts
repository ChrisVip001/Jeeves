import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import http from "node:http";
import { chromium, type Page } from "playwright";
import { BrowserSession } from "../src/session.ts";
import type {
  BrowserActInput,
  BrowserObservation,
} from "../../../../jeeves-rs/jeeves-protocol/schema/typescript/v2/index.ts";
const installed = existsSync(chromium.executablePath());
if (process.env.JEEVES_BROWSER_REQUIRED && !installed)
  throw Error("Install the optional browser runtime before smoke-browser");

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
        `<body><h1>Fixture</h1><label>Name<input placeholder="Name"></label><input type="password" aria-label="Password"><input autocomplete="one-time-code" aria-label="OTP"><button onclick="location.href='/saved'">Save</button><a href="http://169.254.169.254/">Blocked</a><button onclick="setTimeout(()=>document.body.append(' changed'),150)">Change later</button></body>`,
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
