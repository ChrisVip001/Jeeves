# Known limits and where to pick them up

English | [简体中文](limitations.zh.md)

Everything here is a **known** boundary: a deliberate design decision or work nobody has
built. Each entry says what the boundary is, why it exists, and which file to open first to
change it.

Reading convention: **Deliberate** means the behaviour is intended and a change is a design
change that belongs in a discussion first. **Gap** means we would like it and nobody has built
it. **Boundary** means a guarantee stops here.

## Credential vault covers commands only

The vault hides a value from `run_command` output by exact text match. A command that
transforms the value (base64, splitting it) before printing defeats the scrub, and a secret
sent to a remote server is out of Jeeves's hands. It narrows what the model can read; it is not a
sandbox for hostile commands.
→ `jeeves-rs/jeeves-config/src/vault.rs`, `jeeves-rs/jeeves-ops/src/api/vault.rs`

## Models and keys

* **Deliberate** — Models are configured only through `model/*` (REPL `/model`, dashboard
  panel). `JEEVES_API_KEY` / `JEEVES_BASE_URL` / `JEEVES_MODEL` / `JEEVES_SMALL_MODEL` are not read and
  `config/write` accepts no model fields. `JEEVES_PROVIDER` only names the fallback provider when
  nothing is configured.
* **Deliberate** — Keys live in the OS keychain when one works, otherwise in
  `<home>/secrets.json` (plaintext, mode 0600 on Unix). A keychain call that fails or exceeds
  5 seconds counts as failure and falls back to the file, so one credential operation can never
  freeze the server. `model/save` reports the real landing spot (`keychain` / `file`).
  → `jeeves-rs/jeeves-config/src/models.rs` (`SecretStore`, `KEYCHAIN_TIMEOUT_SECS`)
* **Deliberate** — Switching models rewrites in-process settings and re-resolves the provider;
  the configuration lives in `<home>/models.json`, so the next start keeps your last pick. The
  model cannot switch models: `model/*` is human-only and the model never sees a key.
* **Deliberate** — The model catalog (`<home>/models-catalog.json`) is read once at boot; a
  new entry needs a restart. Broken rows cost one stderr line each and are skipped; a broken
  file falls back to the built-in table.
  → `jeeves-rs/jeeves-provider/src/catalog.rs`

## Notifications and sync

* **Boundary** — With `JEEVES_NOTIFY` on, job, schedule and goal outcomes are committed under
  `<home>/outbox/notifications/` before delivery. Failures retry every 30 seconds and on
  restart, at most 32 receipts per sweep. Delivery is at least once: a crash after a remote
  acknowledgement can duplicate. Pending receipts stay bound to the configured destination;
  changing the webhook retains old receipts instead of forwarding them. Approval prompts are
  best-effort and never replayed. Slack-compatible `{"text": …}` endpoints are supported;
  DingTalk/WeCom channels are deferred (human decision 2026-11-18), so their signed
  formats are not implemented. The outbox records transport acknowledgement, not user
  readership.
  → `jeeves-rs/jeeves-tools/src/notify_queue.rs`
