import { randomUUID } from "node:crypto";
import readline from "node:readline";
import type {
  BrowserWorkerRequest,
  BrowserWorkerResponse,
} from "../../../protocol/schema/typescript/v2/index.ts";
import { BrowserSession, ActionFailure } from "./session.ts";
const info = {
  schemaVersion: 1,
  instanceId: randomUUID(),
  capabilities: [
    "humanInput",
    "downloads",
    "download",
    "observe",
    "navigate",
    "click",
    "fill",
    "scroll",
    "batch",
    "cdpAttach",
    "frame",
    "control",
  ],
  maxOrigins: 12,
  maxFrameBytes: 800000,
};
const session = new BrowserSession();
let finished = false;
const stop = async () => {
  if (finished) return;
  finished = true;
  await session.close();
  process.exit(0);
};
process.on("SIGTERM", () => void stop());
process.on("SIGINT", () => void stop());
const lines = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});
lines.on("close", () => void stop());
lines.on("line", (line) => {
    if (line.length > 1_410_000) {
    void stop();
    return;
  }
  void (async () => {
    let request: BrowserWorkerRequest;
    try {
      request = JSON.parse(line);
      if (line.length > 64000 && !(request.command?.kind === "humanInput" && request.command.input?.kind === "upload"))
        throw Error("Request exceeds the browser command limit");
      if (
        !Number.isInteger(request.id) ||
        request.id < 1 ||
        !request.command ||
        ![
          "hello",
          "launch",
          "heartbeat",
          "control",
          "frame",
          "humanInput",
          "downloads",
          "download",
          "observe",
          "act",
          "batch",
        ].includes(request.command.kind)
      )
        throw Error("Invalid request");
    } catch {
      void stop();
      return;
    }
    let reply: BrowserWorkerResponse;
    try {
      const result = await session.command(request.command);
      reply = {
        id: request.id,
        info,
        ...result,
        error: null,
        uncertain: false,
      };
    } catch (error) {
      reply = {
        id: request.id,
        info,
        state: session.state(),
        frame: null,
        batch: (error as { receipts?: BrowserWorkerResponse["batch"] }).receipts ?? null,
        error: String(error).slice(0, 500),
        uncertain: error instanceof ActionFailure && error.uncertain,
      };
    }
    process.stdout.write(JSON.stringify(reply) + "\n");
  })();
});
