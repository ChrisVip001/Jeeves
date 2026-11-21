import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { accountMessage, type AccountMailer } from "./mail.ts";

function mailbox(value: string) {
  if (!/^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+$/.test(value))
    throw new Error("invalid SMTP mailbox");
  return value;
}

/** Submit authentication messages over implicit TLS. Curl reads the credential from its environment. */
export function smtpMailer(
  origin: string,
  from: string,
  endpoint: string,
  user: string,
  password: string,
  auth?: string,
): AccountMailer {
  const url = new URL(endpoint);
  if (
    url.protocol !== "smtps:" ||
    !url.hostname ||
    url.username ||
    url.password ||
    url.search ||
    url.hash ||
    (url.pathname && url.pathname !== "/")
  )
    throw new Error("CP_SMTP_URL must be an smtps:// host and optional port");
  mailbox(from);
  if (auth && !["LOGIN", "PLAIN"].includes(auth))
    throw new Error("CP_SMTP_AUTH must be LOGIN or PLAIN");
  if (!user || user.includes(":") || !password)
    throw new Error("SMTP credentials required");
  return async (email, action, token) => {
    mailbox(email);
    const { subject, text } = accountMessage(origin, email, action, token);
    const messageId = "<" + randomUUID() + "@" + from.split("@")[1] + ">";
    const message = [
      "From: " + from,
      "To: " + email,
      "Date: " + new Date().toUTCString(),
      "Message-ID: " + messageId,
      "Subject: =?UTF-8?B?" + Buffer.from(subject).toString("base64") + "?=",
      "MIME-Version: 1.0",
      "Content-Type: text/plain; charset=UTF-8",
      "Content-Transfer-Encoding: base64",
      "",
      ...Buffer.from(text)
        .toString("base64")
        .match(/.{1,76}/g)!,
      "",
    ].join("\r\n");
    await new Promise<void>((resolve, reject) => {
      const proc = spawn(
        "curl",
        [
          "--disable",
          "--silent",
          "--max-time",
          "30",
          "--proto",
          "=smtps",
          "--url",
          url.href,
          "--variable",
          "%JEEVES_ACCOUNT_SMTP_LOGIN",
          "--expand-user",
          "{{JEEVES_ACCOUNT_SMTP_LOGIN}}",
          ...(auth ? ["--login-options", "AUTH=" + auth] : []),
          "--mail-from",
          from,
          "--mail-rcpt",
          email,
          "--upload-file",
          "-",
        ],
        {
          env: {
            PATH: process.env.PATH,
            JEEVES_ACCOUNT_SMTP_LOGIN: user + ":" + password,
          },
          stdio: ["pipe", "ignore", "ignore"],
        },
      );
      proc.once("error", reject);
      proc.stdin.on("error", reject);
      proc.once("close", (code) =>
        code === 0
          ? resolve()
          : reject(new Error("SMTP submission failed (curl " + code + ")")),
      );
      proc.stdin.end(message);
    });
    return { messageId };
  };
}
