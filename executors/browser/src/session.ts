import { chromium, type Browser, type BrowserContext, type Page } from "playwright";
import type {
  BrowserWorkerCommand,
  BrowserWorkerState,
  BrowserFrameResponse,
  BrowserDownloadView,
  BrowserActInput,
  BrowserBatchReceipt,
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
  private context: BrowserContext | undefined;
  private page: Page | undefined;
  private clock: DocumentClock | undefined;
  private network: Awaited<ReturnType<typeof proxy>> | undefined;
  private scope: NetworkScope | undefined;
  private latest: Capture | undefined;
  private pendingProfile: string | undefined;
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
    this.context = undefined;
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
  private authority(expiresAt: string, generation: number) {
    const deadline = Date.parse(expiresAt);
    if (
      this.status !== "agent" ||
      this.manual ||
      generation !== this.generation ||
      !Number.isFinite(deadline) ||
      Date.now() > deadline ||
      deadline > Date.now() + 30000
    )
      throw Error(
        "Browser authority is stale; observe after the human returns control",
      );
  }

  private async applyAct(
    input: BrowserActInput,
    latest: Capture,
    authority: () => void,
  ): Promise<boolean> {
    const page = this.page;
    if (!page || !this.clock || !this.scope)
      throw Error("Browser is not ready for input");
    if ((await this.clock.read()).token !== latest.token)
      throw Error("Browser observation changed; observe again");
    authority();
    let attempted = false;
    if (input.kind === "navigate") {
      if (!input.url) throw Error("URL required");
      this.scope.permit(input.url);
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
    return attempted;
  }

  async command(command: BrowserWorkerCommand): Promise<{
    state: BrowserWorkerState;
    frame: BrowserFrameResponse | null;
    downloads?: BrowserDownloadView[];
    downloadData?: string;
    batch?: BrowserBatchReceipt[];
    profile?: string;
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
      this.network = command.cdpEndpoint ? undefined : await proxy(this.scope);
      this.lastHeartbeat = Date.now();
      this.watchdog = setInterval(() => {
        if (Date.now() - this.lastHeartbeat > 6000) void this.close();
      }, 1000);
      this.browser = command.cdpEndpoint
        ? await chromium.connectOverCDP(command.cdpEndpoint)
        : await chromium.launch({
            headless: command.headless,
            chromiumSandbox: true,
            proxy: { server: this.network!.url, bypass: "<-loopback>" },
            args: [
              "--disable-quic",
              "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
              "--disable-background-networking",
            ],
          });
      this.browser.on("disconnected", () => {
        if (this.status !== "closed") void this.close("needsAttention");
      });
      const storage = command.storageState
        ? JSON.parse(command.storageState)
        : undefined;
      const context = command.cdpEndpoint
        ? this.browser.contexts()[0]
        : await this.browser.newContext({
            serviceWorkers: "block",
            acceptDownloads: false,
            viewport: { width: 1200, height: 800 },
            permissions: [],
            ...(storage ? { storageState: storage } : {}),
          });
      if (!context) throw Error("Attached browser has no usable context");
      this.context = context;
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
      if (command.action === "saveProfile" && this.status !== "human")
        throw Error("Take control before saving the login profile");
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
      } else if (command.action === "saveProfile") {
        if (!this.context) throw Error("Browser context is unavailable");
        this.pendingProfile = JSON.stringify(await this.context.storageState());
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
      const authority = () => this.authority(command.expiresAt, command.generation);
      authority();
      const latest = this.latest;
      if (!latest || this.active) throw Error("Browser observation is stale; observe again before input");
      let attempted = false;
      const action = async () => {
        attempted = await this.applyAct(command.input, latest, authority);
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
    } else if (command.kind === "batch") {
      const authority = () => this.authority(command.expiresAt, command.generation);
      authority();
      const latest = this.latest;
      if (
        !latest ||
        this.active ||
        command.steps.length === 0 ||
        command.steps.length > 8
      )
        throw Error("Batch requires 1 to 8 steps and a fresh observation");
      const receipts: BrowserBatchReceipt[] = [];
      let attempted = false;
      const action = async () => {
        for (const [index, step] of command.steps.entries()) {
          if (step.observationId !== latest.observation.id)
            throw new ActionFailure(
              "Batch step has a stale observation",
              receipts.some((r) => r.uncertain),
            );
          try {
            attempted = (await this.applyAct(step, latest, authority)) || attempted;
            receipts.push({ index, kind: step.kind, status: "ok", uncertain: false });
          } catch (error) {
            const uncertain = error instanceof ActionFailure ? error.uncertain : attempted;
            receipts.push({
              index,
              kind: step.kind,
              status: "failed",
              error: String(error).slice(0, 300),
              uncertain,
            });
            throw new ActionFailure(String(error), uncertain);
          }
        }
      };
      this.active = action();
      try {
        await this.active;
      } catch (error) {
        throw Object.assign(new ActionFailure(String(error), receipts.some((r) => r.uncertain)), {
          receipts,
        });
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
          throw Object.assign(new ActionFailure(String(error), receipts.some((r) => r.uncertain)), {
            receipts,
          });
        }
      return { state: this.state(), frame: null, batch: receipts };
    }
    const profile = this.pendingProfile;
    this.pendingProfile = undefined;
    return { state: this.state(), frame: null, ...(profile ? { profile } : {}) };
  }
}
