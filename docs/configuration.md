# Configuration reference

English | [简体中文](configuration.zh.md)

## Two layers

The cockpit menu provides account, device and service connection management. Operators configure models, runtime limits and tool permissions through the CLI or Runtime management interfaces.

Environment variables supply startup defaults. `jeeves-config`
reads them once at process start and never again. Jeeves **does not read any
`.env` file** (only the Lark gateway entry point does, see below) — source one yourself before launching
(`set -a; source .env; set +a`). Runtime changes go through `config/write`
(persisted to `<home>/settings.json`, surviving restarts).

## Model profiles and their keys (`<home>/models.json`)

Providers are **pre-adapted in code** (`PROVIDERS` in `jeeves-provider`; the list is in the
`JEEVES_PROVIDER` row below); all a user does is pick one and supply a key:

```
/model                       list configured profiles (★ marks the active one)
/model add ds deepseek deepseek-flash --env DEEPSEEK_API_KEY
/model use ds                switch (no need to retype the key)
```

Use `/model` in the REPL to manage profiles in `<home>/models.json`. Management clients use the `model/*` protocol. Keys go to the OS keychain first, falling back to
`<home>/secrets.json` (0600); `--env VAR` reads the environment and stores
nothing. `model/save` reports **where the key actually landed** (`keychain` /
`file` / `env:VAR`). Switching takes effect immediately: it rewrites Settings and
re-resolves the provider. Removing the active profile restores the model configuration
from startup. A custom endpoint uses a provider from the model catalog so the runtime
knows its wire protocol. Repair unreadable profile, settings or vault files before
saving changes. Startup can still use its defaults.

**Can this model see images? Declare it when you add the profile**, instead of
being told by an error later:

```
/model add ds deepseek deepseek-v4.1-flash --env DEEPSEEK_API_KEY --vision
```

- no flag: follow the vendor's catalog row (the vendor's **default** model);
- `--vision`: this model sees images even though the vendor's default does not;
- `--no-vision`: this model is text-only even though the vendor's default sees.

When one vendor ships both kinds (`deepseek-v4.1-flash` beside
`deepseek-v4-pro`), the profile is the only layer that can be right. The file
form is `"vision": true` on that profile in `models.json`; to change the
**whole vendor**, edit `models-catalog.json` instead (see "The model catalog").

**Only a human can change any of this**: the model has no `model/*` tool and
never sees a key.

When you replace a stored model key, the previous key stays active until Jeeves saves the updated profile. If the runtime stops mid-change, it reads a local record on startup or on the next profile edit and removes the unused key. An interrupted deletion resumes the same way. If the credential service is still busy or refuses deletion, Jeeves reports the failure and keeps the record for retry. The record contains only profile and storage references. A keychain error reports the operation status without returning the helper’s output or the credential.

## Model routing (`<home>/model-routes.json`)

Instead of picking the model yourself, let the server pick it per turn —
a tier abstraction ("You don't pick the model, the task does"):

```json
{
  "mode": "auto",
  "routes": { "reasoning": "ds-r1" }
}
```

- `mode: "fixed"` (the default — a missing or broken file also means fixed)
  keeps the active profile for every turn; routing never intervenes.
