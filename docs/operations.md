# Operations

English | [简体中文](operations.zh.md)

Every way to run Jeeves day to day, the recommended environment, and
troubleshooting.

## Supported platforms

| Platform | Status | Why |
|---|---|---|
| macOS (Apple Silicon / Intel) | ✅ first-class | seatbelt sandbox, Keychain, `sh` hooks |
| Linux | ✅ first-class | bubblewrap sandbox, secret-service, `sh` hooks |
| Windows (native) | ❌ refuses to boot (exit 2) | no sandbox backend — per the "no sandbox, no execution" rule `run_command` could never run; hooks need `sh`; the credential store cannot read secrets back, so keys would sit in plaintext `secrets.json` |
| Windows + WSL2 | ✅ use the Linux build | everything above is a real Linux user space |

The refusal happens in the `jeeves` binary at startup (`cfg!(windows)`) with the
reasons printed — a half-working assistant that looks healthy is worse than a
clear no. CI keeps a Windows job for build + unit tests only (no sandbox, no
hooks tests); libraries compile everywhere, the binary simply will not boot.

## Terminal conversation

```bash
cd jeeves-rs && cargo build
JEEVES_PROVIDER=deepseek DEEPSEEK_API_KEY=… ./target/debug/jeeves
```

The terminal opens the Runtime’s main conversation. `/memory [term]` queries memory, and `/quit` exits. `Ctrl-C` in the terminal
exits the process — the dashboard uses `turn/interrupt` to cancel a turn.

Explicit memory and session reads report storage errors. If one memory index is available, the terminal still prints its results.

## Web dashboard

```bash
cd jeeves-ts && npm run build --workspace @jeeves/dashboard   # build the frontend once
JEEVES_HOME=$HOME/.jeeves ./target/debug/jeeves dashboard        # → http://localhost:7777
```

The first screen shows config / sessions / memory; the chat area shows the
tool calls and streaming replies live. If port
7777 is taken it scans upward (10 tries).

- **The page remembers where you were.** Which conversation is open is kept in
  `localStorage` (`jeeves.session`, next to the working directory), so a reload lands back in
  that session instead of the oldest one; if that session was deleted meanwhile, the newest
  one opens instead — never an empty shell the list cannot show.
- **Switching back returns to where you were reading.** Each session keeps its own distance
  from the bottom (in memory, per page load): leave in the middle of a long conversation,
  come back, and you are at that same line — no dragging. A session opened for the first
  time in this sitting goes to the newest message, as does a reload; the position is placed
  instantly, not animated from the top.
- A cross-device ping (another device finished a turn) re-reads the history **without**
  moving the scroll position: if you are reading further up, you stay there.
- **The agent's own wording is English.** `SOUL.md`, the prompt sections, the tool-result
  messages and the planner prompt are English, whatever language you write in — the model
  answers in yours. `<home>/SOUL.md` is yours to edit: it is used verbatim, and only a file
  identical to the built-in seed is ever replaced.
- **Two languages.** The button at the top right of the sidebar switches between Chinese and
  English and remembers the choice; a fresh browser follows `navigator.language` (Chinese for
  `zh-*`, English otherwise). Switching reloads the page — strings are baked into the DOM as
  they are painted. Model output, session titles and task text stay in whatever language they
  were written in: those are data, not labels.

### Putting it on a server (self-hosting)

```bash
export JEEVES_OPS_HOST=0.0.0.0            # a non-loopback bind requires a token, or startup is refused
export JEEVES_OPS_TOKEN=$(openssl rand -hex 16)
export JEEVES_HOME=/srv/jeeves/.jeeves
jeeves dashboard                          # → http://<server>:7777 (/api/* needs the Bearer token)
```

- The standalone dashboard asks for the token once on the first 401 and keeps it in
  localStorage; the static page itself needs no token (it must load before it
  can ask).
- **No key files to copy**: put `"key_source": "env:DEEPSEEK_API_KEY"` in
  `models.json` and export that variable in the server's environment. The key
  is only read into the memory of the process running the turn — never on
  disk, never in an `/api/*` response.
- TLS belongs to a reverse proxy (Caddy / nginx terminate HTTPS); Jeeves speaks
  plain HTTP.
- Stdio MCP servers execute on the Runtime host. HTTP MCP servers execute at their configured endpoints. On shared infrastructure, isolate each user’s Runtime and tools. [Hosted deployment](../control-plane/README.md)

## MCP servers

