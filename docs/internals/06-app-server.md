# ⑥ Server: `jeeves-app-server`

English | [简体中文](06-app-server.zh.md)

This crate is the single execution runtime per `state.db`, and it is where every earlier layer is assembled into a turn. Clients (CLI, dashboard, chat gateway, desktop) speak the protocol of [01-protocol-and-config](01-protocol-and-config.md); the method and notification tables live in [../protocol.md](../protocol.md), the variables in [../configuration.md](../configuration.md).

## Layout

| Path (`runtime/crates/jeeves-app-server/src/`) | Owns |
|---|---|
| `lib.rs`, `server/` | `Server` (shared handles), boot (`runtime.rs`), tool assembly (`tools.rs`), turn table, capacity caps and follow-up queue (`controls.rs`), event sink (`events.rs`) |
| `stdio.rs`, `dispatch.rs` | transport and the method table |
| `turn/` | the turn lifecycle: `entry.rs` (prepare), `execution.rs` (one loop pass), `driver.rs` (task rounds), `finalize.rs`, `settlement.rs`, `prompt.rs`, `history.rs` |
| `task/`, `interaction/` | task ledger switches, dispatch worker, waits, workspaces; the main conversation and its `task` tool |
| `approval.rs`, `permissions.rs`, `pending_approvals.rs` | approval broker, session policies, live pending snapshot |
| `subagent.rs`, `jobs.rs`, `goal.rs` | subagent executor, background job host, goal judge |
| `ledger_writer.rs`, `memory_worker.rs`, `memory_center.rs` | ledger rows, background memory maintenance, the memory center window |
| `resume.rs`, `scheduler/`, `lock.rs`, `resources.rs`, `trace.rs`, `agents_md.rs` | crash resume, scheduled jobs, runtime ownership, mutation gate, trace file, project conventions |

The deterministic half lives in `runtime/crates/jeeves-agent/src/`: `verify.rs` (acceptance checks), `observe.rs` (what a round did), `plan.rs`, `paths.rs` (the one containment rule shared by write tools and acceptance), plus the pure cron and schedule helpers. That crate holds no `Server` and calls no JSON-RPC, so fact judgments are testable alone.

## Server and boot

### Fields and locks

`Server` is `Clone`; every field is a handle (pool, `Arc`, config cell), so a clone copies no state. Closures that need `'static` ownership take a clone.

| Handle | Role |
|---|---|
| `settings: Arc<RwLock<Settings>>`, `resolved: Arc<RwLock<Option<Resolved>>>` | `config/write` and model switches mutate them; the next turn sees the change. `resolved` is `None` when no model is configured: memory and session methods still work, only turns fail with `PROVIDER_ERROR` and say what to do |
| `turns: Mutex<HashMap<turn_id, ActiveTurn>>` | cancellation token, session id, start time and steering channel of each live turn |
| `approvals` | `turn_id → request_id → Waiting` |
| `session_locks` | one async write-section lock per session, created lazily and never removed (removing one would let two concurrent turns hold different locks) |
| `activations` | per-session deferred-tool activations, in memory only |
| `context_usage` | last context breakdown per session, read by `context/usage` |

Std locks are held only for a read-modify-write and never across an await. The per-session write lock covers history append, ledger append, consolidation hand-off and compaction; model calls never hold it, so interrupt is never queued behind it.

### Boot order

`open(settings)`: stack the saved `config/write` patch over the environment, `validate()` the result (a bad value refuses to start, naming the variable), `ensure_home`, acquire runtime ownership (`lock.rs`: a kernel advisory lock beside `state.db` plus an `app-server.lock` PID file; a second runtime on the same database is refused), open the pool, then `Server::boot_owned`. `boot` (no lock) is for embedders and tests that exclusively own their store.

