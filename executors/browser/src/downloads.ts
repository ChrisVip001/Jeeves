import type { Browser, CDPSession, Page } from "playwright";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type {
  BrowserDownloadView,
  BrowserDownloadAction,
} from "../../../protocol/schema/typescript/v2/index.ts";

const byteLimit = 768 * 1024;
export class BrowserDownloads {
  private rows = new Map<string, BrowserDownloadView>();
  private overflow = false;
  private cdp: CDPSession;
  private dir: string;
  private context: string;
  private constructor(cdp: CDPSession, dir: string, context: string) {
    this.cdp = cdp;
    this.dir = dir;
    this.context = context;
  }
  static async open(
    browser: Browser,
    page: Page,
    admitted: (url: string) => boolean,
  ) {
    const pageSession = await page.context().newCDPSession(page);
    const { targetInfo } = await pageSession.send("Target.getTargetInfo");
    await pageSession.detach();
    if (!targetInfo.browserContextId)
      throw Error("No isolated browser context for downloads");
    const cdp = await browser.newBrowserCDPSession();
    const dir = await mkdtemp(join(tmpdir(), "jeeves-downloads-"));
    const host = new BrowserDownloads(cdp, dir, targetInfo.browserContextId);
    try {
      await cdp.send("Browser.setDownloadBehavior", {
        behavior: "allowAndName",
        browserContextId: host.context,
        downloadPath: dir,
        eventsEnabled: true,
      });
      cdp.on("Browser.downloadWillBegin", (event) => {
        const { guid, suggestedFilename, url } = event;
        if (!/^[0-9a-f-]{36}$/i.test(guid)) return;
        if (host.rows.size >= 16) {
          host.overflow = true;
          void host.cancel(guid).catch(() => {
          });
          return;
        }
        const name =
          suggestedFilename.replace(/[\\/\x00-\x1f\x7f]/g, "_").slice(0, 100) ||
          "download";
        const row: BrowserDownloadView = {
          id: guid,
          name,
          status: "receiving",
          detail: null,
        };
        host.rows.set(guid, row);
        if (!admitted(url)) {
          row.status = "failed";
          row.detail = "Download requires human control and an allowed website";
          void host.cancel(guid).catch(() => {
          });
        }
      });
      cdp.on("Browser.downloadProgress", async (event) => {
        const row = host.rows.get(event.guid);
        if (!row || row.status !== "receiving") return;
        if (Math.max(event.receivedBytes, event.totalBytes) > byteLimit) {
          row.status = "failed";
          row.detail = "Download exceeds 768 KiB";
          await host.cancel(event.guid).catch(() => {
          });
        } else if (event.state === "completed") row.status = "pending";
        else if (event.state === "canceled") {
          row.status = "failed";
          row.detail = "Browser download interrupted";
        }
      });
      return host;
    } catch (error) {
      await host.close();
      throw error;
    }
  }
  private async cancel(id: string) {
    try {
      await this.cdp.send("Browser.cancelDownload", {
        guid: id,
        browserContextId: this.context,
      });
    } finally {
      await rm(join(this.dir, id), { force: true });
    }
  }
  list(): BrowserDownloadView[] {
    const rows = Array.from(this.rows.values()).reverse();
    if (this.overflow)
      rows.unshift({
        id: "limit",
        name: "",
        status: "failed",
        detail: "Download limit reached; open a fresh task browser",
      });
    return rows;
  }
  async decide(id: string, action: BrowserDownloadAction) {
    const row = this.rows.get(id);
    if (!row) throw Error("No such download in this browser");
    if (action === "cancel") {
      if (row.status === "cancelled") return;
      if (!["receiving", "pending"].includes(row.status))
        throw Error("Download cannot be cancelled");
      row.status = "cancelled";
      try {
        await this.cancel(id);
      } catch {
        row.status = "failed";
        row.detail = "Download cancellation could not be confirmed";
      }
      return;
    }
    if (row.status !== "pending") throw Error("Download is not ready to save");
    const path = join(this.dir, id);
    if ((await stat(path)).size > byteLimit)
      throw Error("Download exceeds 768 KiB");
    return (await readFile(path)).toString("base64");
  }
  async close() {
    await rm(this.dir, { recursive: true, force: true });
  }
}
