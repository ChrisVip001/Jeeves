import { JeevesBridge } from "./bridge.ts";
import { WeChatAdapter } from "./wechat.ts";

/** The few without which nothing starts. When one is missing, say where to get it, not just the name. */
function required(name: string, hint: string): string {
  const value = process.env[name]?.trim();
  if (value === undefined || value === "") {
    console.error(`${name} was not given. ${hint}`);
    process.exit(1);
  }
  return value;
}

const token = required(
  "WECHAT_TOKEN",
  "The Token you filled in yourself under Settings & development → Basic configuration → "
  + "Server configuration.",
);
const appId = required(
  "WECHAT_APP_ID",
  "The account's AppID — the string starting with wx, right there on the same page.",
);
const appSecret = process.env["WECHAT_APP_SECRET"]?.trim() || undefined;
const aesKey = process.env["WECHAT_AES_KEY"]?.trim() || undefined;
const path = process.env["WECHAT_PATH"]?.trim() || "/wechat";
const port = Number(process.env["WECHAT_PORT"] ?? "8080");

const allow = (process.env["WECHAT_ALLOW"] ?? "")
  .split(",")
  .map((entry) => entry.trim())
  .filter((entry) => entry !== "");

if (allow.length === 0) {
  console.error(
    "Note: WECHAT_ALLOW is unset, so anyone who follows this account can use it and spend your\n" +
    "      model quota.\n" +
      "      To keep it to yourself, put your OpenID here (visible in the platform's user list).",
  );
}

if (appSecret === undefined) {
  console.error(
    "Note: WECHAT_APP_SECRET is unset. WeChat allows 5 seconds and then drops the user's line,\n" +
      "      and a Jeeves turn (especially with tool calls) usually takes far longer — so without it,\n" +
      "      most messages get no answer at all.\n" +
      "      Also, the customer-service API only answers for a **verified** account; on an unverified\n" +
    "      personal subscription account it stays closed.",
  );
}

if (aesKey !== undefined && aesKey.length !== 43) {
  console.error(
    `Reminder: WECHAT_AES_KEY is ${aesKey.length} characters, expected 43.` +
      "One character was probably lost or added while copying — then nothing decrypts at all.",
  );
}

console.error(
  `Reminder: WeChat wants a **publicly reachable** address, and only ports 80 and 443.\n` +
    `      A local run needs a tunnel first, then put the ${path} address into «Server configuration».`,
);

const bridge = new JeevesBridge();
const adapter = new WeChatAdapter(
  {
    token,
    appId,
    appSecret,
    encodingAesKey: aesKey,
    port,
    path,
    allow,
    onLog: (text) => console.error(`[wechat] ${text}`),
  },
  async (conversation, text) => (await bridge.ask(conversation, text)).reply,
);

const shutdown = (): void => {
  adapter.stop();
  bridge.close();
};
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);

console.error(
  `The Jeeves WeChat Official Account gateway is up, waiting for ${path} on port ${port}. Say something in the account.`,
);
await adapter.run();