* **Deliberate** — Cross-device sync is a knock, not a feed: `/api/events` pings say
  "something happened" and the client re-pulls what it shows. Task events are the exception: `task/events` reads them from a cursor. There is no message-level delta
  protocol for chat, no offline chat queue or general-purpose chat push. Paired devices can receive [task approval notifications](protocol.md#task-approval-notifications).

## Runtime ownership and concurrency

* **Boundary** — Every checked entry point (app-server, REPL, single-turn CLI) takes a
  kernel-backed advisory lock on `<database>.runtime.lock`, keyed to the canonical database
  path, before opening and recovering the database. A competing command fails with a visible
  error: use the active runtime, stop it, or choose a separate `JEEVES_HOME`. Do not delete the
  lock file to bypass an owner. This is local cooperative ownership, not a distributed lease or
  workspace write isolation. `Server::boot(pool, settings)` is for isolated embedding; its
  caller must own the store exclusively.
  → `jeeves-rs/jeeves-app-server/src/lock.rs`
* **Deliberate** — One turn per session: `turn/start` claims the session first and a second
  caller gets `SESSION_BUSY` naming the holder. The gate is per process. `turn/interrupt`
  holds no session and always gets in.
  → `jeeves-rs/jeeves-app-server/src/server/controls.rs` (`begin_turn`)
* **Deliberate** — A per-session write-section lock covers only a turn's write sections
  (compaction and the tail section). Model-call sections never hold it, which would stall the
  session and keep interrupt out. Locks are created lazily and never removed.
  → `jeeves-rs/jeeves-app-server/src/server/controls.rs` (`session_write_lock`)
* **Deliberate** — SQLite waits 10 seconds for a lock; concurrent writers queue instead of
  failing. If that is not enough, the answer is fewer writers, not a longer timeout.
  → `jeeves-rs/jeeves-state/src/db.rs`
* **Deliberate** — `turn/list`, `turn/steer` and the session gate see only turns in this
  process; a turn in another process is invisible to them and is not forwarded.

## Execution

`read_file` / `glob` / `grep` are on by default and bypass the execution gate: their trust
boundary is the process user's OS permissions. The model can read any file that user can
read, and `glob` / `grep` can see the layout of such directories (they skip `.git`,
`node_modules`, `target`, `dist`, `build` as noise, not as secrets). `fetch_url` does not go
through the `run_command` sandbox; its guard is the SSRF address check plus external-content
defusal. Its hand-rolled HTML converter is weaker than readability-class libraries; for
high-quality extraction use `run_command` with curl or an MCP fetching service.

* **Deliberate** — Sandboxed commands run offline by default (`JEEVES_EXEC_NETWORK=1` allows
  network); writes are confined to the working directory, temp and `JEEVES_EXEC_WRITABLE_ROOTS`.
  The Jeeves home is not writable: it holds `settings.json` (the exec allow table) and
  `hooks.json`, so a shell that could write it could grant itself more. Entries that are the
  home, an ancestor of it, or one of `HOME_CONTROL` are dropped even when listed.
  → `jeeves-rs/jeeves-tools/src/roots.rs`, `exec.rs` (`sandbox_argv`)
* **Deliberate** — Without a working sandbox nothing runs. On Linux `bwrap` must exist and be
  able to create a namespace (Ubuntu 24.04 restricts unprivileged user namespaces), so
  availability is probed by running it once per process.
  → `jeeves-rs/jeeves-tools/src/exec.rs` (`sandbox_backend`)
* **Deliberate** — The write tools (`write_file` / `edit`, `JEEVES_WRITE=1`) have no process
  sandbox; the hard gate is the writable roots, resolved from the selected workspace and the applicable extra roots.
  With no folder chosen nothing is writable. Outside them is refused before approval is
  consulted.
  → `jeeves-rs/jeeves-tools/src/roots.rs`, `fs_write.rs`
* **Deliberate** — `JEEVES_WRITE_ALLOW_DIRS` is a "don't ask again" list, not a writable root.
  → `jeeves-rs/jeeves-tools/src/fs_write.rs` (`pass_write_gate`)
* **Deliberate** — The hard deny list is substring matching plus a hand-written
  pipe-into-shell check, so obfuscated variants (`bash -c "$(printf …)"`) slip past. It is a
  seatbelt; the allowlist and the sandbox are what hold.
  → `jeeves-rs/jeeves-tools/src/exec.rs` (`HARD_DENY`, `pipes_into_a_shell`)
* **Deliberate** — `JEEVES_APPROVAL` is `never` (default) or `on-request`, per mode rather than
  per tool class: only exec and writes have a gate to ask at.
  → `jeeves-rs/jeeves-config/src/lib.rs`
* **Deliberate** — "Remember this" has three scopes: `once`, `session` (in-process, gone on
  restart) and `always` (written to `settings.json`). What is recorded is exactly what was
  approved: the command line without wildcards, or a write's directory. Hard-deny, the sandbox
  and the writable boundary still run first.
  → `jeeves-rs/jeeves-app-server/src/approval.rs`
* **Deliberate** — A question waiting for a human survives a restart: it comes back from
  `approval/pending` with `restored: true` and a higher `version`, and an answer carrying an older
  version is refused. A yes to such a question is a single-use, in-memory yes that the resumed turn
  spends when it asks again; it is not stored, so it is lost if the process restarts again first.
  → `jeeves-rs/jeeves-app-server/src/approval.rs`, `jeeves-rs/jeeves-state/src/grants.rs`
* **Partial** — Email and Feishu scoped send approvals record a grant; a once grant is recorded as already spent. General command and directory "remember" still writes `settings.json`, not `action_grants`.
  → `jeeves-rs/jeeves-state/src/grants.rs`
* **Partial** — MCP servers are reconnected between turns when `mcp.json` (or `JEEVES_MCP_CONFIG`) changes,
  and each rebuild logs the tools added and removed; a turn in flight keeps its table. Servers advertising `tools.listChanged` have their list queried at the next turn boundary (no idle notification listener). Connector and MCP tools are refreshed for new turns and new subagents. `JEEVES_EXEC_ALLOW` and Feishu's background token renewer configuration still need a restart.
  → `jeeves-rs/jeeves-app-server/src/server/mcp_live.rs`
* **Deliberate** — Spilled command output is cleaned by age only (`spill/` keeps 7 days,
  pruned at startup), not by a size quota. Nothing else in `JEEVES_HOME` is rotated.
  → `jeeves-rs/jeeves-tools/src/exec.rs` (`prune_spill`)
* **Deliberate** — Background commands go through the same gate as foreground ones. The
  in-process output ring is 64 KiB (overflow drops the oldest bytes and says how many); output
  also streams to `<home>/jobs/` with a 1 MiB cap. After a crash, boot reconciliation marks
  still-running jobs `interrupted` and keeps their output readable; they are never auto-rerun.
  There is no PTY, so interactive programs do not belong in the background.
  → `jeeves-rs/jeeves-app-server/src/jobs.rs`, `jobs_buffer.rs`
* **Deliberate** — MCP reverse requests (elicitation/sampling) get an explicit "not supported"
  error. One MCP connection carries one in-flight request at a time; different servers run in
  parallel. The circuit breaker is in-process: 3 consecutive failures open it for 60 seconds,
  with no persistence and no knob.
  → `jeeves-rs/jeeves-mcp/src/transport.rs`, `lib.rs` (`Breaker`)
* **Gap** — `jeeves mcp serve` implements `initialize`, `tools/list` and `tools/call` only; MCP
  `resources`, `prompts`, sampling and notifications are not implemented. MCP OAuth needs a
  manual `jeeves mcp login <name>`; there is no background re-authorisation after a refresh token
  expires.
  → `jeeves-rs/jeeves-mcp/src/server.rs`, `oauth.rs`
* **Deliberate** — Model calls are retried only for 429/5xx/network, at most `JEEVES_LLM_RETRIES`
  times (default 2) inside a 30-second budget, and never by switching provider.
  → `jeeves-rs/jeeves-provider/src/retry.rs`
* **Deliberate** — The sandbox is Unix-only (macOS `sandbox-exec`, Linux `bubblewrap`). On
  native Windows the `jeeves` binary refuses to boot (exit 2) with the reasons printed; WSL2 is
  the supported route (see the matrix in [operations.md](operations.md)). CI keeps a Windows job
  for build and unit tests only.
  → `jeeves-rs/jeeves-cli/src/main.rs` (`windows_blocker`), `.github/workflows/ci.yml`

## Subagents, turns and goals

* **Deliberate** — A subagent has no approval path (where the parent would ask, the child is
  refused), is not persisted (no `subagent:*` rows in `session/list`), streams no internal
  events, cannot spawn subagents and does not get the todo list or project conventions. The
  parent sees one tool call and its conclusion.
  → `jeeves-rs/jeeves-app-server/src/subagent.rs`
* **Deliberate** — Batched subagents (`tasks`, at most 4) are read-only: their tool table
  carries no `run_command` / `write_file` / `edit` / `apply_patch`, because the one-write-at-a-time
  barrier cannot reach concurrent loops. Single delegations get the full table.
  → `jeeves-rs/jeeves-tools/src/lib.rs` (`ToolRegistry::readonly`)
* **Deliberate** — `JEEVES_MAX_ITERATIONS` (default 32) is a spend budget. The last step is a
  wrap-up with tools removed. A turn continues itself at most `JEEVES_AUTO_CONTINUE_MAX` times
  (default 2) on a chat turn or a task without a budget; a task run that has a budget continues instead for as long as the budget has room (a ceiling of 64 only guards against bugs), and when the budget is spent it stops and names the open items. Each automatic continuation writes an `auto_continue` task event and a stderr line. A turn continues itself only when it was cut off with open todo items, buying one more slice
  (`JEEVES_TURN_STEP_SLICE`). The completion gate reads facts (open todo items), never asks the
  model "are you done?".
  → `jeeves-rs/jeeves-loop/src/lib.rs`, `jeeves-rs/jeeves-app-server/src/turn/history.rs` (`unfinished_work`)
* **Deliberate** — The todo list is capped at 20 items of 4000 characters, one `in_progress`
  at a time.
  → `jeeves-rs/jeeves-tools/src/todo.rs`
* **Deliberate** — The `/goal` judge sees only the conversation (a cheap model, no tools, no
  retrieval), so its verdict can be wrong; it may only continue a round, never get anything past
  a permission check. Goals are set by humans only (`goalCondition` on the task row). After a
  terminal outcome the loop stops and waits for a human.
  → `jeeves-rs/jeeves-app-server/src/goal.rs`
* **Deliberate** — `steer` lands at an iteration boundary; a sentence added just before the
  turn ends is still written to `chat_log` (`meta.steer`). "Do this once you are done" is
  `turn/follow-up` (a durable queue that survives restart, one message at a time). After a
  restart, uncertain claims are retained as `needs_attention`, never auto-replayed. A queued
  follow-up shows up in an SSE stream only when the next turn starts.
  → `jeeves-rs/jeeves-loop/src/lib.rs` (`Steering`), `jeeves-rs/jeeves-state/src/queue.rs`
* **Deliberate** — `turn/interrupt` interrupts model calls and running tools. Before the loop starts, local memory lookup and history compaction also observe cancellation.
  → `jeeves-rs/jeeves-app-server/src/turn/`
* **Deliberate** — The report hardening (`Human:` / `System:` prefixes and control markers get
  a backslash, plus a "this is a transcription" header) is a speed bump, not a sandbox. It does
  not judge maliciousness or change any permission check.
  → `jeeves-rs/jeeves-tools/src/report.rs`

## Model routing

Three tiers only (`fast` / `chat` / `reasoning`): no cost or speed labels on profiles, no
vision-tier routing (images ride on the turn's tier, gated by the provider's `vision` flag), no
sibling-tier failover. If a routed profile fails to resolve, the turn falls back to the active
profile. The L1 classifier sees only the message text.

* **Deliberate** — A declared `tier` beats `fixed` mode. When `model-routes.json` has no entry
  for that tier it falls through to the active profile, and `TurnMeta.routeReason` says so
  instead of hiding it.
  → `jeeves-rs/jeeves-app-server/src/route.rs` (`resolved_for_turn`)

## Files and uploads

Dashboard uploads take two routes. Non-image files land in `<home>/uploads/<date>/` under a
server-generated name (extension whitelist-checked) and their absolute path is injected into
the message; the model reads them with `read_file` if it decides to. Images ride as real
image blocks, gated up front on the provider's `vision` capability, checked for containment
inside `<home>`, existence, media type, 20 MiB per image and a 4096-pixel per-side cap read
straight from the file header.

* **Gap** — Attachments are not replayed on session restore: `chat_log` stores the message text,
  so a restored session describes the image instead of re-showing it.
  → `jeeves-rs/jeeves-app-server/src/turn/attachments.rs`, `history.rs`
* **Deliberate** — Images over 4096 px on a side are refused rather than downscaled; sending
  them in pieces is the right move, and auto-slicing would need an image crate.
  → `jeeves-rs/jeeves-app-server/src/image_meta.rs` (`MAX_EDGE`)
* **Deliberate** — `vision` follows the vendor's catalog row by default and errs toward
  `false`. A declaration on the profile (`--vision` on `/model add`, or `"vision": true` in
  `models.json`) wins.
  → `jeeves-rs/jeeves-config/src/models.rs`, `jeeves-rs/jeeves-provider/src/lib.rs`

## Memory

* **Boundary** — Human deletion and correction atomically store suppression digests.
  Background consolidation skips the same normalized subject and content; forgetting a whole
  subject blocks automatic extraction for it, including paraphrases. Explicit user memory
  writes remain allowed. Individual-claim suppression cannot recognize arbitrary paraphrases;
  this is not erasure of original conversations, backups or human-edited Markdown pages, and a
  digest is not an anonymity guarantee. Facts deleted before the suppression table existed have
  no markers.
* **Deliberate** — Exact duplicates never enter the table: a unique index on
  `(subject, lower(trim(content)))` makes `add` return the existing row. Near-duplicates (the
  same fact reworded) land as a second row; merging them is not done.
  → `jeeves-rs/jeeves-state/src/facts.rs`, `migrations/0008_memory_dedup.sql`
* **Deliberate** — Memory kinds are a closed set of five (`fact` is the fallback); an invented
  category lands in `fact`.
  → `jeeves-rs/jeeves-state/src/facts.rs` (`KINDS`)
* **Deliberate** — Temporary-statement filtering is a marker list, not a classifier, and will
  miss unusual phrasing.
  → `jeeves-rs/jeeves-memory/src/consolidation.rs` (`TEMPORARY_MARKERS`)
* **Deliberate** — Skill discovery is lexical: ASCII words and adjacent Han-character pairs,
  with at least two distinct overlaps. It does not understand synonyms; the skill catalog stays
  available to the model for semantic selection.
  → `jeeves-rs/jeeves-memory/src/skills.rs`
* **Deliberate** — Database-backed memory pages are checked against current durable facts at
  recall time; a missing, corrected or bounded source excludes the whole page, while raw files
  stay on disk. Validity applies to facts, not to episodes or checkpoints
  ([contract](protocol.md#memory-validity-and-derivation-context)).
* **Boundary** — Historical tool output is not recoverable for rows from `jeeves memory backfill`:
  they are marked `fidelity: folded`, and the memory center shows the folded label instead of
  pretending the bytes exist.

## Context

* **Deliberate** — Rolling summaries live in `context_rollups`; `chat_log` keeps the original
  turns, so "re-read history" shows the real dialogue. A rollup re-summarises the previous
  summary plus newly evicted turns within a 700-token output budget; model failure falls back
  to a 600-character excerpt (and the prompt says so). Details can be lost through repeated
  compression, and the summary is not a durable record of authorization or task commitments.
  → `jeeves-rs/jeeves-memory/src/compaction.rs`
* **Deliberate** — Compaction does not preserve file-operation state. What survives eviction is
  the rolling summary plus the `[tools used: …]` fold; a fold line carries a `spill/…` address
  for output over 2000 characters, so shorter output or a changed file has to be read again.
  → `jeeves-rs/jeeves-app-server/src/turn/history.rs`
* **Deliberate** — The six-section meter covers only what grows with a session. The clock,
  todo board, capability note and project conventions (`AGENTS.md` and friends, capped at 32 KB
  per turn) ride in the prompt uncounted, so in a repository with a fat `AGENTS.md` the band
  understates the prompt by that file.
  → `jeeves-rs/jeeves-app-server/src/memory_center.rs`, `agents_md.rs`
* **Deliberate** — Token counts are estimated with a tiktoken encoding applied across
  providers. The estimate decides when to compact, never what is billed; the provider's
  reported `usage` is authoritative. Calibration is a ratio clamped to 0.5–2.0.
  → `jeeves-rs/jeeves-provider/src/tokens.rs`, `jeeves-rs/jeeves-state/src/chat.rs` (`context_factor`)
* **Deliberate** — The per-turn tool-result budget only touches individually large results; many
  medium results overflowing the cap are left alone (one stderr line).
  → `jeeves-rs/jeeves-loop/src/budget.rs`
* **Deliberate** — The runtime speaks English to the model and to the human. Data stays in the
  language it was written in (model output, session titles, task text, memory pages,
  `AGENTS.md`, a hand-edited `SOUL.md`). The only switcher is the dashboard's zh/en button;
  there is no server-side localisation of error messages.

## Turns and tasks

* **Boundary** — Session metadata holds only a name, a working directory and the fork origin:
  no owner, no visibility, no status. Sessions created before the sessions table have an empty
  working directory.
  → `jeeves-rs/jeeves-state/src/chat.rs`, `migrations/0018_sessions.sql`
* **Boundary** — Creation alone does not execute a task. Human-only `task/start` queues one
  initial execution in a separate work session. Local workers (1–8, default 1) need a live process and
  a configured model; it is not an always-online or distributed scheduler
  ([independent execution](protocol.md#independent-task-execution)).
* **Gap** — Only `feishu_send` and `email_send` write receipts. Feishu claims a `feishu_message` receipt
  after a live send and reads the message back (`GET /im/v1/messages/{id}`): `confirmed` when Feishu returns
  it, `failed` otherwise. Email claims `email_message` and confirms from SMTP acceptance, fails on a
  permanent SMTP rejection, and leaves an unknown post-DATA outcome claimed without retrying. Outside a
  task run both send but write no receipt and say so. Other tools write no receipts.
  → `jeeves-rs/jeeves-state/src/receipts.rs`, `jeeves-rs/jeeves-connector/src/{feishu,email}.rs`
* **Boundary** — "Done" is decided by facts, not by the model: `fileExists`, `fileContains`,
  `jsonEquals`, `toolsOk`, `receiptConfirmed`, `commandSucceeds` and `sourceSupports` are checked after every round and a
  failing condition skips the judge. A command check that did not run (exec off, no sandbox, refused) is
  reported as `skipped:` and counts as not met. Anything else ("the report is good") is not accepted as a condition. `fileContains` is
  a plain substring, never a regex.
  → `jeeves-rs/jeeves-agent/src/verify.rs`
* **Boundary** — Budgets are checked once per round, before the next model call, so a single
  round can overshoot a cap; the ledger reports the overshoot instead of pretending to cut
  mid-call. Budgets are not provider billing reservations.
  → `jeeves-rs/jeeves-app-server/src/turn/accounting.rs`
* **Boundary** — Resuming after a crash happens when a process starts, only for checkpoints
  younger than `JEEVES_RESUME_MAX_AGE` (default 60 minutes) — except checkpoints of an open task that has a budget with room left, which resume at any age; older chat checkpoints are marked `abandoned`
  (a status, not a deletion). Notifications are not replayed; whoever missed them reads the chat
  log. A human interrupt, or a turn that errored inside its process, also marks the checkpoint
  `abandoned`, so an `open` checkpoint after a process ends can only mean a crash.
  → `jeeves-rs/jeeves-app-server/src/resume.rs`, `turn/finalize.rs`
* **Boundary** — On restart a `running` task is marked `suspended` first; an `open` checkpoint
  then resumes it. Crash recovery is automatic, a human pause is not: only `task/resume` moves a
  paused task.
  → `jeeves-rs/jeeves-app-server/src/resume.rs`
* **Boundary** — Background jobs are not attached to a task's ledger by default, except that a
  job started inside a task carries that task's run identity and a task can wait on its jobs.
  → `jeeves-rs/jeeves-state/src/jobs.rs`, `jeeves-rs/jeeves-app-server/src/task/`

## Connectors and gateways

* **Boundary** — Connectors have no retry, backoff or rate limiting: a failed send goes back to
  the model, which decides the next step. Bulk third-party calls are out of scope.
  → `jeeves-rs/jeeves-connector/src/feishu.rs`
* **Boundary** — Feishu sends as the app (`tenant_access_token`) by default; acting as a person
  needs one browser authorization (`jeeves connector feishu login`). The flow has no PKCE and the
  callback is fixed at `127.0.0.1:41766/callback`, which must be registered in the console.
  Renewal rotates the refresh token, so a long-lived app-server renews ten minutes before
  expiry; with every process stopped nothing renews and an expired refresh token needs a new
  login. A personal WeChat account has no programmable outbound API.
  → `jeeves-rs/jeeves-connector/src/feishu_oauth.rs`
* **Deliberate** — Connector allowlists are exact matches: no wildcards, no regex, no domain
  suffix entries. Only the human edits them, and a change takes effect on the next turn.
  → `jeeves-rs/jeeves-connector/src/config.rs`
* **Boundary** — Calendar support is read-only CalDAV/ICS. TZIDs must resolve through the IANA
  database; a custom `VTIMEZONE` definition is not interpreted. Recurrence expansion covers
  `RRULE` and `EXDATE`; `RECURRENCE-ID` overrides and `EXRULE` are ignored. Email reads UNSEEN
  messages as plain text, does not store attachments, and keeps at most 16,000 characters of
  each body. Sending is plain text; attachments are not supported. Connectors have no automatic
  retry, so an uncertain SMTP result is reported rather than replayed.
  → `jeeves-rs/jeeves-connector/src/{calendar,email}.rs`
* **Gap** — The WeChat Official Account and Lark gateways have never been pointed at the real platforms; they
  are tested against fakes only. WhatsApp and voice gateways do not exist.
  → `jeeves-ts/packages/channels/`

## Operations

* **Deliberate** — Scheduled jobs fire at minute granularity, one trigger claim at a time.
  `catch_up` only covers missed, unclaimed triggers; an unsettled claimed trigger may already
  have produced effects, so it is never replayed and blocks later firings of the same job (other jobs are not blocked) until
  `jeeves schedule acknowledge <name>` releases it after review
  ([recovery](operations.md#schedule-recovery-and-delivery)).
  → `jeeves-rs/jeeves-app-server/src/scheduler/`
* **Gap** — The Homebrew formula lives in the repo, not in a tap, so
  `brew install ./packaging/homebrew/jeeves.rb` needs a checkout, and its source hash must be
  refreshed on every release.
  → `packaging/homebrew/jeeves.rb`

## Business form (task API, webhooks)

* **Deliberate** — `/api/run` and synchronous webhooks wait for the turn to finish; "accept now,
  deliver later" is the webhook `callback` mode. Neither has a job queue: if delivery fails (or
  the process dies first) it is lost. The answer is in the session history but is not re-pushed.
  → `jeeves-rs/jeeves-ops/src/api/stream.rs` (`run_headless`), `webhooks.rs`
* **Deliberate** — Replay protection exists only with `signature: true`, and freshness is judged
  by the local clock with a ±300 s window. Plain `X-Jeeves-Secret` mode does not refuse replays.
  → `jeeves-rs/jeeves-ops/src/webhooks.rs` (`SIGNATURE_WINDOW`)
* **Deliberate** — The webhook rate cap is a per-process per-minute counter, with no per-caller
  buckets; a restart clears it.
  → `jeeves-rs/jeeves-ops/src/webhooks.rs`

## Configuration

* **Deliberate** — `config/write` persists to `<home>/settings.json`, applied on top of the
  environment, so a saved patch outranks `JEEVES_*` for that field until cleared (an empty string
  clears model overrides; an empty patch removes the file). Invalid values are rejected
  wholesale.
  → `jeeves-rs/jeeves-config/src/lib.rs` (`apply_patch`, `save_patch`)

## Quality system

* **Deliberate** — Deterministic evals run against a scripted provider (no keys, no network).
  Judge evals need a real model and key and are a separate command (`jeeves judge`); their scores
  never block a release.
  → `jeeves-rs/jeeves-eval/src/`, `evals/`
* **Deliberate** — Scenarios that need something the machine may lack (python3, a shell, a
  working sandbox) declare `prereq` and are counted as skipped, never as passed.
  → `evals/deterministic/exec.jsonl`

## Crash recovery and accounting

* **Boundary** — Tool batches persist intent before execution and a failed checkpoint stops
  execution. Recovery never automatically replays a batch with an unknown outcome: it suspends
  for reconciliation. The one exception: after a crash, a batch whose unanswered calls are all
  proven reads is settled as failed with an "interrupted" result and the turn continues. MCP
  transport errors and timeouts report an unknown outcome, and reconnect never repeats the
  failed call. A foreground command timeout, output-read failure or termination without an exit
  code also reports unknown effects and stops further tools; killing a process does not undo its
  writes. A nonzero exit is a known failure and cannot satisfy `toolsOk`. This is conservative
  batch-level protection, not end-to-end exactly-once execution.
  → `jeeves-rs/jeeves-loop/src/lib.rs`, `jeeves-mcp/src/lib.rs`, `jeeves-app-server/src/resume.rs`
* **Boundary** — Round accounts have stable checkpoint identities; task and run totals commit
  atomically with cumulative watermarks, recovery charges only new deltas, and an accounting
  failure stops the task for review. Wall time lost before a crash is approximate, usage the
  provider never reported is not estimated as known spend, and an already-exhausted budget
  cannot be bypassed by a new turn or recovery.
* **Boundary** — Recovered tasks use the same driver as fresh ones for budget, acceptance, goal
  judgement and later rounds. Failed checks, exhausted budgets, accounting failures and judge
  failures become `needs_attention`. Task identity comes from the checkpoint run, not the newest
  task in the session. Malformed stored acceptance conditions or budgets stop the task instead
  of becoming an empty checklist.
* **Boundary** — Plans commit as whole immutable versions and are advisory: they do not change
  acceptance or authorization. A truncated or iteration-limited model response cannot finish a
  task. Structured handoffs are rebuilt each round from persistent records, independent of
  rolling summaries; corruption, an execution-context mismatch or a handoff over 64,000 bytes
  stops execution instead of truncating constraints.
* **Boundary** — Task job waits persist their exact dependency set. A task with unfinished
  background work becomes `suspended` (`waiting for background jobs`) and resumes after the
  jobs finish, including after restart; unknown, interrupted or killed jobs need review. Human
  pause or cancel wins, and cancelling a task stops its live jobs when the job host is
  available. Waiting grants no permissions or goal rounds. Other kinds of external-event waits
  are limited to those in the next section.
* **Boundary** — Task and goal stop notices, job outcomes and schedule settlements commit with
  their completion receipts in one transaction; a receipt failure rolls back the status.
  Pending notices retry on restart and every 30 seconds; a crash between enqueue and
  acknowledgement can duplicate a notice.
* **Boundary** — Scheduled triggers claim atomically; unsettled claims cannot be overwritten by
  later minutes and recorded minutes cannot replay after a clock rollback. Report bodies persist
  before outbox materialization, different existing content is retained for review rather than
  overwritten, and `produced` denotes generated output, not domain acceptance.

## External input waits

External waits support one immutable condition and one targeted event per task, with a 16 KiB
JSON payload. Source and event-key matching is correlation, not sender authentication. There is
no public callback listener, event broadcast, recurring condition or automatic attachment
retrieval. Host authorization is required; neither declaring a condition nor continuing a task
is a model tool. Input arrival is not verified completion. Persistent human pause fences stop
late checkpoint writes and silent restarts of paused work; they are not cross-runtime
execution leases.

## Operation reconciliation

The operation journal records tool batches atomically with intent and result checkpoints. An
interrupted or uncertain batch stays conservatively unresolved. It cannot prove network
transmission, roll back an effect, prevent a semantically repeated action with a new call ID, or
add idempotency the external provider lacks. Stopped task checkpoints that predate the journal can
reconstruct labelled pending-operation metadata after an exact snapshot check, which proves
neither a pre-execution record nor transmission. Human receipts are evidence with provenance,
not independent verification of a remote service. Ordinary-chat reconciliation is
`turn/operations` / `turn/reconcile`; there is no global external-effect deduplication across
conversations or runtimes.

## Artifact evidence

Verification fingerprints describe bytes observed at measurement time; they are not archived
copies or a lock. A local edit detected during a read fails the check, but a file can change
after verification. Matching a substring does not establish research quality, source truth,
calculation correctness or remote transaction settlement; those need domain-specific acceptance
adapters. Human-reconciled remote effects keep their separate provenance and never become
observed successful tools. The wall-time budget is observation-based, not a hard interrupt of
filesystem I/O.

## Replies without tools

Capability notes check whether the current tool table is empty (delegated turns use their
filtered table); an empty table requests plain text and promises no file search. This does not
guarantee compliance: a live sample returned DSML call markers as its reply, and a
relationship-boundary sample promised continuous availability. The evaluator detects the
observed DSML marker, but it is not a production output filter and does not cover all formats
or false promises.
