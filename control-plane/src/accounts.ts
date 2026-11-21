import {
  createCipheriv,
  createHash,
  createDecipheriv,
  createHmac,
  hkdfSync,
  randomBytes,
  scryptSync,
  timingSafeEqual,
} from "node:crypto";
import { readFileSync, renameSync, writeFileSync } from "node:fs";

import { acceptedStep, newSecret } from "./factors.ts";
import type { MailAction } from "./mail.ts";

export interface Account {
  verified?: boolean;
  factor?: { secret: string; last: number; backups: string[] };
  enrollment?: { session: string; secret: string; expires: number };
  id: string;
  email: string;
  salt: string;
  hash: string;
  opsToken: string;
  modelToken: string;
  /** Own model key, when the user brings one: the Pod uses it, the platform quota does not. */
  createdAt: string;
  lastSeen: number;
  awake: boolean;
  /** Paid 24h mode: schedules and chat channels keep running, so the idle sweep skips it. */
  alwaysOn?: boolean;
}

export interface Ledger {
  /** `YYYY-MM` → tokens used through the model proxy. */
  [month: string]: number;
}

export interface LoginSession {
  id: string;
  accountId: string;
  client: string;
  issued: number;
  expires: number;
}

interface StoredSession extends LoginSession {
  digest: string;
}

interface Db {
  sessions: StoredSession[];
  accounts: Account[];
  usage: Record<string, Ledger>;
}

