# ② Storage: `jeeves-state`

English | [简体中文](02-state.zh.md)

Everything Jeeves remembers lives in one SQLite file, `<home>/state.db`. This chapter covers how it is opened, how the schema is organized, and the query contracts. When to write is decided by the server and memory chapters ([05-memory](05-memory.md), [06-app-server](06-app-server.md)).

The app-server runtime owns database access for clients. Other embedded runtimes (CLI, scheduler, MCP server) can still open the same file, so WAL and a connection-level lock wait are required. Execution recovery is guarded by a separate OS ownership lock in app-server (`lock.rs`), not by this crate. Cross-process coordination of conversation turns is not provided; see [limitations](../limitations.md).

## 2.1 Opening the database

`db::open` creates the parent directory, resolves the path to an absolute one, creates the file if needed, connects and runs migrations.

- **Absolute path.** sqlx resolves a relative path against the process cwd, and the cwd must not decide where the state lives; starting from another directory would silently create an empty database.
- **Connection options, not `PRAGMA`.** `busy_timeout` (10 s) and `journal_mode = WAL` are set on `SqliteConnectOptions`. `PRAGMA busy_timeout` is per connection and would only reach whichever pooled connection ran it. The pool has 8 connections. `the_pool_waits_for_locks_and_uses_wal` pins this.
- **Migrations are embedded** (`sqlx::migrate!`), so a binary carries its schema and needs no files at runtime.
- `snapshot` writes a consistent copy with `VACUUM INTO` and returns its schema version; `schema_version` and `known_schema_version` compare a database with the binary.

## 2.2 Schema rules

Schema changes exist only as numbered files in `migrations/`; current structure is whatever the files produce, read them rather than a list here. Rules:

- Append a new migration; never edit an applied one.
- Data access goes through the typed handles exported from `lib.rs` (`Facts`, `Chat`, `Ledger`, `Tasks`, …); no other crate writes SQL against these tables.
- Idempotency belongs in SQL (unique indexes, `INSERT OR IGNORE`, conditional `UPDATE … RETURNING`) rather than check-then-write in Rust.

Table families:

| Family | Role |
|---|---|
| `facts`, `episodes` (+ `*_fts`), `memory_*`, `active_facts` / `durable_facts` views | Memory index. Plain-text pages under `<home>/memory` are the authority; these rows index them |
| `chat_log`, `sessions`, `context_rollups`, `session_*`, `session_queue` | Conversation view, session metadata, rolling summary, queued messages, per-session permissions |
| `ledger`, `ledger_turns` | Append-only record of every turn |
| `agent_tasks`, `task_*`, `task_runs`, `run_*`, `turn_checkpoints`, `jobs`, `job_*` | Long-running tasks, runs, resumable turn state, background jobs |
| `tool_operations`, `approval_log`, `interaction_receipts`, `browser_sessions` | Operation, approval and interaction records |
| `schedule_jobs`, `schedule_state`, `schedule_completions` | Schedule declarations and run state |
| `model_requests`, `historical_turn_usage` | Per-request measurements and historical per-turn totals |

### Facts and episodes

- `facts_fts` and `episodes_fts` are external-content FTS5 tables (`content='facts'`), kept in sync by insert/delete/update triggers; they hold only the index.
- The tokenizer is `trigram`. The default `unicode61` treats a run of Chinese characters as one word and cannot match substrings. The cost is a larger index and no match for terms shorter than 3 characters, which the search tiers below cover.
- A unique index on `(subject, lower(trim(content)))` keeps duplicates out. `Facts::add` returns the existing row and `false` for a duplicate, because repeated consolidation is normal and not an error. A fact suppressed by an earlier human edit is refused by background writers (`add_consolidated`) and restored only by an explicit remember.
- Reads use the `active_facts` view, which hides facts outside their validity window; `durable_facts` additionally hides time-bounded ones from static mirrors.
- `facts.embedding` is JSON text, computed by the memory layer; rows without one still take part in keyword search. A personal store is small enough for cosine in Rust, so no vector extension is needed.

### Ledger

`chat_log` is the view (one row per speaker, tool activity folded to text). `ledger` is the record: every user message, assistant block, tool call, tool result and image, structured. Rules in `ledger.rs`:

1. No content UPDATE path; a correction is a new row. (`ledger_turns` has queue-state columns for consolidation, which are not content.)
2. Fidelity is declared: `exact` for live turns, `folded` for rows backfilled from `chat_log`, whose tool output was already truncated.
3. Large payloads go outside the row (`<home>/ledger/`, written by the caller); the row keeps pointer, digest and byte count so `verify` still works.

Entry kinds are fixed in `KINDS` and anything else is refused at write.

## 2.3 Concurrency gates

Each read-then-act site has an atomic gate in SQL:

| Site | Gate | Prevents |
|---|---|---|
| consolidation | `claim_unconsolidated`: one `UPDATE … RETURNING` marks rows first, process after | Two turns consolidating the same rows (a wasted model call) |
| consolidation failure | `mark_consolidation_failed` returns the claim and schedules a retry with exponential backoff (1 min doubling, capped at 1 h) | Marked-as-done with nothing written; or retrying a bad batch every turn |
| `save_rollup` | `WHERE excluded.covered_turns > context_rollups.covered_turns` | An older summary overwriting a newer one in concurrent compaction |
| `session_queue` | partial unique index allowing one `claimed` row per session | Two workers claiming the same session |
| `Facts::add` | unique index (above) | Duplicate facts |

`context_rollups.covered_turns` is a watermark that only moves forward: each compaction folds the turns newly pushed out of the window into the stored summary instead of recomputing history.

## 2.4 Search

Contract, shared by every method: a miss is never an error. Nothing found gives an empty `Vec`, a missing id gives `false`, nothing deleted gives `0`. The caller is a model and needs a plain "none".

`Facts::search` and `Episodes::search` choose a tier by query:

1. **CJK query** (`fts::cjk_terms`): cut each CJK run into overlapping 2-character terms (other words kept whole, at most 24), fetch rows containing any term by `LIKE`, rank by distinct terms matched, newest first on ties. A Chinese question reaches FTS as one long word that no stored sentence contains, and trigrams of it rarely align, so FTS is skipped.
2. **FTS5** (`fts::to_match_expr`): split on non-alphanumerics, quote each term, join with `OR`. Quoting makes the expression always valid, and the split removes characters like `:` that break FTS5 grammar. Ordered by `bm25`.
3. **LIKE fallback** (`fts::like_pattern`): used when FTS returns nothing, for terms too short to trigram. `%` and `_` are stripped so input cannot become a wildcard scan; a query with no alphanumeric character returns empty.

A personal store has thousands of rows, so the scans in tiers 1 and 3 cost microseconds.

## 2.5 Backend contract

`SemanticStore` and `EpisodicStore` (`store.rs`) are the seam for alternative memory backends. `conformance.rs` holds the acceptance suite every backend must pass, and the built-in SQLite implementation runs it too (`conformance_tests.rs`). It pins the miss semantics above and that trait calls and inherent calls on `Facts` agree.

## 2.6 Tests

- `state_tests.rs` and the per-area `*_tests.rs` cover the handles.
- Tests that create a temporary database call `dir.keep()` rather than dropping the `TempDir`: SQLite still writes `-wal` / `-shm` files, and a vanished directory fails later writes.

## 2.7 Invariants

- The schema is produced only by migrations.
- Tables are accessed only through the typed handles.
- A miss is not an error.
- The ledger is append-only for content.
