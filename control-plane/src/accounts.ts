
import { createHmac, randomBytes, scryptSync, timingSafeEqual } from "node:crypto";
import { readFileSync, renameSync, writeFileSync } from "node:fs";

export interface Account {
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

interface Db {
  accounts: Account[];
  usage: Record<string, Ledger>;
}

export class Accounts {
  #db: Db = { accounts: [], usage: {} };
  file: string;
  secret: string;
  constructor(file: string, secret: string) {
    this.file = file;
    this.secret = secret;
    try {
      this.#db = JSON.parse(readFileSync(file, "utf8"));
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    }
  }

  #save(next: Db) {
    writeFileSync(`${this.file}.tmp`, JSON.stringify(next), { mode: 0o600 });
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

  register(email: string, password: string): Account {
    email = email.trim().toLowerCase();
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) throw new Error("that is not an email address");
    if (password.length < 10) throw new Error("the password needs at least 10 characters");
    if (this.#db.accounts.some((a) => a.email === email)) throw new Error("already registered");
    const salt = randomBytes(16).toString("hex");
    const account: Account = {
      id: `u-${randomBytes(6).toString("hex")}`,
      email,
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
    const account = this.#db.accounts.find((a) => a.email === email.trim().toLowerCase());
    const salt = account?.salt ?? "00";
    const hash = scryptSync(password, salt, 32).toString("hex");
    if (!account || !safeEq(hash, account.hash)) throw new Error("wrong email or password");
    return account;
  }

  touch(id: string, awake?: boolean) {
    const account = this.byId(id);
    if (!account) return;
    const updated = { ...account, lastSeen: Date.now(), awake: awake ?? account.awake };
    this.#save({ ...this.#db, accounts: this.#db.accounts.map(a => a.id === id ? updated : a) });
  }

  /** Session cookie value: `<id>.<expiry>.<mac>`. */
  session(id: string, days = 14): string {
    const body = `${id}.${Date.now() + days * 864e5}`;
    return `${body}.${this.#mac(body)}`;
  }

  fromSession(cookie: string | undefined): Account | undefined {
    const parts = (cookie ?? "").split(".");
    if (parts.length !== 3) return undefined;
    const [id, expiry, mac] = parts as [string, string, string];
    if (!safeEq(this.#mac(`${id}.${expiry}`), mac) || Number(expiry) < Date.now()) return undefined;
    return this.byId(id);
  }

  #mac(body: string) {
    return createHmac("sha256", this.secret).update(body).digest("hex");
  }

  usage(id: string, month = thisMonth()): number {
    return this.#db.usage[id]?.[month] ?? 0;
  }

  addUsage(id: string, tokens: number, month = thisMonth()) {
    const ledger = { ...this.#db.usage[id], [month]: this.usage(id, month) + tokens };
    this.#save({ ...this.#db, usage: { ...this.#db.usage, [id]: ledger } });
  }
}

export function thisMonth(now = new Date()): string {
  return now.toISOString().slice(0, 7);
}

function safeEq(a: string, b: string): boolean {
  const x = Buffer.from(a);
  const y = Buffer.from(b);
  return x.length === y.length && timingSafeEqual(x, y);
}
