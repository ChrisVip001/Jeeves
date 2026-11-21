import type { IncomingMessage, ServerResponse } from "node:http";
import type { Accounts } from "./accounts.ts";
import type { AccountMailer } from "./mail.ts";
import {
  accountFields,
  clientLabel,
  json,
  sessionCookie,
  tokenFrom,
} from "./web.ts";

interface Options {
  mail?: AccountMailer;
  single?: { ownerId: string; registrationEmail?: string };
  allow: (key: string, limit: number) => boolean;
  close: (sessions: { id: string }[]) => void;
  provision: (id: string) => Promise<void>;
}

export async function identity(
  req: IncomingMessage,
  res: ServerResponse,
  url: string,
  accounts: Accounts,
  cfg: Options,
) {
  if (!cfg.allow("auth-source:" + req.socket.remoteAddress, 240))
    return json(res, 429, { error: "try again later" });
  const input = await accountFields(req).catch(() => null);
  if (!input) return json(res, 400, { error: "invalid request" });
  const session = accounts.authenticate(tokenFrom(req));
  const password = typeof input.password === "string" ? input.password : "";
  const code = typeof input.code === "string" ? input.code.trim() : "";
  if (url.startsWith("/api/account/factor/")) {
    const me = session && accounts.byId(session.accountId);
    if (
      !session ||
      !me?.verified ||
      (cfg.single && me.id !== cfg.single.ownerId)
    )
      return json(res, 401, { error: "sign in first" });
    if (!cfg.allow("factor:" + me.id, 10))
      return json(res, 429, { error: "try again later" });
    try {
      accounts.login(me.email, password);
    } catch {
      return json(res, 403, { error: "incorrect password" });
    }
    if (url === "/api/account/factor/start") {
      const secret = accounts.beginFactor(me, session.id);
      return secret
        ? json(res, 200, { secret })
        : json(res, 409, { error: "already enabled" });
    }
    const other = accounts.sessions(me.id).filter((s) => s.id !== session.id);
    if (url === "/api/account/factor/confirm") {
      const codes = accounts.enableFactor(me, session.id, code);
      if (codes) {
        cfg.close(other);
        return json(res, 200, { codes });
      }
    } else if (
      url === "/api/account/factor/disable" &&
      accounts.useFactor(me, code, session.id)
    ) {
      cfg.close(other);
      return json(res, 200, { ok: true });
    }
    return json(res, 403, { error: "invalid factor" });
  }
  const email =
    typeof input.email === "string" ? input.email.trim().toLowerCase() : "";
  if (email.length > 254 || !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email))
    return json(res, 400, { error: "invalid email" });
  const existing = accounts.byEmail(email);
  const signupOpen =
    !cfg.single || Boolean(cfg.single.registrationEmail && !cfg.single.ownerId);
  const ownerAllowed =
    !cfg.single ||
    existing?.id === cfg.single.ownerId ||
    (!existing && signupOpen && email === cfg.single.registrationEmail);
  if (
    url === "/api/register" ||
    url === "/api/forgot" ||
    url === "/api/send-verification"
  ) {
    if (url === "/api/register" && !signupOpen)
      return json(res, 403, { error: "registration closed" });
    const action = url === "/api/forgot" ? "reset" : "verify";
    if (!cfg.allow("send:" + email + ":" + action, 3))
      return json(res, 429, { error: "try again later" });
    if (!cfg.mail) return json(res, 503, { error: "email unavailable" });
    if (
      ownerAllowed &&
      (action === "reset"
        ? existing?.verified
        : (!existing && signupOpen && url === "/api/register") ||
          (existing && !existing.verified))
    ) {
      void cfg
        .mail(email, action, accounts.mailToken(email, action))
        .catch(() =>
          console.error(
            "(control-plane) account email delivery failed; check mail provider",
          ),
        );
    }
    return json(res, 202, { step: "mail" });
  }
  if (!cfg.allow("credentials:" + email, 15))
    return json(res, 429, { error: "try again later" });
  const token = typeof input.token === "string" ? input.token : "";
  if (url === "/api/reset") {
    if (
      !ownerAllowed ||
      !existing?.verified ||
      password.length < 10 ||
      !accounts.validMail(email, "reset", token)
    )
      return json(res, 400, { error: "invalid reset" });
    const sessions = accounts.sessions(existing.id);
    accounts.replacePassword(existing, password);
    cfg.close(sessions);
    return json(res, 200, { ok: true });
  }
  let account;
  let created = false;
  if (url === "/api/verify") {
    if (
      !ownerAllowed ||
      existing?.verified ||
      password.length < 10 ||
      !accounts.validMail(email, "verify", token)
    )
      return json(res, 400, { error: "invalid verification" });
    if (!existing) {
      account = accounts.register(email, password, true);
      created = true;
    } else {
      try {
        account = accounts.login(email, password);
      } catch {
        return json(res, 401, { error: "incorrect credentials" });
      }
      const sessions = accounts.sessions(existing.id);
      accounts.verifyEmail(account);
      cfg.close(sessions);
    }
  } else {
    try {
      account = accounts.login(email, password);
    } catch {
      return json(res, 401, { error: "incorrect credentials" });
    }
    if (!ownerAllowed) return json(res, 403, { error: "account unavailable" });
    if (!account.verified) return json(res, 200, { step: "verify" });
  }
  if (account.factor) {
    if (!code) return json(res, 200, { step: "factor" });
    if (!accounts.useFactor(account, code))
      return json(res, 403, { error: "invalid factor" });
  }
  if (session && accounts.revoke(session.accountId, session.id))
    cfg.close([session]);
  const cookie = sessionCookie(
    accounts.session(account.id, 14, clientLabel(req)),
    14 * 86400,
  );
  if (created) await cfg.provision(account.id);
  return json(res, 200, { id: account.id }, { "set-cookie": cookie });
}