Write `<home>/mcp.json` (stdio or HTTP servers); their tools merge into the
tool registry. For `"oauth": true` remote servers run `jeeves mcp login <name>`
first; expired tokens auto-refresh, and without a refresh_token rerun login.

The dashboard's capability center is the graphical door to the same file:
add / replace / delete a connector, with the transport and credential rules
enforced the same way (`url` **xor** `command`; `auth_env` is a variable
name, never a key). Changes take effect on the next start. Set
`JEEVES_MCP_CONFIG` for machine-wide servers — those rows show up greyed out
under a "Built in" tag and every dashboard write refuses them.

Known operational limits (no log rotation, and more) are listed in
[limitations.md](limitations.md).

### Serving Jeeves's memory to other agents

The reverse direction also exists: `jeeves mcp serve` makes Jeeves itself an MCP
server over stdio, exposing five memory tools (`memory_search`,
`memory_remember`, `memory_forget`, `memory_list`, `memory_episodes`) so
other agents on this machine read and write the same facts. Only memory is
exposed — nothing that makes Jeeves act on someone else's behalf.

```json
{"mcpServers": {"jeeves-memory": {
  "command": "jeeves",
  "args": ["mcp", "serve"],
  "env": {"JEEVES_HOME": "/Users/you/.jeeves"}
}}}
```

Point any MCP client (Claude Code, codex, …) at that stanza and it shares
Jeeves's long-term memory. The server opens the same `state.db` through the same
`open()` (WAL + busy_timeout, so multiple processes coexist); facts written
by other agents are labelled `source: mcp` so the origin stays visible.

## Morning brief (cron-friendly)

Set `JEEVES_MORNING_BRIEFING=outbox@Asia/Shanghai` in the resident Runtime's environment and restart it.
Replace the IANA timezone after `@` with yours. Off by default, the built-in duty runs daily at local 08:00.
It lists today's CalDAV/ICS events, recent unread IMAP mail with short excerpt-based summaries and suggested
draft replies, and unfinished tasks with their latest timeline entries. A missing source is named in the brief;
replies are never sent. The Runtime builds it directly from those sources, without a model.

