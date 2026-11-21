import { JeevesBridge } from "./bridge.ts";
import { LarkAdapter } from "./lark.ts";
import { taskReporter } from "./reports.ts";
import { approvalRelay } from "./approvals.ts";
import { existsSync } from "node:fs";
import { loadEnvFile } from "node:process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../../../", import.meta.url));
const envFile = fileURLToPath(new URL("../../../../.env", import.meta.url));
if (existsSync(envFile)) loadEnvFile(envFile);

const appId = process.env["LARK_APP_ID"]?.trim();
const appSecret = process.env["LARK_APP_SECRET"]?.trim();
if (appId === undefined || appId === "" || appSecret === undefined || appSecret === "") {
  console.error(
    "LARK_APP_ID / LARK_APP_SECRET were not given.\n" +
      "Make a self-built app at https://open.feishu.cn/app → credentials, then:\n" +
      "  LARK_APP_ID=cli_… LARK_APP_SECRET=… npm run gateway:lark",
  );
  process.exit(1);
}

const allow = (process.env["LARK_ALLOW"] ?? "")
  .split(",")
  .map((entry) => entry.trim())
  .filter((entry) => entry !== "");

const claim = process.env["LARK_CLAIM"] === "1";
if (allow.length === 0 && !claim) {
  console.error(
    "Note: LARK_ALLOW is unset, so anyone can use this bot and spend your model quota.\n" +
      "      To keep it to yourself, put your open_id (starts with ou_…).",
  );
}

console.error(
  "Reminder: for this app to receive messages, three things have to be done in the developer\n" +
    "      console; miss one and it is «connected but never reacts»:\n" +
    "      1. Events & callbacks → subscribe over «long connection» (not a webhook);\n" +
    "      2. add the event `im.message.receive_v1` (receive messages);\n" +
    "      3. add the permissions `im:message` (read) and `im:message:send_as_bot` (send as the app),\n" +
    "      then release a version.\n" +
    "      For approval cards, also add the callback `card.action.trigger` (long connection).",
);

const dispatchers = (process.env["LARK_DISPATCHERS"] ?? process.env["JEEVES_DISPATCHERS"] ?? "")
  .split(",")
  .map((entry) => entry.trim())
  .filter((entry) => entry !== "");

const runtimeUrl = process.env["JEEVES_GATEWAY_URL"]?.trim();
const bridge = new JeevesBridge({
  cwd: root,
  ...(runtimeUrl ? { url: runtimeUrl, token: process.env["JEEVES_OPS_TOKEN"] } : {}),
  env: { ...process.env, JEEVES_DISPATCHERS: dispatchers.join(",") },
});
const adapter = new LarkAdapter(
  {
    appId,
    appSecret,
    allow,
    claim,
    dispatchers,
    lark: process.env["LARK_DOMAIN"]?.trim().toLowerCase() === "lark",
    onLog: (text) => console.error(`[lark] ${text}`),
    onWake: (params) => bridge.wake(params as Parameters<typeof bridge.wake>[0]),
    onApproval: (action, actor) =>
      bridge.answerApprovalDetailed(action.turnId, action.requestId, action.approved, actor, action.version),
  },
  async (conversation, text, source, requestId, textApprovalSource) =>
    (await bridge.ask(conversation, text, source, requestId, textApprovalSource)).reply,
);

bridge.onTaskUpdated(
  taskReporter({
    task: (taskId) => bridge.task(taskId),
    send: (chatId, messageId, text) => adapter.report(chatId, messageId, text),
    onLog: (text) => console.error(`[lark] ${text}`),
  }),
);

bridge.onApprovalRequested(
  approvalRelay({
    locate: (turnId, requestId) => bridge.locateApproval(turnId, requestId),
    sendCard: (chatId, messageId, card) => adapter.sendCard(chatId, messageId, card),
    sendTextApproval: (source, request) => adapter.requestTextApproval(source, request),
    onLog: (text) => console.error(`[lark] ${text}`),
  }),
);

const shutdown = (): void => {
  adapter.stop();
  bridge.close();
};
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);

console.error(
  "The Jeeves gateway is up. Say something in a direct chat, or @ it in a group — it will not answer "
  + "in a group without an @.",
);
console.error("Text approvals are available to the sole allowlisted owner in the originating conversation.");
await adapter.run();
