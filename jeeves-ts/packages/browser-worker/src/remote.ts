import { randomUUID } from "node:crypto";
import type { Page } from "playwright";
import type {
  BrowserHumanInput,
  BrowserFrameResponse,
} from "../../../../jeeves-rs/jeeves-protocol/schema/typescript/v2/index.ts";
import type { DocumentClock } from "./document.ts";
import { frame } from "./observe.ts";

export class RemoteInput {
  private current?: {
    id: string;
    generation: number;
    token: string;
    pixels: string;
    expires: number;
  };
  async capture(
    page: Page,
    clock: DocumentClock,
    generation: number,
    human: () => boolean,
  ): Promise<BrowserFrameResponse> {
    const snapshot = await frame(page);
    if (!human()) return snapshot;
    const token = (await clock.read()).token;
    const id = randomUUID();
    if (!human()) throw Error("Browser control changed during capture");
    this.current = {
      id,
      generation,
      token,
      pixels: snapshot.dataUrl,
      expires: Date.now() + 30_000,
    };
    return { ...snapshot, frameId: id };
  }
  async apply(
    page: Page,
    clock: DocumentClock,
    generation: number,
    id: string,
    input: BrowserHumanInput,
    human: () => boolean,
  ) {
    const seen = this.current;
    if (!human() || !seen || seen.id !== id || seen.generation !== generation)
      throw Error("Refresh the human browser frame");
    this.current = undefined;
    if (seen.expires < Date.now() || (await clock.read()).token !== seen.token)
      throw Error("Browser frame expired or changed");
    if ((await frame(page)).dataUrl !== seen.pixels || !human())
      throw Error("Browser display changed; refresh the frame");
    const size = page.viewportSize();
    switch (input.kind) {
      case "click":
        if (
          !size ||
          !Number.isInteger(input.x) ||
          !Number.isInteger(input.y) ||
          input.x < 0 ||
          input.x >= size.width ||
          input.y < 0 ||
          input.y >= size.height
        )
          throw Error("Click lies outside the browser viewport");
        await page.mouse.click(input.x, input.y);
        break;
      case "text":
        if (!input.text || input.text.length > 4000)
          throw Error("Invalid input length");
        try {
          await page.keyboard.insertText(input.text);
        } catch {
          throw Error("Text input interrupted; inspect the page");
        }
        break;
      case "key":
        await page.keyboard.press(
          {
            enter: "Enter",
            tab: "Tab",
            escape: "Escape",
            backspace: "Backspace",
          }[input.key],
        );
        break;
      case "scroll":
        await page.mouse.wheel(0, input.direction === "up" ? -500 : 500);
        break;
    }
  }
}