export class Accounts {
  #db: Db = { accounts: [], usage: {}, sessions: [] };
  file: string;
  #sessionKey: string;
  #stateKey: Buffer;
  constructor(
    file: string,
    secret: string,
    stateSecret = secret,
    migrateLegacy = false,
  ) {
    this.file = file;
    this.#sessionKey = secret;
    this.#stateKey = Buffer.from(
      hkdfSync("sha256", stateSecret, "", "jeeves/account-state/v1", 32),
    );
    try {
      const record = JSON.parse(readFileSync(file, "utf8"));
      if (record.format === "jeeves-accounts-v1") {
        const cipher = createDecipheriv(
          "aes-256-gcm",
          this.#stateKey,
          Buffer.from(record.iv, "base64"),
        );
        cipher.setAuthTag(Buffer.from(record.tag, "base64"));
        this.#db = JSON.parse(
          Buffer.concat([
            cipher.update(Buffer.from(record.data, "base64")),
            cipher.final(),
          ]).toString(),
        );
      } else if (
        migrateLegacy &&
        Array.isArray(record.accounts) &&
        record.usage
      ) {
        this.#save({
          accounts: record.accounts,
          usage: record.usage,
          sessions: [],
        });
      } else {
        throw new Error(
          "Unsupported account store. For a legacy store, stop the server and run --migrate-accounts.",
        );
      }
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT" || migrateLegacy)
        throw error;
    }
  }

  #save(next: Db) {
    const iv = randomBytes(12);
    const cipher = createCipheriv("aes-256-gcm", this.#stateKey, iv);
    const data = Buffer.concat([
      cipher.update(JSON.stringify(next), "utf8"),
      cipher.final(),
    ]);
    const record = {
      format: "jeeves-accounts-v1",
      iv: iv.toString("base64"),
      tag: cipher.getAuthTag().toString("base64"),
      data: data.toString("base64"),
    };
    writeFileSync(`${this.file}.tmp`, JSON.stringify(record), { mode: 0o600 });
    renameSync(`${this.file}.tmp`, this.file);
    this.#db = next;
  }

  all(): Account[] {
    return this.#db.accounts;
  }

  byId(id: string) {
    return this.#db.accounts.find((a) => a.id === id);
  }

  byModelToken(token: string) {
    return this.#db.accounts.find((a) => safeEq(a.modelToken, token));
  }

  register(email: string, password: string, verified = false): Account {
    email = email.trim().toLowerCase();
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email))
      throw new Error("that is not an email address");
    if (password.length < 10)
      throw new Error("the password needs at least 10 characters");
    if (this.#db.accounts.some((a) => a.email === email))
      throw new Error("already registered");
    const salt = randomBytes(16).toString("hex");
    const account: Account = {
      id: `u-${randomBytes(6).toString("hex")}`,
      email,
      verified,
      salt,
      hash: scryptSync(password, salt, 32).toString("hex"),
      opsToken: randomBytes(24).toString("hex"),
      modelToken: `jeeves-${randomBytes(24).toString("hex")}`,
      createdAt: new Date().toISOString(),
      lastSeen: Date.now(),
      awake: false,
    };
    this.#save({ ...this.#db, accounts: [...this.#db.accounts, account] });
    return account;
  }

  login(email: string, password: string): Account {
    const account = this.#db.accounts.find(
      (a) => a.email === email.trim().toLowerCase(),
    );
    const salt = account?.salt ?? "00";
    const hash = scryptSync(password, salt, 32).toString("hex");
    if (!account || !safeEq(hash, account.hash))
      throw new Error("wrong email or password");
    return account;
  }

  changePassword(id: string, current: string, replacement: string) {
    const owner = this.byId(id);
    if (!owner) throw new Error("unknown account");
    this.login(owner.email, current);
    this.replacePassword(owner, replacement);
  }

  replacePassword(owner: Account, replacement: string) {
    const id = owner.id;
    if (replacement.length < 10)
      throw new Error("the password needs at least 10 characters");
    const salt = randomBytes(16).toString("hex");
    const updated = {
      ...owner,
      enrollment: undefined,
      salt,
      hash: scryptSync(replacement, salt, 32).toString("hex"),
    };
    this.#save({
      ...this.#db,
      accounts: this.#db.accounts.map((a) => (a.id === id ? updated : a)),
      sessions: this.#db.sessions.filter((s) => s.accountId !== id),
    });
  }

  byEmail(email: string) {
    return this.#db.accounts.find(
      (a) => a.email === email.trim().toLowerCase(),
    );
  }

  mailToken(email: string, action: MailAction) {
    const expires = Date.now() + 15 * 60_000;
    return expires + "." + this.mailSignature(email, action, expires);
  }

  private mailSignature(email: string, action: MailAction, expires: number) {
    const account = this.byEmail(email);
    return this.#digest(
      JSON.stringify([
        "mail",
        email,
        action,
        expires,
        account?.id,
        account?.hash,
        !!account?.verified,
      ]),
    );
  }

  validMail(email: string, action: MailAction, token: string) {
    const [time, signature] = token.split(".");
    const expires = Number(time);
    return (
      /^\d{13}\.[0-9a-f]{64}$/.test(token) &&
      expires > Date.now() &&
      safeEq(this.mailSignature(email, action, expires), signature!)
    );
  }

  private updateAccount(updated: Account, keepSession?: string | null) {
    this.#save({
      ...this.#db,
      accounts: this.#db.accounts.map((a) =>
        a.id === updated.id ? updated : a,
      ),
      sessions:
        keepSession === undefined
          ? this.#db.sessions
          : this.#db.sessions.filter(
              (s) => s.accountId !== updated.id || s.id === keepSession,
            ),
    });
  }

  verifyEmail(account: Account) {
    this.updateAccount({ ...account, verified: true }, null);
  }

  beginFactor(account: Account, session: string) {
    if (account.factor) return;
    const secret = newSecret();
    this.updateAccount({
      ...account,
      enrollment: { session, secret, expires: Date.now() + 600_000 },
    });
    return secret;
  }

  enableFactor(account: Account, session: string, code: string) {
    const pending = account.enrollment;
    if (
      account.factor ||
      !pending ||
      pending.session !== session ||
      pending.expires <= Date.now()
    )
      return;
    const last = acceptedStep(pending.secret, code, -1);
    if (last === undefined) return;
    const codes = Array.from({ length: 8 }, () =>
      randomBytes(10).toString("hex"),
    );
    this.updateAccount(
      {
        ...account,
        enrollment: undefined,
        factor: {
          secret: pending.secret,
          last,
          backups: codes.map(backupDigest),
        },
      },
      session,
    );
    return codes;
  }

  useFactor(account: Account, code: string, disableForSession?: string) {
    const factor = account.factor;
    if (!factor) return false;
    const last = acceptedStep(factor.secret, code, factor.last);
    const backup = backupDigest(code);
    if (last === undefined && !factor.backups.some((b) => safeEq(b, backup)))
      return false;
    this.updateAccount(
      {
        ...account,
        factor: disableForSession
          ? undefined
          : {
              ...factor,
              last: last ?? factor.last,
              backups: factor.backups.filter((b) => b !== backup),
            },
      },
      disableForSession,
    );
    return true;
  }

  touch(id: string, awake?: boolean) {
    const account = this.byId(id);
    if (!account) return;
    const updated = {
      ...account,
      lastSeen: Date.now(),
      awake: awake ?? account.awake,
    };
    this.#save({
      ...this.#db,
      accounts: this.#db.accounts.map((a) => (a.id === id ? updated : a)),
    });
  }

  session(id: string, days = 14, client = "Web"): string {
    if (!this.byId(id)) throw new Error("unknown account");
    const issued = Date.now();
    const token = randomBytes(32).toString("base64url");
    const session = {
      id: randomBytes(16).toString("hex"),
      accountId: id,
      issued,
      expires: issued + days * 864e5,
      digest: this.#digest(token),
      client,
    };
    this.#save({
      ...this.#db,
      sessions: [
        ...this.#db.sessions.filter((s) => s.expires > issued),
        session,
      ],
    });
    return token;
  }

  #digest(token: string) {
    return createHmac("sha256", this.#sessionKey).update(token).digest("hex");
  }

  authenticate(token?: string): LoginSession | undefined {
    if (!token || !/^[\w-]{43}$/.test(token)) return;
    const digest = this.#digest(token);
    return this.#db.sessions.find(
      (s) => s.expires > Date.now() && safeEq(s.digest, digest),
    );
  }

  fromSession(cookie?: string): Account | undefined {
    const session = this.authenticate(cookie);
    return session ? this.byId(session.accountId) : undefined;
  }

  sessions(id: string) {
    return this.#db.sessions
      .filter((s) => s.accountId === id && s.expires > Date.now())
      .map(({ id, client, issued, expires }) => ({
        id,
        client,
        issued,
        expires,
      }));
  }

  revoke(accountId: string, id: string) {
    const sessions = this.#db.sessions.filter(
      (s) => s.accountId !== accountId || s.id !== id,
    );
    if (sessions.length === this.#db.sessions.length) return false;
    this.#save({ ...this.#db, sessions });
    return true;
  }

  usage(id: string, month = thisMonth()): number {
    return this.#db.usage[id]?.[month] ?? 0;
  }

  addUsage(id: string, tokens: number, month = thisMonth()) {
    const ledger = {
      ...this.#db.usage[id],
      [month]: this.usage(id, month) + tokens,
    };
    this.#save({ ...this.#db, usage: { ...this.#db.usage, [id]: ledger } });
  }
}

export function thisMonth(now = new Date()): string {
  return now.toISOString().slice(0, 7);
}

function backupDigest(code: string) {
  return createHash("sha256").update(code).digest("hex");
}

function safeEq(a: string, b: string): boolean {
  const x = Buffer.from(a);
  const y = Buffer.from(b);
  return x.length === y.length && timingSafeEqual(x, y);
}