`boot_owned`, in dependency order: apply the active model profile, resolve the provider (failure is logged, not fatal), seed default skills, build the tool table, load session policies, attach the mutation gate, build the stores, register `todo_write` and `delegate_task`, run `restore_tasks`, start completion delivery. The memory worker is attached by the process (`attach_memory_worker`, called by the CLI), not by `open`, so evals stay deterministic; without a worker `finalize_turn` consolidates inline.

### Tool assembly

`builtin_tools` builds every switchable capability once, with `exec`, `write` and `delegate` forced on; each turn then hides what the current settings leave off (`enabled_tools`: `run_command` + `job` for `JEEVES_EXEC`, `write_file` + `edit` + `apply_patch` for `JEEVES_WRITE`, `delegate_task` for `JEEVES_DELEGATE`). Switches are therefore read at turn start and flipping one needs no restart. Also assembled here: web search, browser tools (only when the browser runtime is present), the Feishu connector (registered only with a non-empty allowlist), MCP tools from `mcp.json` (registered exclusive because remote schemas do not prove read-only behavior; an unreachable server leaves one stderr line). `ToolCtx` has one construction site (`tool_ctx`), shared by turns and subagents, so a new field cannot be missed on one path. `delegate_task` receives a copy of the table taken before it is registered: recursion does not exist, rather than being denied.

`resources.rs` is the mutation gate: one mutex shared by all sessions; an effectful tool or a running background job holds it, a second claimant waits 2 s and then gets a text error telling it to inspect progress.

### Startup reconciliation

`restore_tasks` settles what a dead process left behind and never replays an effect: steps left `running` become `waiting`; runs left `running` become `waiting`; `running` tasks become `suspended`; interrupted dispatch rows need review; interrupted queued instructions get a note in the transcript; durable job waits and external-input waits are re-armed; sessions with queued follow-ups start draining. Jobs left running are marked interrupted by the job host. Then, once per transport start, `resume::reconcile` resumes every `open` checkpoint except those that were interrupted, failed, or are older than `JEEVES_RESUME_MAX_AGE` (default 60 minutes); those are marked `abandoned` with one stderr line. Resuming re-enters the same approval gates and widens nothing. `task::dispatch::kick` then starts the task worker.

## Transport

`stdio.rs`: newline-delimited JSON-RPC. Notifications and responses share one unbounded channel with exactly one writer task; notifications are wrapped as method `turn/notification`. A parse failure answers `PARSE_ERROR` with a null id. `turn/start` and `interaction/start` are spawned so the read loop keeps reading (`turn/interrupt` and `approval/respond` must get in while a turn waits); every other method is handled in order, so a script's earlier write is visible to its next query. Logs go to stderr only, because stdout is the protocol channel. When stdin closes the loop ends, in-flight spawned turns finish (the writer lives until every sink clone is dropped), the memory worker gets its stop grace, and MCP sessions are closed.

## Dispatch

`dispatch.rs::handle` always produces a response: `Ok(Value)` becomes a response, `Err` an error, and `Outcome::SelfSent` (turn and interaction methods) means the turn already sent it. The match covers session, memory, config, model, schedule, usage, approval, permission, task, browser and turn methods; anything unknown is offered to `memory_center::handle` and otherwise answers `METHOD_NOT_FOUND`. The authoritative method list is `jeeves_protocol::methods`; do not copy counts here.

Conventions worth knowing before editing: storage `i64` is narrowed to protocol `i32` by clamping (`narrow`); list methods page by an opaque cursor computed on raw rows; `session/main` opens the sole public conversation; `turn/interrupt` returns `interrupted: false` for an unknown turn; `approval/respond` returns `accepted: false` when it arrived too late.

## One turn

`run_turn` ([`turn/entry.rs`](../../runtime/crates/jeeves-app-server/src/turn/entry.rs)) is the order below; each step sits where it does so that a rejected turn leaves nothing behind.

### Prepare

