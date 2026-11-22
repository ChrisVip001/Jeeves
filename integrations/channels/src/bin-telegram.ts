import { JeevesBridge } from "./bridge.ts";
import { TelegramAdapter } from "./telegram.ts";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { loadEnvFile } from "node:process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const envFile = fileURLToPath(new URL("../../../.env", import.meta.url));
if (existsSync(envFile)) loadEnvFile(envFile);

const token = process.env["TELEGRAM_BOT_TOKEN"]?.trim();
if (token === undefined || token === "") {
  console.error(
    "TELEGRAM_BOT_TOKEN was not given.\n" +
      "Make a bot with @BotFather, then:\n" +
      "  TELEGRAM_BOT_TOKEN=… TELEGRAM_ALLOW=<your numeric user id> npm run gateway:telegram",
  );
  process.exit(1);
}

const allow = (process.env["TELEGRAM_ALLOW"] ?? "")
  .split(",")
  .map((entry) => entry.trim())
  .filter((entry) => entry !== "");
const claim = process.env["TELEGRAM_CLAIM"] === "1";
if (allow.length === 0 && !claim) {
  console.error(
    "Note: TELEGRAM_ALLOW is unset, so anyone who finds the bot can use it and spend your model quota.",
  );
}

const home = process.env["JEEVES_HOME"]?.trim() || `${process.env["HOME"] ?? "."}/.jeeves`;
const offsetFile = `${home}/telegram-offset`;
const offset = {
  read: (): number => {
    try {
      return Number(readFileSync(offsetFile, "utf8").trim()) || 0;
    } catch {
      return 0;
    }
  },
  write: (next: number): void => {
    try {
      writeFileSync(offsetFile, String(next));
    } catch (error) {
      console.error(`[telegram] could not save the update offset: ${error instanceof Error ? error.message : String(error)}`);
    }
  },
};

const runtimeUrl = process.env["JEEVES_GATEWAY_URL"]?.trim();
const bridge = new JeevesBridge({
  cwd: root,
  ...(runtimeUrl ? { url: runtimeUrl, token: process.env["JEEVES_OPS_TOKEN"] } : {}),
});
const adapter = new TelegramAdapter(
  {
    token,
    allow,
    claim,
    offset,
    onLog: (text) => console.error(`[telegram] ${text}`),
    onApproval: (action, actor) =>
      bridge.answerApprovalDetailed(action.turnId, action.requestId, action.approved, `telegram:${actor}`, action.version),
  },
  async (conversation, text, source, requestId) =>
    (await bridge.ask(conversation, text, source, requestId)).reply,
);

bridge.onApprovalRequested((request) => {
  for (const chatId of allow) {
    void adapter
      .sendApproval(chatId, request)
      .catch((error: unknown) =>
        console.error(`[telegram] could not ask ${chatId} for approval: ${error instanceof Error ? error.message : String(error)}`),
      );
  }
});

const shutdown = (): void => {
  adapter.stop();
  bridge.close();
};
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);

console.error("The Jeeves Telegram gateway is up. Say something to the bot in a direct chat.");
await adapter.run();
