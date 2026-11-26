import { randomUUID } from "node:crypto";
import type { FileChooser, Page } from "playwright";
import type {
  BrowserHumanInput,
  BrowserFrameResponse,
} from "../../../protocol/schema/typescript/v2/index.ts";
import type { DocumentClock } from "./document.ts";
import { frame } from "./observe.ts";

export class RemoteInput {
  private chooser?: { id: string; origin: string; target: FileChooser; expires: number };
  private current?: {
    id: string;
    generation: number;
    token: string;
    pixels: string;
    expires: number;
  };
  offer(page: Page, target: FileChooser) {
    this.chooser = {
      id: randomUUID(), origin: new URL(page.url()).origin, target,
      expires: Date.now() + 120_000,
    };
  }
  clear() {
    this.current = undefined;
    this.chooser = undefined;
  }
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
    const choice = this.chooser;
    return {
      ...snapshot,
      frameId: id,
      ...(choice && choice.expires >= Date.now() && choice.origin === new URL(page.url()).origin
        ? { upload: { chooserId: choice.id, origin: choice.origin } } : {}),
    };
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
    const choice = this.chooser;
    this.clear();
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
        await this.openChooser(page, () => page.mouse.click(input.x, input.y));
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
        {
          const key = {
            enter: "Enter",
            tab: "Tab",
            escape: "Escape",
            backspace: "Backspace",
          }[input.key];
          if (input.key === "enter") await this.openChooser(page, () => page.keyboard.press(key));
          else await page.keyboard.press(key);
        }
        break;
      case "scroll":
        await page.mouse.wheel(0, input.direction === "up" ? -500 : 500);
        break;
      case "upload": {
        if (!choice || choice.id !== input.chooserId || choice.expires < Date.now() ||
            choice.origin !== new URL(page.url()).origin ||
            await choice.target.element().ownerFrame() !== page.mainFrame() || !human())
          throw Error("Open the website file chooser again");
        if (!/^[^/\\:\u0000-\u001f\u007f]{1,255}\.(png|jpe?g)$/i.test(input.filename) ||
            input.data.length > 4 * Math.ceil(1024 * 1024 / 3))
          throw Error("Choose a PNG or JPEG image under 1 MiB");
        const buffer = Buffer.from(input.data, "base64");
        const png = /\.png$/i.test(input.filename);
        const signature = png ? "89504e470d0a1a0a" : "ffd8ff";
        if (buffer.length > 1024 * 1024 || !buffer.toString("hex", 0, signature.length / 2).startsWith(signature))
          throw Error("The image does not match its filename");
        await choice.target.setFiles({ name: input.filename, mimeType: png ? "image/png" : "image/jpeg", buffer });
        break;
      }
    }
  }
  private async openChooser(page: Page, input: () => Promise<void>) {
    const chooser = page.waitForEvent("filechooser", { timeout: 300 }).catch(() => undefined);
    await input();
    await chooser;
  }
}