1. Empty session id becomes `default`; memory distilled in the background since the last turn is announced as `consolidationCompleted`.
2. Claim the task: the dispatch row's task for a worker turn, otherwise the session's active task. Before model resolution, because a task may declare a tier.
3. Tier: explicit argument over task declaration; a misspelling is rejected. Resolve the model (`PROVIDER_ERROR` with guidance if none); `route.rs` picks the model by tier or auto mode and the reason goes to `meta.routeReason`.
4. Attachments are read and checked against vision capability.
5. For a task: take `task-control:<id>`, recheck admission (task open, dispatch state matches), refuse an unknown-outcome checkpoint.
6. Capacity: `turn_capacity` (`JEEVES_MAX_TURNS`) then `home_capacity` (`JEEVES_HOME_MAX_MB`), see the section "Capacity, health and shutdown" below.
7. Session gate `begin_turn`: one driver per session; a second turn gets `SESSION_BUSY` naming the holder's turn id ("wait for it, or `turn/steer`"). `TurnGuard` removes the table entry on any exit and then drains the session's follow-up queue.
8. Task admission (`task/contract.rs`: acceptance and budget validated before any model call), heartbeat `set_running` + `taskUpdated`, optional plan, run opened (run id equals turn id; a task-less turn has no run), checkpoint outlet built.
9. Working directory: this turn's explicit one, else the task's, else the session's. A non-existent directory means no writable scope; the process cwd is never used.
10. Send `TurnStarted`; append the plan brief to the message; install the retry sink (each retry sends `retry` and counts into `meta.retries`).

### Execute

`full_turn` is one loop pass and the confluence of everything the model sees: task handoff and progress projection, memory retrieval (bounded by a 60 s pre-loop timeout), skills (index plus matched bodies), history with rolling summary and compaction under the session write lock (also bounded and interruptible), the memory catalog, project conventions, the system prompt, the context meter, the tool table (`enabled_tools`, session activations, the `task` tool on the main conversation only, restricted to proven reads when earlier chat operations are unresolved, deferred schemas under `JEEVES_TOOL_POLICY=deferred`), then `jeeves_loop::run`. A context-overflow error compacts to the last two turns and retries once; any other model error becomes `PROVIDER_ERROR`. Loop events become `toolStarted` and `toolDelta` immediately; `toolCompleted` is sent at wrap-up. See [03-model-and-loop](03-model-and-loop.md) for the loop.

The task handoff includes recent task events, the last recorded edit-check summary, open items from the last successful recorded todo replacement, and paths mentioned by edit operations. Those paths and historical checks are evidence, not proof of current file state; an interrupted or failed operation may be included. The handoff also carries the saved plan and latest verification receipt. Edit-check summaries are recorded as task events without command output.

A failure closes the checkpoint, run and task honestly (`fail_turn`), because an open checkpoint would be resumed at the next boot as if nothing happened.

### Continuous execution

