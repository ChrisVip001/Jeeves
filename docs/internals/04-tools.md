# ④ Tools: `jeeves-tools`

English | [简体中文](04-tools.zh.md)

Everything the model can use to act lives in `runtime/crates/jeeves-tools/`. The tool layer knows only traits
(`SubagentRunner`, `ApprovalBroker`, `JobRegistry`, `ExecutionPolicy`, `ResourceCoordinator`, `BrowserHost`, `ComputerUseHost`);
every implementation lives in `jeeves-app-server` ([06](06-app-server.md)). Wire types are in
[01](01-protocol-and-config.md), the loop that calls tools is in [03](03-model-and-loop.md), the memory behind
`memory` / `read_page` is in [05](05-memory.md).

## Inventory: 15 always on + 11 gated

"Always on" means no capability switch controls the tool. `handlers::build_default_with_web()` installs 14 of them and
`app-server` adds `todo_write`. The list is pinned by the test `default_registry_has_the_expected_tools`, and
`scripts/check_tool_docs.py` reconciles this section against the code.

| Group | Tools | In one line |
|---|---|---|
| Memory | `memory`, `read_page` | `memory` takes an `action`; `read_page` reads a memory page or skill body by `kind` |
| Files (read) | `read_file`, `glob`, `grep`, `find_symbol`, `repo_map` | Read-only, no shell needed |
| Web | `fetch_url`, `search_web` | `search_web` is registered only when a search backend is usable |
| Discovery | `find_tools` | Opens deferred tools ([Deferred loading](#deferred-loading)) |
| Messages | `send_message` | Writes a draft file; never sends |
| To-do | `todo_write` | Session to-do list |
| Scheduling | `schedule_job`, `list_schedules`, `cancel_schedule` | Jobs stored in the state database |

Gated tools:

| Tool | Gate | Absent means |
|---|---|---|
| `run_command` | `JEEVES_EXEC` | No command can run |
| `job` | `JEEVES_EXEC` | Background jobs cannot be inspected (`action` = output / list / kill) |
| `write_file`, `edit`, `apply_patch` | `JEEVES_WRITE` | No file can be changed |
| `delegate_task` | `JEEVES_DELEGATE` | No subagents |
| `browser_observe`, `browser_act`, `browser_batch` | Browser runtime installed (`JEEVES_BROWSER_RUNTIME`) and available | No task browser |
| `computer_observe`, `computer_act` | macOS window helper installed (`JEEVES_DESKTOP_HELPER`); a window must be selected before each call | No desktop window; the person selects one from the task card |

Not counted: connector tools (`feishu_send`; `calendar_list`; `email_read`; `email_send` when the email
allowlist is non-empty), the `task` tool (registered per turn on the main conversation, and only for senders allowed to dispatch),
and MCP tools. Connector reads declare `read`; an undeclared tool effect is always `write`.

## The registry contract

`ToolRegistry::execute(ctx, name, args) -> String` returns text and never `Err`. A failure is a sentence the model reads:

| Case | Result |
|---|---|
| Unknown name | `Error: no tool named '{name}'. Available: …` (a name that differs from exactly one registered tool only in case, `_`/`-`/space separators, a trailing `tool`, or a leaked `"`/`<` fragment runs that tool and logs the repair on stderr) |
| Arguments violate the schema | `Error: the arguments do not match the {name} schema — …` (at most three violations, each with its field path) |
| Handler returns `Err` | `Error: {name} failed: {e}` |
| Refusal or business failure | The handler reports a failure and the registry renders its message |

The loop uses `execute_result` to read the status from authorization, validation and the handler alongside its text. Successful content may start with `Error:`.

`register` precompiles `input_schema` with `jsonschema`; `execute` validates before anything else, so a
failed validation never reaches a handler. A schema that does not compile skips validation and logs one line on stderr.
When validation fails only because a top-level integer, number, boolean, array or object argument arrived as a JSON string (`"5"`, `"true"`, `"[…]"`), `execute` parses it, runs the call if the result validates, and logs the repaired keys on stderr; anything else keeps the error above.
`require_str` trims and rejects missing, non-string and empty values; `opt_u32` rejects negatives.

Order inside `execute`: schema validation, then the host's `ExecutionPolicy::authorize` (see
[Authorization](#authorization)), then a mutation permit from `ResourceCoordinator` for exclusive tools (except
`delegate_task`, whose children acquire their own), then a re-check that the policy revision did not change while waiting,
then the handler.

**Exclusive tools.** `register_exclusive` marks a tool that must run alone (the loop turns it into a barrier). Write, exec, subagent,
browser and MCP tools are exclusive. When an execution policy is installed, every tool that is not a proven read
(`permissions::proven_read`) is exclusive too.

**Names.** `register` refuses a name that is already taken and says so on stderr; it never replaces an existing tool.

**Read-only view.** `ToolRegistry::readonly()` returns a copy without every exclusive tool; `without(name)` drops one tool.

## Per-turn assembly

At startup `builtin_tools()` registers everything with `exec_enabled`, `write_enabled` and `delegate_enabled` forced on;
`delegate_task` and `todo_write` are registered in `Server` construction. Each turn then calls `enabled_tools(&tools, &settings)`,
which removes the tools whose switch is off in the **current** settings:

| Switch | Removes |
|---|---|
| `exec_enabled` | `run_command`, `job` |
| `write_enabled` | `write_file`, `edit`, `apply_patch` |
| `delegate_enabled` | `delegate_task` |

The switches come from `JEEVES_EXEC` / `JEEVES_WRITE` / `JEEVES_DELEGATE` or from `settings.json` (patched through `config/write`);
they are read at turn start, so a change applies to the next turn without a restart. A tool that is off is not in the
table at all; it is not "registered, then refused". The prompt tells the model what is missing
(`capability_note`, [06](06-app-server.md)) so it does not mistake a missing tool for one it has not found.

After the switches, the turn's table gets the per-session activation set ([Deferred loading](#deferred-loading)), the
`task` tool for the main conversation, and, when earlier chat operations are unresolved, a restriction to proven reads.

The exec policy, the write policy and the browser host are built when the table is assembled at startup. The exec allow
table and the session-approved commands are shared cells, so an approval or a `config/write` change reaches the next call.
`prune_spill` runs once at startup and deletes `<home>/spill/<date>/` directories whose files are all older than 7 days.

## Authorization

`ExecutionPolicy` is implemented in `jeeves-app-server/src/permissions.rs`; each session has a `PermissionMode`
(`readOnly` / `standard` / `fullAccess`), stored per session with a revision.

- The private main conversation uses enabled tools under its session policy. Channel conversations retain their source-scoped tool table.
- **`readOnly`**: refuses every call that is not a proven read.
- **`standard`**: every non-read call asks once through the approval broker (120 s; the preview is redacted for keys such as
  `token`, `password`, `authorization` and capped at 8192 bytes). If it is approved, approval requests the handler itself
  would make during that invocation are answered yes (`ApprovedInvocation`); hard gates still apply.
- **`fullAccess`**: no per-call question from the policy; the tools' own gates and `JEEVES_APPROVAL` still apply.
- **`delegate_task`** is refused outside `fullAccess`.

Proven reads (`proven_read`): `browser_observe`, `read_file`, `glob`, `grep`, `search_web`, `fetch_url`, `read_page`,
`list_schedules`, `find_tools`; plus `memory` with `action` = search / list and `job` with `action` = output / list.
MCP and unknown tools are never proven reads.

`ApprovalBroker::request` returns an `ApprovalOutcome`, and only `Approved` lets the action run. `Denied` (a person said no), `TimedOut` (nobody answered in time) and `Withdrawn` (the turn ended, permissions changed or nobody can answer here) each give the model its own refusal text, so a plain no is not mistaken for a broken approval channel. The request names what
"remember this" would record (`RememberTarget`): an exact command line for `run_command`, a directory for the write tools.

## `memory` and `read_page`

| `action` | Parameters | Behavior |
|---|---|---|
| `save` | `subject`, `content`, optional `kind` | Stores a fact; says where it landed (`state.db` facts table) and reports "Already recorded" for a duplicate |
| `forget` | `subject` | Drops every fact about the subject |
| `search` | `query`, `top_k` (default 4) | Fact search |
| `list` | `limit` (default 10) | Most recent facts |
| `update` / `delete` | `id` (+ `content` for update) | Edit or delete one fact by id |
| `delete_episode` | `id` | Delete an episode |
| `create_skill` | `name`, `description`, `body` | Writes `<home>/skills/<name>/SKILL.md` |

An unknown `action` returns a text listing the options. `create_skill` lowercases the name and turns spaces into `-`,
requires a slug (`is_slug`, shared with `jeeves skill install`), refuses when `SKILL.md` already exists, and validates the
text with `parse_skill_text` before writing. Before the write it asks the human through `ApprovalBroker`, showing the whole draft;
a denial, timeout or absent broker returns a refusal and leaves no file.

`read_page(kind, name)`: `kind=memory` returns a memory-library page with its `sources` (a page that is obsolete or time-bounded
is reported as excluded); `kind=skill` returns the skill body. Details of the bundle and skills are in [05](05-memory.md).

## Files: read, find, write, patch

**`read_file`** (`path`, `offset` 1-based, `limit`): pages of at most 2000 lines / 50 000 bytes, cut on line boundaries,
with `[Use offset=N to continue.]`. Re-reading the same range in the same session returns an "unchanged" placeholder.
`~` is expanded; relative paths resolve against the turn's working directory. A non-UTF-8 file is refused with a pointer.

**`glob`** (`pattern`, `path`): with a pattern, matching paths (`**` crosses directories, `*` and `?` stay within one, a
pattern without `/` matches the file name at any depth); without one, lists `path` (directories first, files with sizes,
500 entries). **`grep`** (`pattern`, `path`, `glob`, `ignore_case`): plain-text match, output `path:line: text`,
lines cut at 240 characters, binary files and files over 2 000 000 bytes skipped. Both walk the tree themselves, skip
`.git .hg .svn node_modules target dist build .venv venv vendor .next .turbo .cache .jeeves`, and stop at 200 matches or 20 000 visited
entries with a `[Stopped at …]` note.

**`find_symbol`** (`symbol`, `path`) finds exact declaration names; **`repo_map`** (`path`) lists declarations. Both build an ephemeral SQLite FTS5 index from common source extensions, with the same walk and match limits as `grep`, plus a 32 MiB source budget and 30,000 declaration cap. Rebuilding per call reflects uncommitted edits without writing repository files. This is not a syntax parser: declarations can be missed or misidentified.

**Writable scope** (`roots.rs`, one definition shared by the write tools and the `run_command` sandbox): the turn's working
directory plus `JEEVES_EXEC_WRITABLE_ROOTS`. `<home>` is not a root. Extra roots are dropped when they are the disk root, `<home>`, an ancestor of
`<home>`, or inside a `HOME_CONTROL` entry (`settings.json`, `hooks.json`, `mcp.json`, `models.json`, `secrets.json`,
`connectors.json`, `state.db`, `skills`, `memory`, …); the chosen working directory is honored even if it is `<home>`, except the disk root.

**Write gate** (`fs_write.rs`): the path, symlinks resolved, must land inside a root, otherwise the write is refused
before any approval; with no working directory chosen nothing is writable. Only then, with `JEEVES_APPROVAL=on-request`, every write
asks once unless its directory was already approved (`write_approved_dirs`, persisted as `JEEVES_WRITE_ALLOW_DIRS`); approval cannot unlock
an out-of-scope path.

- **`write_file`** (`path`, `content`): replaces the file, creates parent directories, at most 1 000 000 bytes, no NUL bytes; identical content reports "nothing was written".
- **`edit`** (`path`, `old_string`, `new_string`): `old_string` must occur exactly once; zero or several matches are refused. Matching runs in tiers and stops at the first that finds anything: exact bytes; then with CRLF, typographic quotes, dashes and special spaces folded to ASCII; then whole lines with trailing whitespace ignored. Indentation is never ignored. A tolerant match is named in the result, and an LF replacement written into a CRLF region takes CRLF. On a miss the error either says `new_string` is already in the file or shows up to 12 numbered lines of the stretch most `old_string` lines agree on (`edit_match.rs`). The result is a change hunk (3 lines of context, at most 40 lines).
- **`apply_patch`** (`patch`, at most 512 KB): `*** Begin Patch` format with `Update File` / `Add File` / `Delete File`; context and removed lines match exactly first, then with `edit`'s per-line tolerance; all hunks are resolved in memory first and nothing is written unless every one matches; `Add File` refuses an existing file; unknown section headers are refused.
- **Stale-file check** (`file_seen.rs`): `read_file` and the three write tools note each file's modification time, size and (up to 8 MiB) content digest per session. Before `edit`, `write_file` or `apply_patch` changes an existing file, a mismatch with that note (an editor, a person or a formatter wrote it since) refuses the write before any approval is asked and tells the model to read the file again. A new timestamp on identical bytes is not a change. A file the session never read or wrote has no note and stays writable.

## `run_command`

Parameters: `command`, `cwd`, `background`. `vet()` returns `Allow`, `Deny(reason)` or `NeedsApproval`, in this order:

1. **Hard-deny list**, not configurable and not subject to approval: substrings (lowercased) `sudo `, `doas `, `mkfs`, `dd if=`, `dd of=`, `shutdown`, `reboot`,
   `halt `, `:(){`, `chmod -r 777 /`, `chmod 777 /`, `chown -r /`, `> /dev/disk`, `> /dev/sd`, `diskutil erase`,
   `launchctl unload`, `systemctl disable`, `rm -rf /*`, `rm -rf ~`, `rm -rf $home`, `eval $(`, `history -c`; plus
   `pipes_into_a_shell`: a `curl`/`wget` segment piped into `sh bash zsh dash python python3 perl ruby`.
2. **Allow table** (`JEEVES_EXEC_ALLOW`, at most 64 rules): `*` allows everything, a trailing `*` is a prefix match, anything else is whole-command equality. Commands approved "for this session" match exactly. An empty table
   allows nothing. On no match, `JEEVES_APPROVAL=never` (default) denies with the current rules in the message, and `on-request` asks (`JEEVES_APPROVAL_TIMEOUT`, default 120 s). Without a broker the refusal says nobody can be asked; a "no" says the user denied it and not to ask again unless the user asks again.
3. **Sandbox**: measured, not assumed. macOS needs `/usr/bin/sandbox-exec`; Linux needs `bwrap` and a successful probe run of `bwrap … /bin/true` (so an installed but AppArmor-blocked `bwrap` does not count). With no usable sandbox the command is refused.

`pass_gate` (gates 1–3 plus approval) and `spawn_sandboxed` are shared by foreground and background runs.
The sandbox allows writes only under the writable roots (the call's `cwd`, which must lie inside the turn's working directory when one is bound, plus `JEEVES_EXEC_WRITABLE_ROOTS`),
`/tmp`, `/private/tmp` and the temp directory. The network is cut unless `JEEVES_EXEC_NETWORK=1` (seatbelt `(deny network*)`, bwrap `--unshare-net`).
Roots are canonicalized before they go into the rules.
Foreground commands and background jobs start in the resolved working directory, so relative output paths are relative to that directory.

Execution: stdin is null, stdout and stderr are read concurrently under `JEEVES_EXEC_TIMEOUT` (default 30 s, 1–3600), stdout is streamed as deltas to an observer when one is watching.
The result includes the exit code and stdout. A non-zero exit marks the call failed. Non-empty stderr appears under `--- stderr ---`.
A timeout, unreadable pipe or missing exit code returns `Error: … outcome is unknown. Reconcile any committed effects …` and notifies the host (`on_uncertain`).
Output over 8000 characters is replaced by a stub of whole head and tail lines; the full text is first written to
`<home>/spill/<date>/command-<time>-<pid>.txt` and the stub names the path. A failed spill leaves the stub without a path.
The same stub logic serves the in-turn tool-result budget ([configuration.md](../configuration.md#per-turn-tool-result-budget)).

## Background jobs

`run_command` with `background=true` passes the same gates, then `JobRegistry::spawn_guarded` starts the job and returns an id.
The registry (`jeeves-app-server/src/jobs.rs`, backed by the state database) keeps a 64 KiB ring of output per job.

The `job` tool: `action=output` (`id`, `cursor`, `wait`), `action=list`, `action=kill` (`id`). Reads continue from an absolute `cursor`; when the ring
overflowed the reply says how many bytes were dropped; `wait=true` waits at most 20 s and then reports the job as still running.
Only the session that started a job can read, list or stop it; another session sees no such job. Output is scrubbed of vault values.
When a job ends, the ring's contents are written to `<home>/outbox/jobs/<pid>-<id>.txt`, the exit is logged on stderr, and a notification is sent when a notifier is configured.

## Credential vault

Secrets are stored by name (uppercase letters, digits, `_`, at most 40 characters, at most 50 names); values live in the
secret store (OS keychain first), `<home>/vault.json` holds names only. At each spawn `spawn_sandboxed` adds every value to that one process as
`JEEVES_SECRET_<NAME>`, foreground and background alike, and `scrub` replaces exact occurrences of each value (4 characters or longer) in the output with `«NAME»`.
Values are read at spawn and scrub time, so a secret added in Settings works on the next command. When `run_command` is enabled the prompt lists the names only
(`$JEEVES_SECRET_<NAME>`), never values.

## Web

**`fetch_url`** (`url`): http/https only; every resolved address must be public (loopback, private, link-local and reserved ranges are refused);
redirects are followed by hand, at most 5, re-checked on every hop; timeout 20 s; download capped at 2 MiB and text at 50 000 characters, with a truncation note.
HTML is converted to text by the module's own converter. The body is wrapped by `report::as_external` as data, not instructions.
For current offers, the tool guidance asks the model to verify amount, currency, item, local date, and relevant scope in the same listing or clearly scoped section. It distinguishes indicative prices and schedules from verified checkout totals and availability; it is not a structured quote validator.

**`search_web`** (`query`, `max_results` default 5): each turn may make at most `JEEVES_SEARCH_MAX_PER_TURN` calls (default 20, clamped 1–200).
For an approximate schedule time, opt in with `approximate_time` matching exactly one literal `HH:MM` in the dated route query. Jeeves keeps the original query and adds one timetable query that replaces only the clock time with a 30-minute window on either side, clipped to the same day; this discovers candidates, not booking or seat-map confirmation. Omit the field for strict departure bounds.

For the owner-chat shortcut for already-booked flights and seat advice, a unique nearby lead with one flight number, direction, and departure time together can trigger one targeted flight-number and exact-date search through the ordinary search budget. The refinement discovers evidence; neither an index excerpt nor an undated timetable confirms the actual flight or aircraft layout.
For that shortcut, if the first search succeeds without a coherent nearby lead, the one host refinement instead searches English-language schedules for the same window. A dated FlightStats link supplied by the user is read directly. If a successfully read tracker for a different day has one matching dated navigation row, the runtime follows its exact link and re-checks the main status. The built-in HTML reader includes visible same-origin destinations next to their labels, preserving query parameters without following them or bypassing public-URL checks.
In the owner’s main chat, a narrow Chinese already-booked flight and seat question with one clear date, route and remembered time can trigger this public lookup before the model runs. Only those query terms are sent. The runtime does not pre-search if a strict time constraint or a no-search instruction is detected.
The shortcut publishes no model draft. It can name a candidate only when a successful fetch of a dated FlightStats tracker returns a main status record whose flight number, direction, local date, scheduled departure (within 30 minutes), and linked detail record ID agree. Old status, navigation, search excerpts, and failed fetches cannot confirm the candidate. If no single record qualifies, the reply says it could not verify the flight. The reply gives only generic seat-map guidance and does not claim to have identified the user’s booking.
In production app-server first tries the Parallel search MCP handler for `search_web` and most `fetch_url` calls (connected on first use; a failure cools it down for 60 s and the built-in path answers). FlightStats tracker pages use the same built-in HTTP path directly to preserve exact same-origin links, with unchanged public URL, DNS, redirect and TLS checks.
The built-in path uses Tavily (`TAVILY_API_KEY` or `JEEVES_SEARCH_API_KEY`) and Brave (`BRAVE_API_KEY`) when keyed; with no key it uses DuckDuckGo and then Bing (`JEEVES_SEARCH_DDG=0` / `JEEVES_SEARCH_BING=0` turn them off).
When no backend is usable the tool is not registered and stderr says why. Results are wrapped by `report::as_search_results`.
Configuration is in [configuration.md](../configuration.md#search-and-evaluation).

## Messages and scheduling

**`send_message`** (`to`, `body`) writes `<home>/outbox/<UTC %Y%m%dT%H%M%S>-<to>.txt` containing `To: …` and the body; `to` is reduced to alphanumerics (others become `-`) and 40 characters.
It makes no network call; the reply says nothing was sent.

**`schedule_job`** (`name`, `cron`, `prompt`, `delivery` = outbox / notify / quiet, `catch_up`), **`list_schedules`**, **`cancel_schedule`** (`name`) share the validation and
cron semantics of `jeeves_agent::schedule` / `cron` with the runtime that fires the jobs. Names are lowercase letters, digits and hyphens (at most 64) and must not collide with a skill's `schedule:` line; the reply states the next run and that the job fires only while the Runtime is up.
A job declared by a skill cannot be cancelled with the tool.
`run_at` (local `YYYY-MM-DD HH:MM`, instead of `cron`) makes a one-shot job: it is refused in the past or more than a year ahead, its stored cron pins the same minute for display, and the scheduler (`scheduler/once.rs`) fires it once and removes the row; the cron paths skip it.

## To-do list

`todo_write` takes `todos`, each `{content, status}` with status `pending` / `in_progress` / `completed` / `cancelled`; omitting `todos` reads the list back.
Each call replaces the whole list. At most 20 items, 4000 characters per item, at most one `in_progress`; a violation changes nothing.
The list is kept per session by the server and injected into the system prompt every turn rather than stored in history.

## Subagents

`delegate_task` takes `task` or `tasks` (up to 4, `tasks` wins) and `result_schema`.
The runner (`jeeves-app-server/src/subagent.rs`) gives the child the parent's current enabled table minus `delegate_task` and `todo_write`, no approval channel (anything needing approval is denied),
no memory and no project instructions. A schema result is validated and retried once, then falls back to prose. Output is wrapped by `report::as_report`, which escapes instruction-shaped fragments.
A batch (`tasks`) runs the children concurrently, each with `readonly()` applied, because the parent's barrier cannot order four independent loops; a single task keeps the full table.
Results come back in the order asked; one failed child does not fail the batch.

## Deferred loading

`JEEVES_TOOL_POLICY=eager` (default) sends every schema; `deferred` hides all tools not in `ALWAYS_ON` (`lib.rs`: `run_command`, `write_file`, `edit`, `apply_patch`, `delegate_task`, `todo_write`,
the browser tools, `find_tools`, `memory`, `read_page`, `read_file`, `glob`, `grep` and the scheduling tools) until activated. `find_tools(query)` activates hidden tools whose name or description contains
any word of the query (if none match, the whole hidden shelf), and they appear from the next round. The activation set is per session and held in memory. Hiding a schema changes only what is sent; it grants nothing,
and the switches and approval still apply. There is no tool that calls another tool by name.

## MCP tools

`jeeves-mcp` turns configured servers' tools into registry tools named `{server}_{tool}`, sanitized to `[a-zA-Z0-9_-]` and 64 characters; collisions get `_2`, `_3` and a stderr line.
They are registered exclusive. Each connection has a circuit breaker: after 3 consecutive failures calls return an explanation for 60 s, then one probe is let through.
A transport failure reconnects for later calls and never replays the call that failed. Configuration is in [configuration.md](../configuration.md).
