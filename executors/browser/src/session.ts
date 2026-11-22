import { chromium, type Browser, type Page } from "playwright";
import type {
  BrowserWorkerCommand,
  BrowserWorkerState,
  BrowserFrameResponse,
  BrowserDownloadView,
} from "../../../protocol/schema/typescript/v2/index.ts";
import { BrowserDownloads } from "./downloads.ts";
import { RemoteInput } from "./remote.ts";
import { DocumentClock } from "./document.ts";
import { NetworkScope, proxy } from "./network.ts";
import { capture, installObservation, type Capture } from "./observe.ts";

export class ActionFailure extends Error {
  readonly uncertain: boolean;
  constructor(message: string, uncertain: boolean) {
    super(message);
    this.uncertain = uncertain;
  }
}
export class BrowserSession {
  private remote = new RemoteInput();
  private downloads?: BrowserDownloads;
  private browser: Browser | undefined;
  private page: Page | undefined;
  private clock: DocumentClock | undefined;
  private network: Awaited<ReturnType<typeof proxy>> | undefined;
  private scope: NetworkScope | undefined;
  private latest: Capture | undefined;
  private generation = 1;
  private status = "starting";
  private active: Promise<void> | undefined;
  private lastHeartbeat = Date.now();
  private watchdog: ReturnType<typeof setInterval> | undefined;
  private manual = false;
  state(): BrowserWorkerState {
    return {
      generation: this.generation,
      status: this.status,
      observation: this.latest?.observation ?? null,
      blockedOrigins: [...(this.scope?.blocked || [])],
    };
  }
  async close(status = "closed") {
    if (["closed", "needsAttention"].includes(this.status)) return;
    this.status = status;
    this.generation++;
    if (this.watchdog) clearInterval(this.watchdog);
    await this.browser?.close().catch(() => {});
    await this.downloads?.close();
    await this.network?.close();
  }
  private async observe() {
    if (!this.page || ["closed", "needsAttention"].includes(this.status))
      throw Error("Browser is closed");
    if (this.active) throw Error("Browser input is still settling");
    const old = this.latest;
    this.scope!.permit(this.page.url());
    const generation = this.generation;
    const status = this.status;
    const next = await capture(this.page, this.clock!);
    if (generation !== this.generation || status !== this.status) {
      await Promise.allSettled(
        [...next.references.values()].map((handle) => handle.dispose()),
      );
      throw Error("Browser control changed while observing");
    }
    this.latest = next;
    if (old)
      for (const handle of old.references.values()) await handle.dispose();
  }
  async command(command: BrowserWorkerCommand): Promise<{
    state: BrowserWorkerState;
    frame: BrowserFrameResponse | null;
    downloads?: BrowserDownloadView[];
    downloadData?: string;
  }> {
    if (command.kind === "hello" || command.kind === "heartbeat") {
      this.lastHeartbeat = Date.now();
      return { state: this.state(), frame: null };
    }
    if (command.kind === "launch") {
      if (this.browser) throw Error("Browser already launched");
      this.generation = command.generation;
      this.scope = new NetworkScope(command.origins, command.allowLocalhost);
      this.scope.permit(command.url);
      this.network = await proxy(this.scope);
      this.lastHeartbeat = Date.now();
      this.watchdog = setInterval(() => {
        if (Date.now() - this.lastHeartbeat > 6000) void this.close();
      }, 1000);
      this.browser = await chromium.launch({
        headless: command.headless,
        chromiumSandbox: true,
        proxy: { server: this.network.url, bypass: "<-loopback>" },
        args: [
          "--disable-quic",
          "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
          "--disable-background-networking",
        ],
      });
      this.browser.on("disconnected", () => {
        if (this.status !== "closed") void this.close("needsAttention");
      });
      const context = await this.browser.newContext({
        serviceWorkers: "block",
        acceptDownloads: false,
        viewport: { width: 1200, height: 800 },
        permissions: [],
      });
      await context.routeWebSocket("**/*", (route) => route.close());
      await context.route("**/*", async (route) => {
        try {
          this.scope!.permit(route.request().url());
          await route.continue();
        } catch {
          await route.abort("blockedbyclient");
        }
      });
      this.page = await context.newPage();
      this.clock = await DocumentClock.create(this.page);
      this.page.on("popup", (popup) => {
        this.scope?.blocked.add("popup blocked");
        void popup.close();
      });
      this.page.on("filechooser", (chooser) => {
        this.scope?.blocked.add("uploads unsupported");
        void chooser.setFiles([]);
      });
      this.downloads = await BrowserDownloads.open(
        this.browser,
        this.page,
        (url) => {
          if (this.status !== "human") return false;
          try {
            this.scope!.permit(url);
            return true;
          } catch {
            return false;
          }
        },
      );
      this.page.on("dialog", (dialog) => {
        if (this.status !== "human") {
          this.scope?.blocked.add(
            "dialog cancelled; take over before retrying",
          );
          void dialog.dismiss();
        }
      });
      await installObservation(this.page, () => {
        if (!this.active && this.status === "agent") {
          this.status = "paused";
          this.manual = true;
          const previous = this.latest;
          this.latest = undefined;
          if (previous)
            void Promise.allSettled(
              [...previous.references.values()].map((handle) =>
                handle.dispose(),
              ),
            );
        }
      });
      this.status = "agent";
      await this.page.goto(command.url, {
        waitUntil: "domcontentloaded",
        timeout: 12000,
      });
      await this.observe();
    } else if (command.kind === "control") {
      if (command.generation <= this.generation)
        throw Error("Stale browser control generation");
      this.generation = command.generation;
      this.status = "pausing";
      const previous = this.latest;
      this.latest = undefined;
      if (this.active) await this.active.catch(() => {});
      if (previous)
        await Promise.allSettled(
          [...previous.references.values()].map((handle) => handle.dispose()),
        );
      if (command.action === "close") {
        await this.close();
      } else if (command.action === "allowOrigin") {
        if (!command.origin) throw Error("Origin required");
        this.scope!.add(command.origin);
        this.status = "human";
        await this.page?.bringToFront();
      } else {
        this.status =
          command.action === "takeover"
            ? "human"
            : command.action === "return"
              ? "agent"
              : "paused";
        this.manual = false;
        if (this.status === "human") await this.page?.bringToFront();
        if (this.status === "agent") await this.observe();
      }
    } else if (command.kind === "observe") {
      if (this.status !== "agent")
        throw Error(
          "Browser is paused or controlled by a person; wait for the human to return control",
        );
      await this.observe();
    } else if (command.kind === "downloads" || command.kind === "download") {
      if (!this.downloads) throw Error("No active download context");
      if (command.kind === "downloads")
        return {
          state: this.state(),
          frame: null,
          downloads: this.downloads.list(),
        };
      if (this.status !== "human" || command.generation !== this.generation)
        throw Error("Download authority changed");
      const downloadData = await this.downloads.decide(
        command.downloadId,
        command.action,
      );
      return {
        state: this.state(),
        frame: null,
        downloads: this.downloads.list(),
        ...(downloadData === undefined ? {} : { downloadData }),
      };
    } else if (command.kind === "frame" || command.kind === "humanInput") {
      if (this.active) throw Error("Browser input is still settling");
      if (
        !this.page ||
        !this.clock ||
        ["closed", "needsAttention"].includes(this.status)
      )
        throw Error("Browser is closed");
      const generation = this.generation;
      const human = () =>
        this.status === "human" && generation === this.generation;
      if (command.kind === "humanInput") {
        if (!human() || command.generation !== generation)
          throw Error("Take control before sending browser input");
        this.active = this.remote.apply(
          this.page,
          this.clock,
          generation,
          command.frameId,
          command.input,
          human,
        );
        try {
          await this.active;
        } finally {
          this.active = undefined;
        }
        if (!human())
          throw Error("Control changed; inspect the page before another input");
      }
      const preview = await this.remote.capture(
        this.page,
        this.clock,
        generation,
        human,
      );
      return { state: this.state(), frame: preview };
    } else if (command.kind === "act") {
      const authority = () => {
        const deadline = Date.parse(command.expiresAt);
        if (
          this.status !== "agent" ||
          this.manual ||
          command.generation !== this.generation ||
          !Number.isFinite(deadline) ||
          Date.now() > deadline ||
          deadline > Date.now() + 30000
        )
          throw Error(
            "Browser authority is stale; observe after the human returns control",
          );
      };
      authority();
      const page = this.page;
      const latest = this.latest;
      if (
        !page ||
        !latest ||
        this.active ||
        command.input.observationId !== latest.observation.id
      )
        throw Error("Browser observation is stale; observe again before input");
      const input = command.input;
      let attempted = false;
      const action = async () => {
        if ((await this.clock!.read()).token !== latest.token)
          throw Error("Browser observation changed; observe again");
        authority();
        if (input.kind === "navigate") {
          if (!input.url) throw Error("URL required");
          this.scope!.permit(input.url);
          attempted = true;
          await page.goto(input.url, {
            waitUntil: "domcontentloaded",
            timeout: 12000,
          });
        } else if (input.kind === "scroll") {
          if (!["up", "down"].includes(input.text || ""))
            throw Error("Scroll direction must be up or down");
          attempted = true;
          await page.mouse.wheel(0, input.text === "up" ? -500 : 500);
        } else {
          const element = latest.references.get(input.reference || "");
          if (!element) throw Error("Unknown element reference");
          if (
            !input.expectedName ||
            latest.observation.elements.find(
              (item) => item.reference === input.reference,
            )?.name !== input.expectedName
          )
            throw Error(
              "Expected element name does not match the observed reference",
            );
          const unsafe = await element.evaluate((el) => {
            const node = el as HTMLInputElement;
            const href = node.getAttribute("href");
            return (
              ["password", "file"].includes(node.type) ||
              /password|one-time-code/.test(node.autocomplete || "") ||
              (!!href &&
                !["http:", "https:"].includes(
                  new URL(href, location.href).protocol,
                ))
            );
          });
          authority();
          if (unsafe)
            throw Error(
              "Sensitive fields and external protocols require human control",
            );
          if (input.kind === "fill") {
            if (input.text == null || input.text.length > 8000)
              throw Error("Invalid text length");
            attempted = true;
            await element.fill(input.text, { timeout: 1500 });
          } else {
            attempted = true;
            await element.click({ timeout: 1500, noWaitAfter: true });
          }
        }
      };
      this.active = action();
      try {
        await this.active;
      } catch (error) {
        throw new ActionFailure(String(error), attempted);
      } finally {
        this.active = undefined;
        this.latest = undefined;
        await Promise.allSettled(
          [...latest.references.values()].map((handle) => handle.dispose()),
        );
      }
      if (this.status === "agent")
        try {
          await this.observe();
        } catch (error) {
          throw new ActionFailure(String(error), attempted);
        }
    }
    return { state: this.state(), frame: null };
  }
}