Conversation and task runs use one loop, driven by tool results, user input and stop conditions. Task budgets are checked at model and tool boundaries. Recovery starts from the last settled checkpoint and credits spend already recorded for that round. [Loop exit conditions](03-model-and-loop.md#314-exits-and-wrap-up)

### Task rounds

`driver.rs::drive_task` observes and accounts for the combined result of the round before deciding what to do with it.

- `account_round` writes steps (idempotency key `<turn_id>:<seq>`), artifacts that really exist, spend, and runs the acceptance checks (`fileExists`, `fileContains`, `jsonEquals`, `toolsOk`, `receiptConfirmed`, `commandSucceeds`: a `stat`, a read, an SQL count, or one command through the exec gate; paths resolved through `paths.rs`).
- Decisions consider interruption, task budget, unknown tool effects, running background jobs, declared external input, response or resource limits, acceptance failures, contradictions and finally the judge, in that order.
- Failed acceptance with rounds left (`max_rounds`: the task's, else `JEEVES_GOAL_MAX_ROUNDS`, default 5) feeds the measurement back as the next message and runs another pass; with none left the task needs attention. The judge is not called while a fact fails.
- The judge (`goal.rs`) runs only when the task has a goal condition: the small model, no tools, the last 8 turns capped at 8000 characters, JSON `{ok, impossible, reason}`; a self-contradicting answer counts as not met. `ok` → done, `impossible` → failed, rounds exhausted → failed with status `round-limit`, two stalled rounds in a row (`turn/progress.rs`: same reason as the round before, or no successful tool call) → needs attention with status `stalled`, a broken judge → needs attention with status `blocked`, otherwise the reason becomes the next user message in the same session. Every verdict sends `goalRound`. Only a human activates a goal: `set_task_goal` (the REPL's `/goal`) and `task/goal/confirm` (the card's answer) are the two doors, both reached from human controls. The `task` tool's `goal_draft` writes the inert `agent_tasks.goal_draft` column, which the judge never reads; `Tasks::confirm_goal_draft` promotes it to `goal_condition`, refusing when the row already has one, and clears the draft either way.
- `needs_attention` is non-terminal: a human can continue or cancel.
- Plan (`JEEVES_PLAN`, off by default): before the first pass one model call makes a 3 to 6 step plan stored as `task_steps` (`kind = "plan"`) and appended to the message as text. A failed acceptance may revise it once (`MAX_VERSIONS = 2`). The plan never executes anything and never carries acceptance conditions; a planner failure prints one line and the turn runs as usual.

### Wrap up

`finalize_turn`, in order:

1. `settle_task` (`settlement.rs`) closes the checkpoint (`done`; `abandoned` for `turn/interrupt`; `suspended` for `task/suspend` or an unknown effect; `waiting_jobs` or `waiting_input` when a wait was committed) and the task by decision, and sends `taskUpdated`.
2. Send `toolCompleted` for every call, in request order.
3. `fold_tool_activity` appends one line per call to the stored reply: `[tools used: name(N chars[, full output at spill/…]; first line…)]`. Output over `FOLD_SPILL_CHARS` (2000) with no address of its own is spilled to `<home>/spill/` at fold time. This is the evidence that stops the next turn from repeating an action; `turnCompleted.reply` carries the unfolded text. A footer the model copied from history onto the end of its own reply is dropped first (`strip_echoed_footer`), so only calls that ran this turn are listed.
4. Write one trace line and one usage row (failures only log).
5. Under the session write lock: append to `chat_log` (source `browser-task` for browser tasks so memory extraction skips them), append the ledger (`ledger_writer.rs`), save the (observed, estimated) input-token pair, arm job or input waits.
6. Wake the memory worker and spawn pre-compaction; without a worker, consolidate inline and send `consolidationCompleted`. With a worker, an unnamed session (an id without `:`) that just got its first reply also gets one small-model title call (`turn/title.rs`, metered as `title`). The write only lands while the session is still unnamed with one reply, so a rename or a second turn wins; an answer-shaped reply is refused and the first message stays the name.
7. Send `turnCompleted`, then, only then, the JSON-RPC response, so a client has every notification before the answer. The resume path uses the same `finalize_turn`.

## Prompt assembly

`build_system(PromptParts) -> SystemPrompt { stable, tail }`; the cut is the cache breakpoint.

```
stable = SOUL.md + "your model / provider" line + conversation rules   (byte-identical within a session)
tail   = clock (day granularity) · capability gaps · capability-use note · todo · memory hits ·
         memory catalog · skill catalog · matched skill bodies · project conventions · rolling summary
         (then, appended by full_turn: unresolved-operation note, session permission note)
```

- Nothing that changes per turn goes in `stable`; the clock is day-granular so the prefix does not break hourly. The minute rides on the live ask instead: a fresh turn's user message starts with `[Sent YYYY-MM-DD HH:MM (Weekday), UTC±hh:mm]` (`prompt::sent_stamp`), so "in two hours" and `run_at` can be worked out. Stored history keeps the bare text, and a resumed turn is not re-stamped, so earlier messages never change.
- The capability note appears only when something is off (`JEEVES_EXEC` / `JEEVES_WRITE`, or no tools at all): without it a model cannot tell "tool absent" from "not found yet". With all switches on it is absent. Stored credential names (never values) are listed when exec is on.
- `SOUL.md` is user-owned. `load_soul` replaces it with the current seed only when it is byte-for-byte a built-in seed (current or legacy); once edited it is never touched. `JEEVES_SOUL_MODE` (`auto`/`thin`/`full`) selects the seed; `auto` gives local providers (`ollama`, `lmstudio`) the full one.
- The soul seeds, tail sections, planner prompt and engine error wording are written in one language; user content (replies, titles, memory pages, `AGENTS.md`, an edited `SOUL.md`) stays as written.

### Project conventions

`agents_md.rs`: from the turn's working directory up to the directory holding `.git` (or the disk root), take one file per directory (`AGENTS.override.md`, else `AGENTS.md`, else `CLAUDE.md`), concatenate outermost first with the source path as a prefix. Budget 32 KB filled from the innermost file outward: outer files are dropped whole, an oversized innermost file is truncated and labelled. Tail only, because it changes with the directory. No file means no section and no tokens. Subagents do not get it (`instructions: None`). With no working directory nothing is read.

### Context metering and calibration

`usage_of` meters six segments: soul (`stable`), memory (hits plus catalog), skills (catalog plus bodies), tools (schemas), history, summary. Clock, todo and capability lines are excluded: they are small and do not grow. Budget = context window (`JEEVES_CONTEXT_WINDOW` or the provider table) × `JEEVES_COMPACT_THRESHOLD`, floor 1024, divided by a calibration factor; a zero budget gives ratio 0 and no warning. At or above `JEEVES_CONTEXT_WARN_RATIO` (default 85) the panel carries a notice.

Calibration: at wrap-up the turn stores the last request's provider-measured input tokens and local estimate in `session_context`; the next turn divides its budget by `observed / estimated`, clamped to 0.5 to 2.0. Nothing is stored when either number is not positive, only the latest pair is kept, and a failed write only logs.

## Approvals and permissions

`approval.rs` implements `ApprovalBroker` (the tool layer knows only the trait). A request registers a `oneshot` in `approvals[turn_id][request_id]` (id `ap-<pid>-<micros>-<counter>`, unique across process lifetimes), sends `approvalRequested` (and the optional notifier), then waits with a timeout (`JEEVES_APPROVAL_TIMEOUT`, default 120 s) that this broker, not the tool layer, owns. Whatever happens (approved, denied, voided because the turn ended, timeout) one row goes to the approval audit ledger and the registration is removed; anything but a yes blocks the action, and the broker returns which of denied, timed out or withdrawn it was. `finish_turn` drops a turn's whole bucket.

`answer_approval` returns `false` for too late or unknown (protocol `accepted`). `remember` is applied before the answer is delivered so the next call already sees it: `once` records nothing, `session` writes the live cell only, `always` also persists through `apply_config_patch` (a command is stored verbatim, never as a wildcard; a write approval stores the directory, not the file). A remembered rule does not bypass hard-deny or the sandbox. `approval/pending` returns a live snapshot of unexpired questions owned by a conversation, built from the waiting table, never from the audit log; nothing survives a restart.

`permissions.rs` holds the human-controlled session policy (`readOnly`, `standard`, `fullAccess`) with a per-session revision, persisted and checked at every tool boundary (`ExecutionPolicy`):

- Read-only refuses everything that is not a proven read; standard asks once per operation (arguments redacted, preview capped at 8 KB); full access still obeys enabled capabilities, roots and sandbox. Delegation is refused unless full access.
- Raising to full access needs explicit confirmation; writes use an expected revision, so a stale client is rejected. A policy change pauses the owner's browsers, voids the owner's pending approvals, and broadcasts `permissionChanged`; in-flight effects are not rolled back.
- A task's worker session is bound to its owning conversation: the owner's policy applies, changes are made only through the owner, and the binding is restored at boot.
- Private main-chat actions follow [tool authorization](04-tools.md#authorization).
- Unreadable stored policy blocks tool execution instead of defaulting open.

## Subagents

`subagent.rs` implements `SubagentRunner` for `delegate_task` (the dependency direction forbids the tool layer holding it). Invariants:

- Settings and the resolved model are read at call time through the shared `Arc<RwLock<_>>`, never snapshotted at construction, so a provider switch applies.
- Own context: empty history, short system prompt, no retrieval, skills, summary or project conventions; nothing persisted to `chat_log`; no approval channel; no notifications or interrupt.
- Tool table: the parent's enabled table without `delegate_task` and `todo_write`; read-only batch members also lose the exclusive tools, because four parallel loops cannot honor the single-writer barrier.
- Delegation uses the [catalog output policy](../configuration.md#the-model-catalog-models-catalogjson) and a separate 120,000-token budget.
- `result_schema`: the reply must validate; one retry carries the error and the previous messages; if it still fails the raw text is returned with the reason.
- The conclusion is wrapped as a paraphrase by `jeeves-tools/src/report.rs` (see [04-tools](04-tools.md)).

## Background jobs

`jobs.rs` implements `JobRegistry` for `run_command` with `background=true` and the `job` tool (output, list, kill). It shares the foreground `ExecPolicy`, so background is not a way around approval, sandbox or allowlist. Live output sits in a 64 KiB ring that records how many bytes it dropped; every byte is also appended to `<home>/jobs/<pid>-<id>.log` (1 MiB cap, then a marker). `wait=true` waits at most 20 s and says honestly that the job is still running. Start and finish are rows in `state.db`; at startup a job left running is marked interrupted and never rerun. Jobs are owner-scoped: an unknown id and another owner's id give the same answer. While a job runs it holds the mutation gate.

A task whose round ends with jobs still running is suspended in `waiting_jobs`; a poller (`task/waits.rs`, 1 s) resumes it through the same recovery driver when they settle, and external input (`task/signal`) does the same for `waiting_input` (`input_wake.rs`). Both waits are durable and re-armed at boot. Completion notices are delivered from committed outcome rows (`job_delivery.rs`, `completion_delivery.rs`), retried until the notifier accepts them.

## Main conversation and tasks

`session/main` opens `jeeves-main` (created once, default policy standard, no directory). `interaction/start` is one ordinary turn on it. There is no router model: when a goal needs a durable thread, that turn calls the `task` tool (`create` up to four independent items, `continue`, `cancel`, `list`); work that finishes inside the conversation does not. The tool exists only while that interaction runs and only for senders allowed to dispatch (local views are the owner; chat senders must be listed in `JEEVES_DISPATCHERS`, an empty list admits nobody; the check is in the Runtime). Every `instruction` must be a verbatim, non-overlapping span of the current human message, so the model cannot invent work. An explicit `taskId` continues that task directly, after an ownership check.

Idempotency: `(sessionId, messageId)` is a persisted receipt (message id 1 to 120 characters of letters, digits, `-`, `_`; message at most 64 000 bytes). The same completed input replays its stored events and result without new work; a different input under the same id, or an unfinished or failed receipt, is refused. `turn/start` with `requestId` uses the same table under a `turn:` key. The acknowledgement says queued, never done.

A new task gets a managed workspace `<home>/workspaces/YYYY-MM-DD-title-uniqueId`, canonicalized inside home (title reduced to alphanumerics and `-`, at most 32 characters; the id, not the label, is the identity), unless a project directory is bound. It also receives its exact instruction, any leading document references, and a bounded snapshot of the last two main-conversation exchanges (at most 2000 human and 3000 assistant characters each), marked as quoted data. Main-conversation prompts see only the bounded task progress projection, never task tool output.

Task execution is one serial worker (`task/dispatch.rs`) draining the durable dispatch queue: it names the task's session, binds its permission owner, and runs `run_turn_inner` with the dispatch row. A pending follow-up appends, a running one is steered at the next model boundary (`turn/steer`), `turn/follow-up` queues a message to run after the turn in flight (refused when idle). Human switches (`task/suspend`, `resume`, `cancel`, `signal`, `wait`) are never model tools. `task/list` returns the most recent 100.

## Capacity, health and shutdown

- `JEEVES_MAX_TURNS` (0 = no cap, max 64) limits concurrent turns across sessions; over the cap a turn is refused with `SESSION_BUSY`, never queued silently.
- `JEEVES_HOME_MAX_MB` (0 = no cap) refuses new turns when the home directory (regular files, symlinks not followed, measured off the async thread) is at or over the cap; reads, exports and cleanup keep working so the owner can make room. Both refusals print a stderr line.
- `GET /healthz` (`jeeves-ops/src/api.rs`): calls `session/capabilities` with a 3 s bound and returns 200 with version and protocol version, or 503.
- Shutdown (`jeeves-ops`): on Ctrl-C or SIGTERM the HTTP server stops accepting, then `shutdown(25 s)` closes the child's stdin and waits; the child finishes in-flight turns as described in the Transport section. Past the grace the child is killed and unfinished turns resume from their checkpoints at the next start. The ownership lock is released by the OS on exit.

## Memory center and background maintenance

| File | Owns | Boundary |
|---|---|---|
| `ledger_writer.rs` | structured user / assistant / tool_use / tool_result / image entries per turn | append-only; payloads over `JEEVES_LEDGER_INLINE_MAX` move to `<home>/ledger/` with a pointer and digest; a failure only degrades to a stderr line |
| `memory_worker.rs` | distillation and the bundle mirror off the turn's clock | single flight through the database claim; woken by finalize, 60 s heartbeat backstop; `stop` waits at most 5 s and unfinished work resumes at the next start |
| `memory_center.rs` | read window (ledger, index, page, context usage, skill stats) and `session/compact` | read-only, sandboxed to home, page paths validated by `Bundle::page_path`; compaction is human-only and shared with the REPL |

Invariants: `entries_for_turn` and `payload_text` decode the same rows, so a spilled row is both viewable and rebuildable; summaries only move forward (`save_rollup` refuses to overwrite a summary covering more turns, and the error says so); manual compaction keeps the newest 2 turns. Retrieval, consolidation and the bundle are in [05-memory](05-memory.md), the tables in [02-state](02-state.md).

## Observation files

`trace.rs` appends one JSON line per turn to `<home>/traces/<local date>.jsonl`: `meta` plus flat fields for `jq` (`tools[]`, `retries`, `tokens` with estimated and observed input and the turn budget, `history` turns total/kept/compacted, `steps`, `continues`, `unfinished`). The usage ledger is in `state.db`. A failed write logs to stderr and never panics or fails the turn.

## Where to change

| To change | Edit |
|---|---|
| a method | `jeeves-protocol` v2 types and `methods`, then `dispatch.rs`, then `just write-app-server-schema` |
| the order of a turn | `turn/entry.rs`, `driver.rs`, `finalize.rs`; keep checks before the judge and the response after the notifications |
| a tool's capability switch | `enabled_tools` and `builtin_tools` in `server/tools.rs` |
| prompt sections | `turn/prompt.rs` (`stable` must stay byte-stable per session) |
| session policy or approval | `permissions.rs`, `approval.rs`, `server/controls.rs` |
| a cap | `jeeves-config` bounds, then `server/controls.rs` |

Tests: unit tests sit beside each module (`turn/tests.rs`, `dispatch_tests.rs`, `approval_tests.rs`, `permissions_tests.rs`, `jobs_tests.rs`, `resume_tests.rs`, `interaction/tests.rs` and others); cross-module assembly behavior is in the evals of [08-quality](08-quality.md). Architecture overview: [../architecture.md](../architecture.md); known gaps: [../limitations.md](../limitations.md).