- `mode: "auto"` picks a **tier** per turn, then maps tier → profile id:
  token estimate over half the chat profile's context window → `reasoning`;
  otherwise `chat`. Messages in the ambiguous band (≥2k tokens, under half
  the window) additionally ask the cheap small-model once ("is this deep
  reasoning?"); any failure or vague answer falls back to the L0 answer
  (fail-open).
- Missing tier entries fall through a default chain: `reasoning`/`fast` →
  `chat` → the active profile. A route pointing at a deleted profile falls
  back to the active profile and says so in `TurnMeta.routeReason`.
- `JEEVES_MODEL_ROUTE=auto|fixed` forces the mode from the environment
  (for CI / unattended hosts where the home file is out of reach).
- Editing: REPL `/route`, `/route auto|fixed`, `/route set <tier> <id>`
  (the file is re-read every turn — changes take effect immediately).
- Routing is a **server-side policy**: the model has no tool to switch its
  own model, and the human's active profile always wins.

Routing is transparent about itself: `TurnMeta.routeReason` records
`auto:reasoning → ds-r1` or `fallback: …` whenever a choice or a fallback
actually happened; the dashboard footer prints it.

### Fallback models

`"fallback": ["profile-id", …]` in the same file lists backup profiles for the main
conversation model, tried in order. It works in both `fixed` and `auto` mode.

- A call moves to the next profile only after the current provider's own retries give up,
  and only on errors another provider can fix: HTTP 429, 5xx, 401, 402 or 403, a network
  failure, or a stream that broke before any text was shown. Context overflow and other
  request errors are reported as before.
- Once text has reached the person, the turn never switches, because a second answer would
  print after the first.
- Each switch sends a `turn/retry` notification whose reason starts with
  `switching to fallback <id>`. Later calls in the same turn start at the profile that
  answered; the next turn tries the main profile again.
- A request with an image skips backups whose profile cannot see images.
- Usage is recorded under the provider and model that answered.
- Only the main conversation model falls back. Routing, summaries, memory and screen
  calls keep their own model.
- Unknown ids and profiles that repeat the model already in use are skipped with a line on
  stderr. Saving the tier table (`/route`, `model/routes/save`) keeps the list.

### A task can declare its tier

`task/create` and `turn/start` both accept `tier` (`fast` / `chat` / `reasoning`):

- **A declaration beats the mode**: even with `mode: "fixed"`, a task or a
  caller saying `reasoning` runs this turn on the reasoning profile — fixed is
  the default policy, not a veto;
- A task's declared tier is stored on the task row and every round of that task
  reuses it — no need to repeat it per turn;
- A mistyped tier is an error on the spot, never a silent `chat` (that would put
  the bill on the wrong model).

## Inbound webhooks (`<home>/webhooks.json`)

Another system knocks, Jeeves runs one turn (CI failure, form submitted, payment
landed…). Without this file there is **no door at all** (404), and the model
never learns the hook exists:

```json
{"hooks": [{
  "name": "deploy",
  "secret": "env:JEEVES_HOOK_SECRET",
  "prompt": "Production is unhappy — handle this event:\n{{payload}}",
  "tier": "reasoning",
  "signature": true,
  "limit": 6,
  "callback": "https://ci.example.com/jeeves/hook-result"
}]}
```

- `POST /api/hook/<name>`; the `X-Jeeves-Secret` header must match `secret`
  (`env:VAR` is read per request; unreadable = refuse, never degrade to an
  empty string). Wrong or missing is always 401;
- This route sits **outside** the dashboard token layer on purpose: the calling
  machine has never heard of `JEEVES_OPS_TOKEN`; what it proves is that it knows
  this hook's secret. In return a hook can only **hand over one message** — it
  cannot read state or start a second turn;
- Unknown name, `enabled: false`, no config file — all three are 404 (the door
  does not exist, and its existence is not revealed);
- `{{payload}}` is replaced by the raw request body; without the placeholder the
  body is appended to the prompt (silently dropping a body would be worse);
- The turn is still an ordinary turn: tools keep their approval cards, and
  nobody is at the keyboard — destructive tools time out and are denied.
  A webhook adds no capability;
- The answer is a `RunResponse` (`turnId` / `reply` / `model` / `provider` /
  `routeReason`) — the same shape as the task API (pushed to the callback in async
  mode).

Set `RUNTIME_ORIGIN` in the following examples to the private Runtime origin. On a source
installation started with `just local-up`, use the assigned loopback port from
`<home>/runtime-endpoint.json`. On Kubernetes, use the tenant's private Runtime Service.

For a signed connector feeding recurring work, declare `event_type` instead of the prompt,
enable `signature` and use `event_fields` to declare the names and types made available for
review and filtering. Supported types are `string`, `integer`, `boolean` and `stringArray`.
For example, a connector can publish a message reference and a list of attachment media types.
The signature uses the same headers. The JSON body contains `eventId`, `occurredAt` (ISO time
with offset), `previousCursor` (null for the first event), `cursor` and `fields`.
A 202 acknowledgement is sent after the inbox and cursor transaction. A 409 means the
producer must check its previous cursor or event identity. Downstream callbacks and prompt
execution are unavailable in event mode.

### Three optional fences

- **`signature: true` — no replays.** The request must carry `X-Jeeves-Timestamp`
  (epoch seconds) and `X-Jeeves-Signature` =
  `hex(HMAC-SHA256(secret, "<timestamp>.<body>"))`. A timestamp more than ±300s
  away from our clock → 401; the same signature seen twice inside the window →
  409. The other side verifies with tools it already has:

  ```bash
  BODY='{"status":"failed"}'; TS=$(date +%s)
  SIG=$(printf '%s' "$TS.$BODY" | openssl dgst -sha256 -hmac "$JEEVES_HOOK_SECRET" -r | cut -d' ' -f1)
  curl -X POST "${RUNTIME_ORIGIN}/api/hook/deploy" \
    -H "x-jeeves-timestamp: $TS" -H "x-jeeves-signature: $SIG" -d "$BODY"
  ```

  **Retries must be re-signed** (a new timestamp): each signature is accepted once.
  Without `signature`, plain `X-Jeeves-Secret` still works — it proves "knows the
  secret" and **cannot refuse a replay** (the same request runs a second turn).
  That difference is why the signature exists.
- **`limit: N` — rate.** At most N accepted requests per rolling minute per hook;
  only requests that pass auth are counted (guessing secrets does not burn the
  budget), and going over answers 429 + `Retry-After` seconds. The counter is
  per-process: a restart clears it, multiple processes do not share it.
  `limit: 0` is refused at load ("configured but can never pass" is a typo, not
  a policy).
- **`callback: "https://…"` — async.** The caller gets `202` +
  `{"accepted":true,"hook":…}` at once; the turn runs in the
  background and its result is POSTed to that URL:
  `{"hook","ok":true,"result":RunResponse}` or
  `{"hook","ok":false,"error":{code,message}}`. Three attempts (1s and
  2s apart), **no queue**: if all three fail (or the process dies first) that
  delivery is lost — the answer itself is in the session history and is not gone.
  A callback must be an http(s) URL, or loading fails loudly.

## Task API (`POST /api/run`)

"One turn in, one JSON answer out" for programs: the body is `TurnStartParams`
(`TurnStartParams`), no streaming, and it returns when the turn ends:

```bash
curl -sS -X POST "${RUNTIME_ORIGIN}/api/run" \
  -H "Authorization: Bearer $JEEVES_OPS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"message":"Summarise yesterday in three points","tier":"reasoning"}'
```

- The dashboard keeps SSE (a person wants to watch the words arrive); this
  endpoint is for CI / cron / another agent;
- It takes the same Bearer: it is part of the private Runtime API;
- `model` and `routeReason` are the visible proof that the tier pin took effect.

## Invalid values fail at startup

`Settings::validate` runs once, **after** the environment and
`<home>/settings.json` are merged, and a bad value aborts the process with the
variable named in the message — a misconfiguration should not quietly become a
default that only surfaces later as strange behaviour. The same bounds table
(`BOUNDS` in `jeeves-config`) validates `config/write` patches, so the two entry
points cannot disagree.

| Variable | Range |
|---|---|
| `JEEVES_MAX_TOKENS` | 0 or 128 – 200000 |
| `JEEVES_HISTORY_TURNS` | 0 – 1000 |
| `JEEVES_CONTEXT_WINDOW` | 1024 – 10000000 |
| `JEEVES_COMPACT_THRESHOLD` | 0.05 – 0.95 |
| `JEEVES_TOOL_RESULT_TOTAL_CHARS` | 0 – 4000000 |
| `JEEVES_TOOL_RESULT_MAX_CHARS` | 0 – 1000000 |
| `JEEVES_LEDGER_INLINE_MAX` | 1024 – 1048576 |
| `JEEVES_MAX_TURNS` | 0 – 64 |
| `JEEVES_TASK_WORKERS` | 1 – 8 |
| `JEEVES_HOME_MAX_MB` | 0 – 10000000 |
| `JEEVES_TOOL_RESULT_KEEP_RECENT` | 0 – 50 |
| `JEEVES_TURN_TOKEN_BUDGET` | 0 – 2000000 |
| `JEEVES_RESUME_MAX_AGE` | 0 – 10080 |
| `JEEVES_CONSOLIDATE_EVERY` | 1 – 1000 |
| `JEEVES_RETRIEVAL_TOP_K` | 1 – 100 |
| `JEEVES_LLM_TIMEOUT` | 1 – 3600 |
| `JEEVES_LLM_RETRIES` | 0 – 5 |
| `JEEVES_THINKING_BUDGET` | 0 – 200000 |
| `JEEVES_GOAL_MAX_ROUNDS` | 1 – 20 |
| `JEEVES_EXEC_TIMEOUT` | 1 – 3600 |
| `JEEVES_APPROVAL_TIMEOUT` | 5 – 3600 |
| `JEEVES_SKILL_INDEX_TOKENS` | 32 – 2000 |
| `JEEVES_SKILL_BODY_TOKENS` | 64 – 8000 |
| `JEEVES_CONTEXT_WARN_RATIO` | 50 – 99 |

`JEEVES_APPROVAL` must be `never` or `on-request`, `JEEVES_SOUL_MODE` one of `auto` / `thin` / `full`, `JEEVES_TOOL_POLICY` one of `eager` / `deferred`. `JEEVES_EXEC_ALLOW` is checked too: at most 64 rules, each non-empty, no newlines,
at most 200 characters. An **empty** allowlist is valid — it means "allow
nothing", which is the default.

## Core variables

| Variable | Default | Purpose |
|---|---|---|
| `JEEVES_HOME` | `./.jeeves` | state directory: state.db, SOUL.md, skills/, traces/, outbox/, mcp.json, settings.json |
| `JEEVES_PROVIDER` | `anthropic` | provider: anthropic / openai / deepseek / gemini / kimi / glm / minimax / xai / openrouter / opencode_zen / opencode_go / ollama. **Only** the fallback when no profile is configured (that is how local models start zero-config); models and keys live in `<home>/models.json` |
| `JEEVES_LLM_TIMEOUT` | `120` | per-call model timeout in seconds |
| `JEEVES_LLM_RETRIES` | `2` | retries on 429/5xx/network hiccups — exponential backoff (500ms, capped at 8s per wait, 30s total), **announced every time**, never a different provider; `0` disables |
| `JEEVES_THINKING_BUDGET` | `0` | Anthropic Messages thinking for the main model. `0` uses the model default. A value from 1024 to below the request output allowance sets a manual budget on supported models or enables adaptive thinking on Claude 5.5 and MiniMax M3 models. Thinking counts as output. |
| `JEEVES_GOAL_MAX_ROUNDS` | `5` | how many rounds a goal may continue for; on overrun the status is `round-limit` and the miss is reported honestly |

## Behavior knobs

| Variable | Default | Purpose |
|---|---|---|
| `JEEVES_MAX_TOKENS` | `0` | optional ceiling on the automatic output allowance. `0` uses the [model catalog](#the-model-catalog-models-catalogjson) and available context |
| `JEEVES_HISTORY_TURNS` | `12` | working-memory window **ceiling**: only the last N turns enter the prompt (older turns are folded into a rolling summary, not dropped) |
| `JEEVES_CONTEXT_WINDOW` | provider default | override the context-window estimate (local models differ wildly; the table holds common defaults) |
| `JEEVES_COMPACT_THRESHOLD` | `0.8` | start compacting at this fraction of the window — tokens are the real gate, turns are the ceiling |
| `JEEVES_TOOL_RESULT_TOTAL_CHARS` | `200000` | per-turn cap on the total size of tool results; over it the **oldest** results are replaced with stubs, newest kept whole (`0` disables) |
| `JEEVES_TOOL_RESULT_MAX_CHARS` | `30000` | a single result must exceed this to be stubbed (`0` disables) |
| `JEEVES_LEDGER_INLINE_MAX` | `32768` | ledger payloads above this (bytes) are written under `<home>/ledger/` and the row keeps a pointer plus digest — one turn should stay one readable row |
| `JEEVES_MAX_TURNS` | `0` | per-tenant cap on turns running at once across all sessions; a turn over the cap is refused with an error, never queued (`0` = no cap) |
| `JEEVES_TASK_WORKERS` | `1` | how many human-started tasks run side by side; each holds a lease, see [task workers](operations.md#task-workers-and-leases) |
| `JEEVES_HOME_MAX_MB` | `0` | MB; once the home directory is this large new turns are refused, while reads, export and cleanup keep working (`0` = no cap) |
| `JEEVES_TOOL_RESULT_KEEP_RECENT` | `3` | how many of the newest results are never stubbed (trimming is oldest-first) |
| `JEEVES_TURN_TOKEN_BUDGET` | `0` | Token budget for one turn. As measured usage plus the next estimated input approaches the budget, the model can report progress without tools. `0` disables the budget |
| `JEEVES_RESUME_MAX_AGE` | `60` | minutes; an `open` turn checkpoint older than this is marked `abandoned` at startup instead of resumed (`0` = always resume) |
| `JEEVES_CONSOLIDATE_EVERY` | `6` | run consolidation every N new turns |
| `JEEVES_RETRIEVAL_TOP_K` | `4` | maximum matching facts per turn |
| `JEEVES_SKILL_DIRS` | — | colon-separated extra skill directories (besides `<home>/skills`); their skills are built-in in the dashboard's Settings (greyed out, read-only) |
| `JEEVES_MCP_CONFIG` | — | colon-separated extra mcp.json files (besides `<home>/mcp.json`); their servers are built-in in Settings (greyed out, read-only) — the user's file wins on a same-named server |

## Prompt shape and the context meter

| Variable | Default | Purpose |
|---|---|---|
| `JEEVES_SOUL_MODE` | `auto` | `thin` / `full` / `auto`: how much built-in scaffolding wraps `SOUL.md` |
| `JEEVES_SKILL_INDEX_TOKENS` | `200` | cap on the always-present skill catalog (name plus one line each) |
| `JEEVES_SKILL_BODY_TOKENS` | `1200` | cap on one selected skill's body; beyond it, `read_page` returns the rest |
| `JEEVES_TOOL_POLICY` | `eager` | `deferred` keeps only the always-on schemas in the request; `find_tools` opens the rest |
| `JEEVES_CONTEXT_WARN_RATIO` | `85` | percentage of the window at which the meter shows a notice |

The system prompt is two halves: a **stable prefix** (soul plus model identity) and a
per-turn tail (day-granular clock, todo list, memory hits, skill catalog, rolling summary).
Anthropic gets an explicit `cache_control` boundary between them; the wires with automatic
prefix caching only need the prefix not to move. Token accounting records
`cache_read_input_tokens` and `cache_creation_input_tokens`, so the hit rate is a number
rather than a belief.

`thin` and `full` only decide which built-in text **seeds** `SOUL.md` and which an
**untouched** default becomes — a soul the user edited is returned as written, in either
mode. `deferred` never hides a tool a human switched on: `run_command`, `write_file`, `edit`
and `delegate_task` stay in the table, because a discovery step must not run ahead of
consent. Activations are per session and in memory: a restart asks the model to search
again, which is cheap and honest.

## Per-turn tool-result budget

History has a sliding window and a token budget; the turn itself did not. `run_command`
caps itself at 8000 characters and `search_web` caps each snippet at 400, but **MCP tools answer to
nobody** — ten calls returning 50k characters each put half a million characters into a
single request.

When the total size of a turn's tool results exceeds `JEEVES_TOOL_RESULT_TOTAL_CHARS`, the
oldest results are replaced with a stub (newest kept whole): the full text goes to `<home>/spill/<date>/`,
and the context keeps the head and tail — **complete lines only** — plus
`…（结果共 N 字符，已截断，省略了 M 行；完整输出在 spill/…）`.

* Only results above `JEEVES_TOOL_RESULT_MAX_CHARS` are touched. A hundred medium results
  overflowing the cap cost more in files than the context they save, so that case is
  logged on stderr and left alone.
* Never half a line — half a JSON object reads worse than one line fewer.
* A failed spill still stubs (without promising a path it does not have) and never turns
  a successful call into an error.
* Tools that budget themselves (`run_command`) are skipped; the pass is idempotent.

## Running commands

| Variable | Default | Purpose |
|---|---|---|
| `JEEVES_EXEC` | `0` | enable the `run_command` tool (off = the model never sees it). A switch saved with `config/write` overrides the variable and applies from the next request |
| `JEEVES_WRITE` | `0` (`1` in the dashboard) | enable the `write_file` / `edit` / `apply_patch` tools (off = the model never sees them). Writable boundary: **only** this turn's chosen working directory plus `JEEVES_EXEC_WRITABLE_ROOTS`; with no folder chosen nothing can be written |
| `JEEVES_DELEGATE` | `0` | enable the `delegate_task` tool (a subagent gets its own context; it cannot delegate again). Also a Settings switch, same override rule |
| `JEEVES_EXEC_ALLOW` | — | allowlist, comma separated, trailing `*` wildcards (`cargo test,git status,ls *`). Empty = deny everything |
| `JEEVES_EXEC_TIMEOUT` | `30` | per-command timeout in seconds |
| `JEEVES_APPROVAL` | `never` (`on-request` in the dashboard) | what to do when the allowlist does not match: `never` = refuse, `on-request` = ask (no answer = refusal). Never applies to the deny list or the sandbox |
| `JEEVES_WEB_PUSH_CONTACT` | unset | VAPID contact URI (operator mailto or HTTPS URL). Enables enrollment for [task approval notifications](protocol.md#task-approval-notifications). |
| `JEEVES_APPROVAL_TIMEOUT` | `120` | seconds to wait for an answer before treating it as a refusal |
| `JEEVES_EXEC_NETWORK` | `0` | let sandboxed commands reach the network. **Off by default** — an allowed command should not be able to send your data out |
| `JEEVES_EXEC_WRITABLE_ROOTS` | — | extra writable directories (colon separated, must be existing absolute paths), e.g. a build cache |
| `JEEVES_WRITE_ALLOW_DIRS` | — | directories whose writes need no confirmation (colon separated, absolute). A **"don't ask again" list, not a writable root**: outside the boundary nothing lands |

A task created with `repo` or `parentTaskId` restricts file changes, delegated writes, sandbox commands and command checks to its managed Git worktree. Put writable build caches inside the worktree. Sandboxed task commands use it for temporary files. Ordinary sessions retain the shared roots configured above.

Commands always run sandboxed (macOS `sandbox-exec` / Linux `bubblewrap`)
with writes confined to the working directory, the Jeeves home and temp, and
**no network** unless `JEEVES_EXEC_NETWORK=1`; a
machine without a sandbox refuses to run anything. The hard deny list
(`sudo`, `mkfs`, download-piped-into-shell, …) is not configurable. See
[SECURITY.md](../SECURITY.md).

## Approving a command interactively

By default an unmatched command is simply refused. With `JEEVES_APPROVAL=on-request`
it becomes a question: the turn emits `approvalRequested` (tool, the command
itself, why it was not allowed, the deadline) and **waits**. The REPL prints
`y`/`a`/anything-else, the dashboard renders a small confirm bar; both send
`approval/respond`. Silence is a refusal — timeouts, a late answer and a
missing UI all end the same way.

`remember` says how long the yes lasts: `once` (default, this time only),
`session` (kept in memory, gone on restart), `always` (written into
`settings.json`). In the terminal `y`/`s`/`a` pick the three; the dashboard card
offers Deny / Allow once / Always allow this session / Remember this. What is recorded is exactly
what was approved — the command line, no wildcard; for a write, the **directory**
(never the file name). It takes effect immediately, and it never outranks
hard-deny, the sandbox, or the writable boundary.

## Writing files (`JEEVES_WRITE`)

`JEEVES_WRITE=1` registers three tools: `write_file` (whole-file write, overwrites),
`edit` (one exact-match replacement — `old_string` must appear exactly
once, 0 or several matches are refused, and the reply carries a change hunk) and
`apply_patch` (several files and hunks, all or nothing).
The terminal and a bare `jeeves app-server` keep the conservative off-by-default; the **dashboard
defaults to on**, because a human is watching and can approve (explicit
`JEEVES_WRITE` / `JEEVES_APPROVAL` environment variables set the default; a switch saved in Settings overrides `JEEVES_WRITE`).

In the dashboard the working directory is a folder-picker button, not a text
field. File tools require a selected workspace. Ordinary sessions can also write under configured extra roots. A managed coding task stays inside its worktree. Neither the process directory nor Jeeves home is implicitly selected.
Outside the boundary the write is refused before approval is ever consulted.
Content must be UTF-8 text (no NUL bytes) and at most 1,000,000 bytes. The dashboard also
defaults `JEEVES_APPROVAL=on-request`: writes outside previously approved directories request a decision (allow / allow and remember / deny; no answer times out as denial).

## Task execution plans (`JEEVES_PLAN`)

With `JEEVES_PLAN=1`, a long task spends **one extra model call before its first round** asking
for a plan: 3–6 short, actionable steps whose completion can be seen. The plan is stored as
`task_steps` rows (`kind = "plan"`, empty `run_id` — a plan outlives the execution that drew
it) and appended to that round's message as plain text. A failed round may revise the plan
**once** (two versions max: rewriting after every failure is thrash, not progress).

**Off by default**: it is one more model call and it changes the prompt — a switch pressed by
hand. A plan never carries acceptance conditions: those are the human's facts, and a model must
not be able to soften them by re-planning.

## The model catalog (`models-catalog.json`)

The built-in providers ship in the binary (`jeeves-provider`). To add one — or to
retune a built-in's defaults — without touching code, write
`<home>/models-catalog.json`: a JSON array, each row either **patches an
existing id** (any subset of fields) or **adds a new provider** (`wire`,
`model` required; `key_env` empty or absent = keyless local endpoint;
`context_window` defaults to 32768). Example:

```json
[
  { "id": "deepseek", "context_window": 999999 },
  { "id": "mybox", "wire": "openai", "model": "qwen3.5:9b",
    "base_url": "http://127.0.0.1:11434/v1",
    "model_output_limits": { "qwen3.5:9b": 8192 } }
]
```

`model_output_limits` maps exact model IDs to output token ceilings for that provider. The bundled output table comes from the models.dev snapshot in OpenCode. The Runtime uses 32,000 tokens when an exact entry is absent and caps the catalog ceiling at 256,000. Conversation, task and subagent requests count the projected input, including the system prompt and tool schemas, before fitting output to the available window. The estimate has a 15% input margin and leaves 1% window headroom. When the estimate leaves little space, the OpenCode policy uses a 1,024-token floor, still subject to catalog and explicit budget caps.

The file is read **once at boot** and merged into the catalog; `/model` and
`model/list` see the merged result, and
`model/save` validates against it. Unknown fields are rejected (a typo that
silently does nothing is worse than an error); a broken file or row costs one
stderr line and keeps the built-in table — it never blocks boot. Restart the
process after editing. Note the embedding endpoint (hybrid retrieval) still
selects from the built-in table only.

To skip hand-writing the JSON row, use the generator script (stdlib only):

```bash
python3 scripts/gen_catalog.py --list        # what the upstream offers
python3 scripts/gen_catalog.py glm --model glm-5.3              # built-in id → patch
python3 scripts/gen_catalog.py zhipuai --model glm-5.3 \
  --base-url https://open.bigmodel.cn/api/anthropic \
  --out ~/.jeeves/models-catalog.json                              # new id → new provider, merged in place
```

The data source is [models.dev](https://models.dev) (MIT, maintained by the
OpenCode team): context limits, vision and tool-call flags come from it; the
wire and key_env come from jeeves's mapping table inside the script. The script
picks "the largest-context tool-call model" as the default — that is a starting
point only; the main and small models deserve a human look (candidates are
printed to stderr).

### Built-in default models

These defaults apply when the profile leaves the model ID unset. Pick a model explicitly in your profile to use another version.

| Provider | Main model | Helper model |
| --- | --- | --- |
| [Anthropic](https://platform.claude.com/docs/en/models/overview) | `claude-sonnet-5-5` | `claude-haiku-5-5` |
| [OpenAI](https://developers.openai.com/api/docs/guides/model-selection) | `gpt-6.2-sol` | `gpt-6-luna` |
| [OpenRouter](https://openrouter.ai/collections/free-models) | `nvidia/nemotron-3.5-super-vl:free` | `nvidia/nemotron-3.5-lightning:free` |
| [Gemini](https://aistudio.google.com/docs/models) | `gemini-4-neon` | `gemini-4-hydrogen` |
| [DeepSeek](https://api-docs.deepseek.com/quick_start/pricing/) | `deepseek-v4-pro` | `deepseek-flash` |
| [MiniMax](https://platform.minimax.cn/docs/api-reference/text-anthropic-api) | `MiniMax-M3.1-Flash` | `MiniMax-M3` |
| [Kimi](https://platform.kimi.ai/docs/pricing/chat.md) | `kimi-k3.1-pro` | `kimi-k2.7-code` |
| [GLM](https://docs.z.ai/guides/llm/glm-5.3) | `glm-5.3` | `glm-5.3-flash` |
| [xAI](https://docs.x.ai/developers/models) | `grok-4.7` | `grok-4.3` |
| [OpenCode Zen](https://opencode.ai/docs/console/models/) | `deepseek-v4.1-flash-free` | `deepseek-v4.1-flash-free` |
| [OpenCode Go](https://opencode.ai/docs/console/go/) | `deepseek-v4.1-flash` | `deepseek-v4.1-flash` |
| [Ollama](https://ollama.com/library/qwen3.5/tags) | `qwen3.5:9b` | `qwen3.5:4b` |

OpenRouter and OpenCode Zen use free model IDs. OpenCode Go uses the subscription endpoint. Ollama retains compact models and a 32,768-token Runtime context default.

## Delegating work to a subagent

`JEEVES_DELEGATE=1` adds `delegate_task`, which hands one self-contained job to a
subagent and brings back only the conclusion — useful when the work would
otherwise fill the conversation with searching and reading, or when you want it
done in a fresh context. What the subagent gets:

* an empty history and a short system prompt (soul + "do this one thing"),
  with no retrieval, no skills and no summary;
* the parent's tool table **minus `delegate_task`** — delegating again is not
  refused, it simply is not an option;
* the same model catalog and context-based output policy, plus a separate token budget;
* no interactive approval path and no way to ask you a question.

The conclusion comes back with the tools it used, and failed calls are marked,
so a parent can tell whether the answer rests on something that worked. The
subagent's conversation is not stored. It runs in the same process against the
same state, so nothing is isolated beyond its context.

## Local inference (Ollama)

`JEEVES_PROVIDER=ollama` runs entirely on this machine: no key, no network,
nothing leaves the computer. Install [Ollama](https://ollama.com), then:

```bash
ollama pull qwen3.5:9b     # the main model
ollama pull qwen3.5:4b     # the cheap model for consolidation
JEEVES_PROVIDER=ollama jeeves  # no API key anywhere
```

Ollama exposes an OpenAI-compatible endpoint (`http://127.0.0.1:11434/v1`),
so it reuses the same wire as the cloud providers. LM Studio and vLLM are
the same shape — add a profile pointing at them (`--url` for the endpoint). A profile
also picks a model other than the default:

```
/model add local ollama qwen3.5:27b
```

This is the privacy path and the zero-cost path, and the one that keeps
working when the network (or a key) does not. If Ollama is not running the
error surfaces as a network error — start it with `ollama serve`.

## Per-provider key variables

Jeeves does **not** read these implicitly — a key only comes from the profile you
configured (`--env VAR` declares "read this one"). The names are each vendor's
conventional one, matching `JEEVES_PROVIDER`: `ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, `DEEPSEEK_API_KEY`,
`GEMINI_API_KEY`, `MOONSHOT_API_KEY` (kimi), `ZHIPU_API_KEY` (glm),
`MINIMAX_API_KEY`, `XAI_API_KEY`, `OPENROUTER_API_KEY`,
`OPENCODE_ZEN_API_KEY`, `OPENCODE_GO_API_KEY`.

A missing key answers `PROVIDER_ERROR (-32000)` with where to get one and
which variable to set.

## Search and evaluation

| Variable | Purpose |
|---|---|
| `JEEVES_SEARCH_MCP` | `1` by default: Parallel MCP is the primary search/read service; `0` uses built-in web tools only |
| `TAVILY_API_KEY` / `JEEVES_SEARCH_API_KEY` | Tavily in the built-in fallback (without keys: DuckDuckGo Lite, then Bing HTML) |
| `BRAVE_API_KEY` | second keyed backend in the chain (tried after Tavily) |
| `JEEVES_SEARCH_MAX_PER_TURN` | per-turn `search_web` cap (default 20; values above 200 are clamped, zero or unparsable falls back to 20) |
| `JEEVES_SEARCH_BING` | `0` disables the keyless Bing fallback (default on) |
| `JEEVES_SEARCH_DDG` | `0` disables the keyless DuckDuckGo Lite backend (default on; tried first without keys) |
| `JEEVES_JUDGE_MODEL` | referee model for `jeeves judge` (default: the small model; the referee is not a contestant) |

The production runtime keeps `search_web` and `fetch_url` as its two web entry points.
Both prefer the official Parallel MCP at `https://mcp.parallel.ai/mcp`, exposing only
its search and fetch operations internally. This managed service is separate from
user-configured `mcp.json` servers. It connects on the first web call, not at boot,
and uses anonymous access; it never picks up paid search credentials automatically.
The service has free-tier limits and receives the submitted queries or URLs.

A service attempt is capped at 20 seconds, including connection and discovery;
waiting for a busy connection is also capped at 20 seconds. Connection, tool and
malformed-response failures cool the service down for 60 seconds. Failures visibly
fall back to the built-in tools. The search turn cap is checked before either path
and charged once per logical call; fallback network attempts remain separately bounded.
MCP sources retain URLs, available publication/acquisition dates and the retrieval time.
Search excerpts are capped at 2,000 characters per source; fetch content at 45,000,
with explicit truncation. The final external-data envelope is capped at 50,000.
Fetch targets pass local public-address validation before they are sent to the service;
the remote service owns its own redirects and page extraction. A successful extraction
is not proof that a claim is correct or that the source is current. See the
[Parallel MCP documentation](https://docs.parallel.ai/integrations/mcp/quickstart).

Built-in search backends form an ordered chain: Tavily, then Brave; without any key the
keyless **DuckDuckGo Lite → Bing HTML** pair enters the plan. Errors, verification
pages and empty or unusable result lists advance to the next configured backend.
After a keyed backend fails, the free endpoints stay out of the plan.
When MCP is off and no built-in backend is usable, `search_web` is not registered and startup says
so; search results are wrapped as untrusted external data, same as a fetched
page body. Result entries stay within their own HTML block, tracking links are decoded
to source URLs, and duplicate URLs do not consume result slots. Titles are bounded to
240 characters and snippets to 400 (plus a truncation marker). The model is instructed
to refine irrelevant queries and read sources with `fetch_url` before relying on claims.
Public HTML endpoints can still be blocked or return poor results; these checks do not
establish general search relevance. All-backend failure is reported as a tool error.
For a single positive `site:domain[/path]` term, results must match that host or a
subdomain and path prefix; unrelated or deceptive suffix domains are discarded before
trying the next backend. Complex multi-site expressions retain the engine's semantics.

DDG Lite uses its normal POST search form. Jeeves prefers system `curl` for this public
HTML endpoint when available, with the Rust HTTP client as a fallback before Bing.
This compatibility path runs no shell and accepts no executable or destination from
the model; it disables curl configuration files, bounds response reads to 2 MiB and
the request to 20 seconds, and kills the child on cancellation. If the page still
requires verification, the configured backend chain continues.

## Dashboard and binaries

| Variable | Default | Purpose |
|---|---|---|
| `JEEVES_PORT` | `7777` | Private Runtime port. Both `just local-up` and a Runtime started by the Mac development app set it to `0` for automatic loopback discovery. The Kubernetes example uses `7777` on the private network |
| `JEEVES_NOTIFY` | off | `1` / `true` / `yes` turns it on; then reads `<home>/notify.json` (`{"slack": "webhook URL"}`) and pushes notifications: **approval requests / background-job finish / goal outcomes**. Completion notices persist before delivery and retry after failures/restarts (see [delivery limits](limitations.md#notifications-and-audit)); approval prompts remain best-effort. Network delivery does not block the turn. The URL is admin-configured, so pointing it at a self-hosted intranet endpoint (ntfy/Gotify — anything accepting `{"text": …}`) is legitimate; no SSRF guard applies
| `JEEVES_OPS_HOST` | `127.0.0.1` | Private Runtime bind address. The container uses `0.0.0.0` |
| `JEEVES_OPS_TOKEN` | — | Private bearer credential required on every bind address. Alternatively use an owner-only `<home>/runtime-token` file. The gateway attaches it server-side and browsers use the account session |
| `JEEVES_DISPATCHERS` | — | Comma-separated chat sender ids (Feishu `open_id`) allowed to hand out tasks from a group. A message from anyone else still gets an answer, but without the `task` tool; empty admits nobody. Local views are never affected. Not in `config/read` |
| `JEEVES_BIN` | — | explicit `jeeves` binary path for the TS clients and your own scripts |
| `JEEVES_PERSONAL_HOME` | `~/.local/share/jeeves-personal` | Account and Runtime data directory for `just local-up` |
| `JEEVES_PERSONAL_PORT` | `8743` | Loopback Go gateway port for `just local-up` |
| `JEEVES_PERSONAL_BROWSER` | `1` | Open the browser from `just local-up`. Use `0` for an unattended launch after setup |
| `JEEVES_WEB_GATEWAY_URL` | — | Go gateway URL opened by the Mac development app |
| `JEEVES_DESKTOP_BINARY` | — | read by debug builds of the desktop shell only: absolute path of the `jeeves` binary to start (default `target/debug/jeeves`) |

## Connectors (`<home>/connectors.json`)

"Draft, never send" is the default; a connector is the one exception, fenced three ways:

1. **Off until configured.** No `connectors.json` → no connector tools exist; the model
   cannot even see their names.
2. **Dry run by default.** A configured connector with `"live": false` (the default)
   only writes drafts to `<home>/outbox/`; nothing leaves the machine.
3. **Every live send asks once.** The approval card names the recipient and previews
   the subject and body; refusal/timeout = no send. The allowlist is a hard boundary,
   and **empty means nobody**. Email authorizations are scoped to one account,
   recipient and exact content; a `once` authorization is revoked immediately after use.

```json
{
  "feishu": {
    "app_id": "cli_a1b2c3",
    "app_secret": "env:FEISHU_APP_SECRET",
    "receive_id_type": "open_id",
    "allow_recipients": ["ou_xxx", "oc_yyy"],
    "live": false
  }
}
```

| Field | Meaning |
|---|---|
| `app_secret` | `env:VAR` or a `SecretStore` account (keychain first, 0600 plaintext fallback — reported honestly) |
| `allow_recipients` | exact `open_id` / `chat_id` |
| `live` | only `true` actually sends; flipping it is a human act |
| `receive_id_type` | `open_id` (a person) or `chat_id` (a group) |
| `base_url` | for self-hosted Lark; defaults to `https://open.feishu.cn` |
| `as` | Feishu sending identity: `app` (default) or `user` (needs `jeeves connector feishu login` first) |

The human's own commands (the terminal sends without an approval card — the person
typing is the approval):

```bash
jeeves connector list
jeeves connector feishu --to ou_xxx --text text [--dry-run]
```

The model-side tool is `feishu_send`, registered only when the connector is
configured and its allowlist is non-empty. Unknown keys in `connectors.json` are rejected
(a typo such as `allow_recipents` must not silently mean "nobody").

### Calendar and email

Calendar access is read-only. A CalDAV collection uses REPORT; a URL ending in `.ics`
is fetched directly. Recurrences are expanded with IANA timezones, so weekly events
keep their wall time across daylight-saving changes; all-day dates remain dates.

```json
{
  "calendar": {
    "url": "https://caldav.example.com/calendars/alice/",
    "username": "alice@example.com",
    "password": "env:CALDAV_PASSWORD",
    "timezone": "Asia/Shanghai"
  },
  "email": {
    "username": "alice@example.com",
    "password": "env:IMAP_PASSWORD",
    "from": "Alice <alice@example.com>",
    "imap": {"host": "imap.example.com", "port": 993, "security": "tls"},
    "smtp": {"host": "smtp.example.com", "port": 587, "security": "starttls"},
    "allow_recipients": ["bob@example.com"],
    "live": false
  }
}
```

| Field | Meaning |
|---|---|
| `calendar.url` | CalDAV collection or direct `.ics` URL |
| `calendar.timezone` | IANA display timezone; also interprets floating and all-day starts |
| `email.imap` / `email.smtp` | host, port and `tls` / `starttls` / `plain`; plain is for local fake servers and is never chosen implicitly |
| `email.allow_recipients` | exact recipient addresses; empty keeps `email_read` but does not register `email_send` |
| `email.live` | `false` writes a JSON draft under `<home>/outbox/` and never connects to SMTP |
| credentials | `env:VAR` or a `SecretStore` account; responses and cards carry only `secretPointer` |

For task mail, the runtime keeps the immutable envelope in the local database. Ask the main chat to hold the send and save a draft. If more than one send matches, reply with the listed action ID and the draft instruction. A queued send becomes one durable draft. The same transaction records the draft in the task transcript when it has an execution session. After sender claim, the chat reports the recorded outcome. Identical envelopes for the same task and account share one dispatch right across restarts. Chat-only sends also persist their outcome. Replaying the same invocation or an unresolved identical envelope in that session cannot issue another send.

IMAP and SMTP TLS connections verify the server certificate and hostname. TLS initialization
and handshake failures are reported as tool errors; they do not trigger a plaintext retry.

`calendar_list` and `email_read` declare the `read` effect and stay usable in read-only
sessions and batches. `email_read` uses `BODY.PEEK[]`, so reading mail does not mark it
seen. `email_send` declares `send`: Standard sessions defer to its per-recipient card,
and Full Access does not remove that card. A scoped action grant is consumed when it
matches; an approval persists the grant, and a `once` grant is revoked in the same
operation. SMTP acceptance confirms the receipt. An explicit 4xx or 5xx reply records failure and its code. A missing or incomplete final reply leaves the task receipt unconfirmed and requires reconciliation before another send.

The Connections settings form saves one validated calendar or email section atomically, without replacing other sections. Use an HTTPS calendar URL without embedded credentials, query or fragment. An authenticated service root (such as `https://caldav.icloud.com/`) discovers principal, calendar home and up to eight calendar collections using read-only PROPFIND, followed by REPORT. Malformed discovery items or failed REPORT items are read errors, not successful empty or partial results. For iCloud use an [app-specific password](https://support.apple.com/en-us/102654) in the vault; credentials only follow the configured origin or Apple's numbered CalDAV shard hosts. `jeeves connector calendar test` uses the real read path, displaying only an event count for seven days. For mail, only TLS or STARTTLS can be saved in Settings. Legacy pointers outside `env:VAR` and `vault:NAME` must be corrected before saving. An empty mail recipient list permits reading only; an explicit live switch still requires per-send approval. Connector tools are rebuilt for each new turn and newly started subagent, while in-flight tables stay unchanged. Changes to Feishu's identity/renewer setup still need a restart. A tool without a declared effect is treated as `write`, never `read`.

### Feishu user identity (`"as": "user"`)

By default messages come from the app identity (`tenant_access_token`). To send as a
person, run one browser authorization first:

```bash
jeeves connector feishu login     # authorization-code grant, token at <home>/connector-auth/feishu.json (0600)
jeeves connector feishu status    # valid / expired (refreshed before the next send)
jeeves connector feishu logout    # delete the token
```

Then set `"as": "user"` in the feishu block of `connectors.json`. The callback is fixed
at `127.0.0.1:41766/callback` and must be registered **exactly** in the app console;
there is no PKCE.

The token is **rotating**: each refresh token dies when it is used, so renewals are
serialized (in-process through `connector-auth/feishu.lock`). The long-lived process
(app-server) renews it **ten minutes before each expiry** — a send never pays a network
round trip for it, and a laptop that slept past the expiry wakes up still valid. When
Feishu refuses the grant (authorization revoked, refresh token dead) it says so once and
asks for a human login instead of writing the same line into the log every five minutes.
The CLI keeps its lazy path ("expired, will refresh on the next send" in
`jeeves connector feishu status`).

## Channel adapters

| Platform | Variables |
|---|---|
| Lark | `LARK_APP_ID`, `LARK_APP_SECRET`, `LARK_ALLOW` (open_id), `LARK_DISPATCHERS`, `LARK_DOMAIN=lark`, `LARK_CLAIM=1` (set by the Runtime when it hosts the gateway) |
| Telegram | `TELEGRAM_BOT_TOKEN`, `TELEGRAM_ALLOW` |

`JEEVES_GATEWAY_BIN` is the gateway script the Runtime hosts (Settings → Connections); the Docker image sets it. `JEEVES_GATEWAY_URL` attaches the Lark gateway to an existing Web runtime; unset starts
a standalone app-server. Authenticated Web runtimes also require `JEEVES_OPS_TOKEN`.
The Lark entry point (`bin-lark.ts`) reads the repository-root `.env` without overriding exported values. Setup walkthroughs: [guide](guide.md#channels-lark-and-telegram).

## MCP servers (`<home>/mcp.json`)

```json
{"servers": [
  {"name": "fs", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]},
  {"name": "notes", "url": "https://host/mcp", "auth_env": "NOTES_API_KEY"},
  {"name": "cloud", "url": "https://host/mcp", "oauth": true}
]}
```

`command` uses stdio and `url` uses Streamable HTTP. `auth_env` holds the
environment variable **name** (its value is sent as Bearer); `oauth: true`
uses browser authorization (`jeeves mcp login <name>`) with the token in
`mcp-auth/<name>.json` (0600). Unreachable servers are skipped with a warning;
Jeeves still starts.

The resident Runtime checks configured MCP OAuth credentials every 15 seconds and starts renewal within 60 seconds of expiry. Browser sign-in binds the resource URL and token endpoint. A changed resource address requires sign-in. Legacy unbound credentials retain on-connect refresh until the next browser sign-in enables background renewal. Temporary service failures preserve the credential until its actual expiry. An `invalid_grant` response clears the tokens and the card requests sign-in. Open connections read the rotated token on their next request. Renewal contacts only the authorization service and logs status changes.

MCP HTTP replies are read with a 2 MiB cap, including streamed replies. A tool
that returns only `structuredContent` retains that JSON instead of becoming an
empty result. Larger responses fail explicitly rather than growing memory without bound.

When a connection drops with an unknown tool outcome, Jeeves reconnects for the next call. Reconcile the remote record before trying again. Settings saves MCP configuration in `<home>/mcp.json`. The next turn loads changes and each running turn keeps its starting tool table.

### Connector manifests

Add `"manifest": "team/connector.json"` to an MCP server entry to load a manifest relative to the configuration directory. You can also put the following object directly in `manifest`. The connection owner maintains this file using the original tool names published by the server.

```json
{
  "category": "tasks",
  "tools": {
    "list": {"effect": "read", "resource": "tasks"},
    "close": {"effect": "write", "disabled": true}
  },
  "verbs": {"tasks.list": "list"}
}
```

The categories are `calendar`, `mail`, `tasks`, `notes`, `files`, `contacts`, `maps` and `other`. Tools can declare `read`, `draft`, `write`, `send` or `delete`. A missing manifest, tool entry or effect receives write authorization semantics. Standard sessions require approval for each effectful call. Disabling a tool removes it from the next turn's tool table. The resource label identifies the approval target. The provider controls remote access scope.

Optional standard verbs are `calendar.list_events`, `calendar.create_draft`, `mail.search`, `mail.draft_reply`, `mail.send` and `tasks.list`. A mapping must reference a tool with the matching category and effect. An invalid manifest is rejected as a whole. The curated catalog lists setup methods and official documentation. Capability cards report actual connection status.

### Built-in MCP servers (`JEEVES_MCP_CONFIG`)

A colon-separated list of extra config files, the same split as
`JEEVES_SKILL_DIRS`: `<home>/mcp.json` is the user's, the extras are the
operator's (machine-wide or bundled packs). The user's file is read first and
**wins on a same-named server**; a broken extra file is a warning, never a
blank tool list. In Settings these rows carry a 「内置」 tag and
are greyed out — every write refuses them, because deleting one from a single
machine's UI would silently unconfigure every machine that shares it.

### Discovery order

One rule covers skills, MCP servers and the capability center: `user > pack > builtin`. The user's
file or folder is read first and wins on a same name, so a bundled pack can never shadow a
person's own choice. Writes are the mirror image: only the user's `<home>/skills` and
`<home>/mcp.json` are editable; a name already owned by a pack is refused rather than silently
overwritten. `config_paths_for` and `skill_rows_in` are the pure cores, pinned by
`config_paths_put_the_users_file_first` and `skill_rows_in_*` tests. See the
[capability map](internals/09-capability-map.md#where-to-change-what) for where each surface lives.

## `settings.json` (config/write persistence)

`config/write` accumulates patches into `<home>/settings.json`, layered over
environment variables at boot. To clear a model override, submit an empty
string for the field. A patch failing validation is rejected wholesale —
invalid values never reach the file.

### Automatic BYOK rescue

`modelAutoSwitchToByok` (default `false`) is a human-only switch in this patch:

```json
{"modelAutoSwitchToByok": true}
```

When a **platform-assigned** model is active and its call fails on quota exhaustion (HTTP 429)
or a network error, that turn may fall through to one of your own keyed model profiles. Other
errors (5xx, 401/402/403, a broken stream, context overflow, or a failure after text was shown)
never trigger the switch. With the switch off, quota exhaustion surfaces plainly and you choose
a different model yourself. The model cannot change this setting.

### Native OpenAI Responses

The built-in OpenAI entry uses `wire: "responses"` and `/responses`. Other OpenAI-compatible providers retain `wire: "openai"` and `/chat/completions`. The native adapter supports text, images, function tools, SSE text streaming and encrypted reasoning continuation. Requests set `store: false`; continuation items remain in local working memory and checkpoints. An interrupted stream is an error, and incomplete function calls are never executed. Hosted tools, WebSocket steering and asynchronous API tool calls are not enabled by this adapter.

For a custom endpoint that only implements Chat Completions, patch the catalog and restart:

```json
[{ "id": "openai", "wire": "openai" }]
```

A checkpoint containing native Responses continuation cannot be resumed through a different wire; choose a Responses profile or begin a fresh turn. This follows the current [function-calling](https://developers.openai.com/api/docs/guides/function-calling) and [Responses migration](https://developers.openai.com/api/docs/guides/migrate-to-responses) contracts.

## Per-model capability declarations

`model/save` accepts an optional `capabilities` object, also persisted on the profile
in `models.json` and returned by `model/profiles`. For an existing profile:

```json
{"id":"oa","capabilities":{"tools":true,"contextWindow":128000,"reasoningEffort":"high"}}
```

`tools:false` refuses main-model requests containing function declarations before the
provider is called. `contextWindow` must be positive and replaces the provider's default
estimate; the existing `JEEVES_CONTEXT_WINDOW` runtime override still applies. Image support
continues to use the profile's `vision` declaration. These are human declarations, not
capabilities inferred from a model name or proved by a network probe.

`reasoningEffort` currently requires the Responses adapter. Accepted spellings are `none`,
`minimal`, `low`, `medium`, `high`, `xhigh`, `max`; each model may support fewer. Known invalid
GPT-6 Astra / GPT-6.1 Sol combinations and their Chat Completions function-tool mismatch
are rejected locally. Unknown model-specific restrictions remain the provider's decision.
Explicit effort applies only when the request uses this profile's main model, not a different
`smallModel`. No effort field is sent when omitted. [Official reasoning parameter contract](https://developers.openai.com/api/docs/guides/reasoning#reasoning-effort).

Omitting `capabilities` preserves an existing declaration; `{}` replaces it with inherited
defaults. Unknown capability fields are rejected rather than accepted as inactive switches.
Invalid declarations submitted by `model/save` are rejected before storing credentials.
Hosted tools, native compaction and asynchronous API tools are still unsupported.

## Optional task browser

| Variable | Default | Meaning |
|---|---|---|
| `JEEVES_BROWSER_RUNTIME` | unset | Self-contained components prepared by `just browser-runtime`; without installation browser tools are absent. Browser-enabled desktop bundles locate their own resources |
| `JEEVES_DESKTOP_HELPER` | unset | Absolute path to the trusted macOS window helper. The packaged Mac shell locates its bundled helper for its local Runtime. See [single-window operation](protocol.md#macos-single-window-operation) for development. |
| `JEEVES_BROWSER_HEADLESS` | `0` | Tests only; no visible window exists and human takeover is explicitly refused |

The Web UI controls a browser on the Runtime machine. Remote interactive viewing is not included.