- `outbox@Area/City` (local only): writes a proposal under `<home>/outbox/schedule-daily-morning-briefing-*.md`.
- `notify@Area/City`: saves the same local copy and delivers the proposal once to the configured
  [webhook](configuration.md#dashboard-and-binaries), enabled with `JEEVES_NOTIFY=1`. Choose a private destination: it receives event titles, task progress,
  and unread-mail excerpts. A delivery failure is visible in stderr and `schedule-runs.jsonl`; it is not
  retried. Check the local copy and destination, then run `jeeves schedule acknowledge daily-morning-briefing`
  to unblock later briefs without replaying the failed one.

Sleep/wake catch-up is limited to one proposal for the current local date. The older ad-hoc
`morning-briefing` skill remains available for GitHub/web/memory scans; it is separate from this duty.

## Driving it from a script

There is no client library to install: `jeeves app-server` speaks JSON-RPC over
stdio, one JSON object per line, so any language can drive it in a few lines.

```python
import json, subprocess

proc = subprocess.Popen(["jeeves", "app-server"], stdin=subprocess.PIPE,
                        stdout=subprocess.PIPE, text=True, bufsize=1)
proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "turn/start",
                             "params": {"message": "hi", "stream": True}}) + "\n")
proc.stdin.flush()
for line in proc.stdout:               # notifications and the response share the stream
    print(json.loads(line))
```

Environment variables are decided by the embedding process (the subprocess
inherits them); `JEEVES_BIN` can point at the binary.

## Packaging

```bash
JEEVES_OPS_TOKEN=$(openssl rand -hex 24) docker compose up -d   # dashboard, see compose.yaml
docker build -t jeeves .
docker run --rm -p 7777:7777 -v jeeves-state:/home/jeeves/.jeeves \
  -e JEEVES_OPS_TOKEN=… jeeves                     # the same, without compose
docker run --rm -it -v jeeves-state:/home/jeeves/.jeeves \
  -e ANTHROPIC_API_KEY=… jeeves app-server      # protocol on stdio
```

All state lives under `JEEVES_HOME` (`/home/jeeves/.jeeves` in the image), so one volume
carries it. The image binds the cockpit to all interfaces, so `JEEVES_OPS_TOKEN` is mandatory (startup refuses without it), and it carries the built front end and Node.js for the hosted chat gateway. It installs `bubblewrap`, which is what the Linux sandbox
backend needs — without it `run_command` refuses to execute anything, by
design. A Homebrew formula template lives in `packaging/homebrew/jeeves.rb`
(source build via `cargo install`; fill in the release tarball's sha256).

`compose.yaml` sets `stop_grace_period: 30s` and a healthcheck that polls `/healthz`
(200 with versions, or 503). The dashboard first checks a real typed Runtime capability response (up to 60 s); only then does it bind a port, announce its address and start the Lark gateway. A conflicting home refuses startup instead of leaving a broken listener. Ctrl-C or SIGTERM closes persistent SSE streams and starts the HTTP, gateway and Runtime drains concurrently. Running turns have up to 25 s to settle, with a short forced-stop/reap fallback; unfinished turns remain recoverable. An engine subprocess exit also stops the cockpit. `just smoke-lifecycle` exercises both stop signals, duplicate-home rejection and engine crash recovery with temporary homes and ports.

## Keeping it running

`jeeves service` writes a supervisor unit so the Runtime (`jeeves dashboard`) survives closing every client and a reboot. The supervisor is the operating system's own; Jeeves adds none.

```bash
jeeves service print            # show the unit that would be written
jeeves service install          # write it; nothing is started
jeeves service install --start  # write it and load it (starts now and at every login)
jeeves service status           # unit installed? is the port answering?
jeeves service uninstall        # unload and remove the unit; state in JEEVES_HOME is untouched
```

* macOS: `~/Library/LaunchAgents/ai.jeeves.dashboard.plist`, restarted after a failure with a 10 s back-off.
* Linux: `~/.config/systemd/user/jeeves.service` (`Restart=on-failure`, 5 s). To start at boot without a login, run `loginctl enable-linger $USER` once.
* Containers: `restart: unless-stopped` in `compose.yaml`.
* Windows has no template (the Runtime does not boot there).

The unit pins `JEEVES_HOME` and `JEEVES_PORT` to the values of the shell that ran `install` and logs to `<home>/jeeves.log` (no rotation). It never contains a token or key: keep `JEEVES_OPS_TOKEN` and provider keys in the keychain or the settings file, because a supervisor does not read your shell profile.

## Mac desktop app

The hosted Mac app connects a device to the cloud account. Build it with `just desktop-device-build https://jeeves.example.com` using the installation's public origin. Open the app, choose **Connect**, and approve the pairing in the browser. Jeeves requests window access on the Mac when a task needs it. Manage access in the account's device list. Open the workspace in a browser or on iOS to talk to the assistant and follow the task.

The following commands apply to the source checkout's Runtime development app.

The desktop shell (`desktop/`, Tauri) starts `jeeves dashboard` and loads its address; setting
`JEEVES_RUNTIME_URL` attaches it to an already-running Runtime instead. `just desktop-install`
installs the front-end dependencies, `just desktop-build` bundles the release `jeeves` binary
into the app, `just desktop-dev` runs it in development, and `just desktop-build-browser`
adds the optional task-browser runtime prepared by `just browser-runtime`.

For everyday use and joint testing, run `just desktop-open` or double-click the checkout’s `desktop/src-tauri/target/release/bundle/macos/Jeeves.app` in Finder. Both open the same application bundle. If it is already running, macOS brings its window forward with the current Runtime, login and conversation. The command builds the bundle if it is missing. Use `just desktop-build` when you need to rebuild it after a code change.

Before a joint test, run `just desktop-status` to record the existing app and Runtime PIDs, listener, and state and trace paths. The checkout revision describes the source tree. The running app may come from an earlier build. Use the same window and account, then leave the tested task or error open for handoff. For a reproduction, record the steps, local time and, when available, task ID or turn ID. Look up that turn under the reported trace directory. If the Runtime's stderr goes to `/dev/null`, record the visible error as well. Automated smoke runs use fixtures and report their results separately.

Double-clicking the Mac app starts its bundled Runtime. With Finder’s root working directory and no explicit `JEEVES_HOME`, the app in the repository build output reuses the checkout’s existing `.jeeves` directory. An installed copy defaults to `~/.jeeves`. Set `JEEVES_HOME` explicitly when launching from a terminal to select a different profile.

The Mac app and mobile web view share one interface and follow the device’s light or dark appearance.

For the running development bundle in this checkout, run `just desktop-refresh-ui` and reload its window to pick up interface changes. A replaced Runtime binary takes effect on its next launch.

The app opens in Chat. Open a receipt or My work to read a delivery and request a revision. [Chat, work and delivery](design/main-entry.md)

To repeat the native acceptance on a Mac, run `just smoke-mac-cockpit` from the repository. It builds Jeeves.app, opens its own foreground window, and drives the chat and Cowork with an isolated Runtime and simulated model and publication service. It quits and reopens the app on the same temporary data to check task identity, conversation history and a single completion. Accessibility must already be enabled for the runner. When Screen Recording is already available, the test saves screenshots of the Jeeves window alongside its JSON report under `jeeves-rs/target/acceptance/mac-cockpit/<run>/`.

## Scheduled jobs

The resident Runtime (`jeeves app-server`, `jeeves dashboard`) fires scheduled jobs on a five-field
cron (`*`, `*/step`, `a-b`, `a,b`); there is no separate scheduler process. Two ways to declare one:

* a skill whose frontmatter carries a schedule:

  ```markdown
  ---
  name: weekly-review
  description: summarize the week and draft the Monday brief
  schedule: 0 8 * * 1
  ---
  ```

* a stored job, made from the capability center or by asking Jeeves (`schedule_job`). Jobs live in
  `state.db`. An old `<home>/schedules.json` is imported once at startup and renamed to
  `schedules.json.migrated`.

Each generic job runs a full turn in its own session (`schedule:<name>`), so jobs
carry their own history and rolling summary; the answer lands in
`<home>/outbox/schedule-<name>-<stamp>.md` rather than interrupting chat. One
firing per minute at most, jobs run one at a time, skill edits are picked up
without a restart. Keep the Runtime up with [`jeeves service`](#keeping-it-running); a job missed while it was down is
caught up once on the next start (only the latest missed minute, looking back at most seven days; `catch_up` off drops it).

Both sources show up in the dashboard's capability center (**Schedules**), next to
Skills and MCP connectors: one row per job with its cron, its prompt, the next fire, the
last fire, and where the result goes. Rows that come from a skill are greyed and read-only
— that job *is* the skill, so the way to remove it is to edit `SKILL.md` or delete the
skill. Adding or deleting a stored job changes exactly that row in `state.db` and nothing else:

```
name      morning-news        lowercase letters, digits and hyphens (the schedule key)
cron      0 8 * * 1-5         five fields: minute hour day month weekday
prompt    Give me today's tech news   one sentence — it becomes one turn
delivery  outbox | notify | quiet
catch_up  run once after a miss (default), or drop it
```

The panel is an editor for jobs, not a scheduler: the Runtime fires them, and the panel says so at
the bottom of the list — a form that silently needed a running process would be worse than no form.

The panel is not the only door: say it in chat — "every day at eight, give me a tech news brief" — and the model
calls `schedule_job`, which stores the same row through the same validation. It answers
with the next fire time and the reminder that the Runtime has to be up. Deleting
from chat is `cancel_schedule`; listing is `list_schedules`.

Why a tool and not "let it write a file": a job declaration is Jeeves's own state, not project content,
so it lives in `state.db`, written by the tool that owns it; nothing asks for a working directory.

## Data and backups

Only a few things under `<home>/` are worth backing up: `state.db` (memory
and conversation, the source of truth), `SOUL.md` (persona), `skills/`
(procedural memory), `mcp-auth/` (OAuth tokens, 0600). `traces/`
and `outbox/` are regenerable derivatives.

`jeeves home export <dir>` writes the whole home to a new directory: `state.db` as a
consistent snapshot (safe while Jeeves runs), every other file as it is, and
`jeeves-home.json` with the format and schema version. `jeeves home import <dir>`
restores it into an empty `JEEVES_HOME` and migrates an older schema forward. The
same pair moves a home to another machine or between local and cloud. Keys and
sign-ins are left out (`secrets.json`, `mcp-auth/`, `connector-auth/`); give
them again after an import. Choose a destination outside the source tree. The finished
bundle or restored home appears at the destination after copying and database checks succeed.

## Schema changes: expand, migrate, contract

Every change to `state.db` follows three steps, so a downgrade never breaks work in flight:

1. **Expand.** Add only nullable columns or new tables. Old and new binaries both run on the result. Never rename a column in place.
2. **Migrate.** Backfill in code, and make the backfill safe to run twice.
3. **Contract.** Drop the old structure at least one release after nothing reads it.

On start, Jeeves refuses a database whose applied schema is newer than the binary knows, and prints both
version numbers and the backup path. Before migrating an existing database it copies `state.db` to
`state.db.pre-<version>` (`<version>` is the target schema) and prints that path. The copy is made once;
a new database is not copied.

## Troubleshooting

| Symptom | Action |
|---|---|
| `PROVIDER_ERROR (-32000)` missing key | add the key with `/model add` (or export the variable the profile names) and restart the process |
| 401 / 403 | key expired or region unreachable; replace it with `/model add <name> <provider> --key new` |
| MCP tools disappeared | read the stderr warning: unreachable / not signed in (`jeeves mcp login`) / broken mcp.json |
| `[tools used: …]` in a reply | expected folding marker; tool activity is in history |
| "why did it answer that" | open `traces/<date>.jsonl` at that turnId |
| port 7777 taken | the dashboard scans upward; the startup log has the real port |

## Compacting and looking at memory

* `/compact [focus]` in the REPL — or **Compact now** in the dashboard's memory center — folds
  the older turns into the rolling summary **now**, keeping the newest two in the prompt. The
  focus is one extra sentence in the summarizer's material ("keep the migration plan in
  mind"), not a different prompt: summaries have to stay comparable turn to turn.
* The memory center has three read-only views. **Ledger**: one row per turn,
  open it for that turn's entry stream; `Folded` marks a backfilled turn whose tool output only
  exists as a summary. **Library**: catalog on top, one page below, and the `ledger:…` chips
  walk back to the rows the page was distilled from. **Context**: the six-section band
  (soul / memory / skills / tools / history / summary) with used, budget and a notice at
  `JEEVES_CONTEXT_WARN_RATIO`.
* The band measures what **grows with the session**. Four things ride in every prompt
  uncounted: the clock, the todo board, the capability note (the two or three lines that
  appear when `JEEVES_EXEC` / `JEEVES_WRITE` are off), and the project-conventions section read
  from `AGENTS.md` / `AGENTS.override.md` / `CLAUDE.md` between your working directory and
  the enclosing `.git`. The first three are a few dozen tokens; the conventions are capped
  at 32 KB — so in a repository with a fat `AGENTS.md` the band *understates* the prompt by
  that file. That is the trade: the band answers "what grows within a session", and a file
  that is constant per directory does not grow.
* A turn may spend at most `JEEVES_MAX_ITERATIONS` model calls (default 32). The **last one is a
  wrap-up**: the tools are taken away and the model has to answer in three sentences — done /
  stuck / what it needs next. A long turn therefore ends with something you can act on
  instead of an apology. Oversized tool results are stubbed oldest-first before each request
  (`JEEVES_TOOL_RESULT_TOTAL_CHARS`), so one chatty command cannot fill the window by itself.
* `jeeves memory rebuild` folds hand edits of the library pages under `<home>/memory` back into the
  facts and regenerates the library; `jeeves memory backfill` rebuilds ledger rows from old chat history.

## Crash recovery has an age limit

A checkpoint left `open` means the last process died mid-turn. At startup Jeeves resumes it —
the human asked for that work, and continuing is not the model assigning itself anything.
`JEEVES_RESUME_MAX_AGE` (minutes, default `60`, at most `10080`) bounds how old the row may be;
`0` turns the limit off. Checkpoints of an open task with a budget that still has room are exempt and resume at any age. Turns the human interrupted and turns that failed inside their process
are marked `abandoned` and not resumed either.

Why an hour and not "forever": a checkpoint is crash recovery, so its real span is seconds
to minutes. Resuming a two-day-old turn is not the same turn continued — "give me today's
tech news", answered two days later, answers a question nobody is asking, and it spends a
real model call plus a real fetch to do it. The stale row is marked `abandoned` and the
startup log says so:

```
(jeeves) reconcile: turn t… stopped before 2 days (limit 1 hour), not resuming on its own — to carry on, say that sentence again
```

The ledger keeps the row either way: `abandoned` is a status, not a deletion.

## Task workers and leases

`JEEVES_TASK_WORKERS` (1–8, default 1) sets how many human-started tasks run at once; with 1 the behaviour is unchanged. Each running task holds a lease that its worker renews every 10 s and that lapses after 30 s. A lapsed lease is swept exactly once: the task resumes from its open checkpoint, or, with no checkpoint, goes to `needs_attention`. Either way there is a `lease_expired` task event and a stderr line. A restart clears all leases, and the existing startup recovery decides what resumes.

Recovery of a task after a restart still waits on worker slot 0, so with several workers it is serialised there.

## Schedule recovery and delivery

Task and schedule completion notifications use a stable receipt key when entering the local webhook outbox. Reopening the producer after enqueue or after confirmed delivery cannot create a second local notice. A receiver that accepted the HTTP request just before its response was lost may still see a duplicate.

A machine that sleeps while the Runtime keeps its process is not a process that was down. The scheduler notices a wall-clock jump of more than 90 s between two ticks and runs one reconcile pass over the gap with the same rules as startup catch-up: the latest missed, unclaimed trigger fires once (never a chain, at most seven days back); a job with `catch_up` off is reported on stderr and not run; a claimed, unsettled trigger is never replayed. Waits declared with `task/wait` already poll the wall clock every second, so a deadline that passed during sleep is handled on the first poll after wake.

`catch_up` only covers missed triggers that were never claimed. An unsettled claimed trigger may already have produced external effects: restart does not replay it, and later minutes cannot overwrite it. Stop the Runtime, review the actual effects, then run:

```sh
jeeves schedule acknowledge morning-news
```

Every firing is first recorded in the wake inbox under the key `schedule:<name>:<minute>`; a repeated key is skipped with a stderr line and a notification, never silently. After a restart a firing that was claimed but not settled moves to `reconcile` and is never redelivered. It blocks only its own job until acknowledged; other scheduled jobs keep firing.

This releases the reviewed old claim without replaying it; future authorized triggers may proceed. Do not bypass an unreviewed outcome by deleting state, changing the clock or repeatedly starting it. Failed claim persistence prevents model execution.

SQLite stores the result, settlement and delivery intent. A failed outbox write retries from stored content; different existing content is retained for review rather than replacing human edits. With notifications enabled, enqueue follows confirmed file delivery and stays tied to the original configured destination across retries/restart. `produced` denotes generated output, not domain acceptance or user readership.

## Browser hold across a restart

The task browser is a child process and ends with the Runtime. A human takeover that was open at that moment is kept as a hold (`browser_holds`): the browser session is marked `needsAttention` with a message naming the site (origin only, never the page URL), crash recovery refuses to resume the task behind your back, and a stderr line and a task notice say so. The hold survives further restarts. It ends only when you open a fresh browser for the task: Jeeves launches it, takes a new observation and then clears the hold; the old page state is never trusted. Only `human` control counts as a hold; an agent-controlled or policy-paused browser is simply ended as before.

## Waiting for task input

Before **Start task**, choose **Waiting for input** and describe the required data, optionally setting its latest arrival time. **Provide input** stores the data; receiving it does not itself satisfy acceptance. The task keeps its draft while waiting and resumes when the input arrives. You can keep chatting in the originating conversation. **Pause** stops automatic wake-up; data remains stored and **Resume** explicitly authorizes continuation. A resume request may wait up to ten seconds for the previous execution to stop; a refusal leaves the pause intact.

SDK hosts use the authenticated `task/wait` and `task/signal` methods described in the [protocol](protocol.md#durable-external-input). Keep the same event ID and data when retrying a signal. An input deadline applies to arrival; accepted input remains usable after it. Restart the runtime normally to recover a committed event and draft. Unknown tool effects still require reconciliation before resuming.

## Reviewing unknown operations

For a paused task or one needing attention, choose **Review operations**. Check the actual external service or file, select the observed outcome and record supporting evidence or an external receipt. A transport error alone does not prove that nothing happened. The form retains evidence when the server refuses a save. Recording the result keeps the task stopped; use **Resume** separately after every unresolved operation is settled.

SDK hosts can inspect and reconcile through the [operation protocol](protocol.md#operation-journal-and-human-reconciliation). Re-read the checkpoint digest after another operation is reconciled. Do not use reconciliation to manufacture a successful `toolsOk` measurement or provider idempotency guarantee.

## Running Jeeves for many users

Web and iOS users sign in at the public HTTPS origin. The Mac device app pairs with the same account. Deploy the [Go unified gateway](../jeeves-ts/packages/gateway/README.md), the private [Go control plane](../control-plane/README.md), and a Rust Runtime image for tenant Pods. Keep the control plane, PostgreSQL and Runtime endpoints private.

For a personal source installation, run `just local-up` and create the owner in the browser on that machine. It runs the gateway and one Runtime on loopback.
