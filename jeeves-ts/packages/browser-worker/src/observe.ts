import type { Page, ElementHandle } from "playwright";
import type { DocumentClock } from "./document.ts";
import { randomUUID } from "node:crypto";
import type { BrowserObservation } from "../../../../jeeves-rs/jeeves-protocol/schema/typescript/v2/index.ts";
export interface Capture {
  observation: BrowserObservation;
  references: Map<string, ElementHandle>;
  token: string;
}
export async function installObservation(page: Page, onManual: () => void) {
  await page.exposeBinding("__jeevesManual", () => onManual());
  await page.addInitScript(() => {
    const callback = (window as unknown as { __jeevesManual: () => void })
      .__jeevesManual;
    for (const event of ["pointerdown", "keydown", "wheel"])
      document.addEventListener(
        event,
        (e) => {
          if (e.isTrusted) callback();
        },
        { capture: true },
      );
  });
}
export async function capture(
  page: Page,
  clock: DocumentClock,
): Promise<Capture> {
  const before = await clock.read();
  const candidates = page.locator(
    "a[href],button,input:not([type=hidden]),textarea,select,[role=button],[role=textbox]",
  );
  const handles: ElementHandle[] = [];
  const references = new Map<string, ElementHandle>();
  const elements: BrowserObservation["elements"] = [];
  try {
    const count = Math.min(await candidates.count(), 120);
    for (let index = 0; index < count; index++) {
      const handle = await candidates
        .nth(index)
        .elementHandle({ timeout: 1500 });
      if (handle) handles.push(handle);
    }
    for (const handle of handles.slice(0, 120)) {
      const info = await handle.evaluate((element) => {
        const node = element as HTMLElement;
        const rect = node.getBoundingClientRect();
        const type = node.getAttribute("type") || "";
        const autocomplete = node.getAttribute("autocomplete") || "";
        if (
          !rect.width ||
          !rect.height ||
          ["password", "file"].includes(type) ||
          /password|one-time-code/.test(autocomplete) ||
          node.closest("[aria-hidden=true]")
        )
          return null;
        const name =
          node.getAttribute("aria-label") ||
          node.getAttribute("placeholder") ||
          (node instanceof HTMLInputElement
            ? node.labels?.[0]?.innerText
            : node.innerText) ||
          node.tagName.toLowerCase();
        const role =
          node.getAttribute("role") ||
          (node.tagName === "A"
            ? "link"
            : node.tagName === "BUTTON"
              ? "button"
              : ["INPUT", "TEXTAREA"].includes(node.tagName)
                ? "textbox"
                : node.tagName.toLowerCase());
        return { name: name.slice(0, 160), role };
      });
      if (info) {
        const reference = `e${elements.length + 1}`;
        references.set(reference, handle);
        elements.push({ reference, ...info });
      } else await handle.dispose();
    }
    for (const handle of handles.slice(120)) await handle.dispose();
    const text = await page.evaluate(
      () => document.body?.innerText.slice(0, 10000) || "",
    );
    if (before.token !== (await clock.read()).token) {
      for (const handle of references.values()) await handle.dispose();
      throw Error("Page changed while observing; observe again");
    }
    return {
      observation: {
        id: randomUUID(),
        url: page.url(),
        title: (await page.title()).slice(0, 300),
        documentVersion: before.version,
        elements,
        text,
        collectedAt: new Date().toISOString(),
      },
      references,
      token: before.token,
    };
  } catch (error) {
    await Promise.allSettled(handles.map((handle) => handle.dispose()));
    throw error;
  }
}

/** Human preview only: it must not replace the agent's current observation or references. */
export async function frame(page: Page) {
  const bytes = await page.screenshot({
    type: "jpeg",
    quality: 60,
    scale: "css",
    timeout: 2000,
    mask: [
      page.locator(
        "input[type=password],input[autocomplete*=password],input[autocomplete=one-time-code]",
      ),
    ],
  });
  if (bytes.length > 800_000) throw Error("Browser frame exceeds limit");
  return {
    dataUrl: `data:image/jpeg;base64,${bytes.toString("base64")}`,
    observedAt: new Date().toISOString(),
  };
}
