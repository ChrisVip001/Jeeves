import { DatabaseSync } from "node:sqlite";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { homedir } from "node:os";
import { createHash } from "node:crypto";

export class LarkInbox {
  readonly #db: DatabaseSync;

  static forAccount(appId: string): LarkInbox {
    const home = process.env["JEEVES_HOME"]?.trim() || join(homedir(), ".jeeves");
    const account = createHash("sha256").update(appId).digest("hex");
    return new LarkInbox(join(home, "gateway", "lark", account, "inbox.sqlite3"));
  }

  constructor(path = ":memory:") {
    if (path !== ":memory:") mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
    this.#db = new DatabaseSync(path);
    this.#db.exec(`
      PRAGMA busy_timeout=1500;
      PRAGMA synchronous=FULL;
      CREATE TABLE IF NOT EXISTS incoming (
        id TEXT PRIMARY KEY, parts INTEGER NOT NULL, done INTEGER NOT NULL DEFAULT 0
      );
      CREATE INDEX IF NOT EXISTS pending_incoming ON incoming(done,id) WHERE done=0;
      CREATE TABLE IF NOT EXISTS fragments (
        id TEXT NOT NULL, seq INTEGER NOT NULL, body BLOB NOT NULL, PRIMARY KEY(id,seq)
      );
    `);
  }

  admit(id: string, parts: number, seq: number, body: Uint8Array): void {
    this.#db.exec("BEGIN IMMEDIATE");
    try {
      this.#db.prepare("INSERT OR IGNORE INTO incoming(id,parts) VALUES(?,?)").run(id, parts);
      const row = this.#db.prepare("SELECT parts,done FROM incoming WHERE id=?").get(id)!;
      if (!row.done) {
        if (row.parts !== parts) throw new Error("inconsistent fragment count");
        this.#db.prepare("INSERT OR IGNORE INTO fragments(id,seq,body) VALUES(?,?,?)").run(id, seq, body);
      }
      this.#db.exec("COMMIT");
    } catch (error) {
      this.#db.exec("ROLLBACK");
      throw error;
    }
  }

  ready(id?: string): string[] {
    const select = "SELECT i.id FROM incoming i JOIN fragments f ON f.id=i.id WHERE i.done=0";
    const grouped = " GROUP BY i.id HAVING count(*)=i.parts ORDER BY i.rowid";
    const rows =
      id === undefined
        ? this.#db.prepare(select + grouped).all()
        : this.#db.prepare(select + " AND i.id=?" + grouped).all(id);
    return rows.map((row) => String(row.id));
  }

  body(id: string): Buffer {
    return Buffer.concat(
      this.#db
        .prepare("SELECT body FROM fragments WHERE id=? ORDER BY seq")
        .all(id)
        .map((row) => row.body as Uint8Array),
    );
  }

  finish(id: string): void {
    this.#db.exec("BEGIN IMMEDIATE");
    try {
      this.#db.prepare("UPDATE incoming SET done=1 WHERE id=?").run(id);
      this.#db.prepare("DELETE FROM fragments WHERE id=?").run(id);
      this.#db.exec("COMMIT");
    } catch (error) {
      this.#db.exec("ROLLBACK");
      throw error;
    }
  }
  close(): void {
    this.#db.close();
  }
}
