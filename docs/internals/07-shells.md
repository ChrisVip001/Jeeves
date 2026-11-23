# ⑦ Shells: CLI, Dashboard Backend, Front Ends, Gateways, Desktop

English | [简体中文](07-shells.zh.md)

Every shell is a client of the same Runtime ([06-app-server](06-app-server.md)) and speaks the
protocol ([01-protocol-and-config](01-protocol-and-config.md)). A shell renders, transports and
exposes native capabilities; it never re-implements agent behavior
([architecture](../architecture.md)).

Directories: `runtime/crates/jeeves-cli/`, `runtime/crates/jeeves-ops/`, `web/`, `gateway/`, `integrations/channels/`, `executors/browser/`,
`desktop/src-tauri/`, plus `Dockerfile`.

## 7.1 The `jeeves` binary

`jeeves-cli/src/main.rs` dispatches on the first argument:

| Invocation | Behavior |
|---|---|
| `jeeves` | Terminal chat: opens the Server in-process, attaches the memory worker, runs `repl::run` |
| `jeeves app-server` | JSON-RPC over stdio; attaches the memory worker and the scheduler |
| `jeeves runtime` | `jeeves_ops::serve` (HTTP + SSE) |
| `jeeves schedule acknowledge <name>` | Releases an interrupted scheduled slot; any other `schedule` form is refused (jobs fire inside the Runtime) |
| `jeeves home export <dir>` / `import <dir>` | Moves one home (7.3) |
| `jeeves memory [rebuild\|backfill]` | Memory maintenance (7.3) |
| `jeeves skill [list\|export\|install\|update]` | Skills (7.3) |
| `jeeves mcp [list\|login <name>\|serve]` | MCP client config and the memory MCP server (7.3) |
| `jeeves connector [list\|feishu …]` | The human's own path to the Feishu connector (7.3) |
| `jeeves eval [path]` / `eval --replay [path]` / `judge [path]` / `research [path]` / `bench [path]` | Quality commands, see [08-quality](08-quality.md); the exit code is the verdict (`research` and `bench` always exit 0) |
| `jeeves --version` / `-V` | `jeeves <version>` |

Anything else fails with the usage text. On Windows the binary refuses to start (exit code 2): the exec
sandbox has only macOS and Linux backends, hooks run through `sh`, and the Windows credential store
cannot be read back.

`home()` is `JEEVES_HOME`, otherwise `./.jeeves` in the current working directory. The `app-server`, `memory`,
`runtime` and REPL paths write that value into `settings.home`, so an explicit `JEEVES_HOME` always wins.
`runtime` does not create the directory itself; the spawned app-server does.

## 7.2 Terminal chat

`repl.rs` runs each turn with `run_turn` inside `tokio::spawn` and draws frames from an `EventSink` channel as they arrive:
streamed text is printed immediately, and the other notifications (memory gate, tool start/finish,
consolidation, turn summary line) are drawn as one-line small text. The reply is printed whole when the turn
streamed nothing.

Slash commands: `/help`, `/memory [term]`, `/compact [focus]`, `/goal <condition>`
(`/goal clear`), `/cwd [path]`, `/model` (`use`, `add`), `/route` (`auto`, `fixed`, `set <tier> <profile>`),
`/quit` (`/exit`, `/q`). An unknown `/x` is reported as unknown.

Approvals are answered on the spot by reading stdin on a separate handle: `y` once, `s` for the session, `a` to
remember the rule, anything else (including EOF) denies. An answer that arrives after the turn stopped waiting is reported as
expired.

## 7.3 Maintenance subcommands

| Command | Behavior |
|---|---|
| `jeeves home export <dir>` | Writes a plain directory: a consistent `state.db` snapshot, every other file copied, and a `jeeves-home.json` manifest (`format`, `schema`, `jeeves`, `exportedAt`, what was left out). The target must be empty |
| `jeeves home import <dir>` | Restores such a directory into the home |
| `jeeves memory rebuild` (default) | Folds hand edits of the plaintext library back in, then regenerates it from the facts |
| `jeeves memory backfill` | Builds ledger rows from chat history, once, only while the ledger is empty |
| `jeeves skill list` (default) | One line per skill |
| `jeeves skill export --to claude,codex [--project] [--force] [--names a,b]` | Copies skills to the `.claude` / `.codex` skill directories |
| `jeeves skill install <url\|path>` | Installs one skill; never overwrites |
| `jeeves skill update [index]` | Updates by version from an index (default `<home>/skills/index.json`; a path or http(s) URL also works) |
| `jeeves mcp list` (default) | Entries of `mcp.json` with their auth method |
| `jeeves mcp login <name>` | Browser OAuth for a server that has a `url` and `oauth: true`; the only place a browser is opened |
| `jeeves mcp serve` | Exposes facts and episodes as an MCP server over stdio |
| `jeeves connector list` / `feishu login\|logout\|status` | Connector state and Feishu user authorization |
| `jeeves connector feishu --to <id> --text <body> [--dry-run]` | Sends through the same allowlist and wire code as the tool; the person typing is the approval |

`home export` leaves out `state.db`'s live files, lock files, `secrets.json`, `mcp-auth/` and `connector-auth/`;
after an import, keys and sign-ins are given again.

## 7.4 `jeeves-ops`: the HTTP face

`jeeves runtime` runs `jeeves_ops::serve`. The crate is a client of its own `jeeves app-server` child:
`AppServer::spawn` starts `current_exe() app-server` with the same `JEEVES_HOME` (and defaults `JEEVES_WRITE=1` and
`JEEVES_APPROVAL=on-request` when unset), correlates requests by id, and broadcasts notifications to subscribers. Errors from the
app-server are passed back verbatim as the protocol's `ErrorObject`.

**Startup.** The private Rust Runtime validates its service credential and waits for the engine before binding. It prints `Jeeves Runtime → http://<host>:<port>` and starts the configured Feishu channel. On shutdown it closes SSE and drains active work.

**Auth.** The Go gateway adds the bearer credential server-side. The private health probe and signed webhooks use their own protocols. Rust serves HTTP API and SSE. Go serves the workbench assets.

**Routes** (`router` in `lib.rs`; handlers under `api/`). Each is a translation of one or a few protocol methods:

| Group | Routes |
|---|---|
| First screen and config | `GET /api/data`, `GET/POST /api/config`, `GET /api/usage`, `GET /api/inbox` |
| Turns | `POST /api/turn` (SSE stream of `ServerNotification`), `POST /api/turn/steer`, `POST /api/turn/follow-up`, `POST /api/interrupt`, `GET /api/turns`, `POST /api/interaction`, `POST /api/run` (task API: one turn in, one `RunResponse` out, no SSE) |
| Approvals | `GET /api/approval/pending`, `POST /api/approval`, `GET /api/approvals`, `GET/POST /api/session/permission` |
| Sessions | `GET /api/session`, `GET /api/session/main`, `GET /api/session/meta`, `POST /api/session/update` |
| Tasks and browsers | `GET /api/tasks`, `POST /api/task`, `POST /api/turn-operation`, `GET /api/browser`, `POST /api/browser/start\|control`, `GET /api/browser/frame` |
| Models | `GET /api/model/catalog\|profiles\|routes`, `POST /api/model/save\|remove\|routes/save` |
| Capabilities | `/api/skills` (+ `install`, `remove`, `stats`), `/api/mcp` (+ `save`, `remove`), `/api/schedules/jobs`, `/api/schedules/save\|remove`, `/api/vault` (+ `remove`) |
| Memory center | `GET /api/ledger/turns`, `/api/ledger/turn`, `/api/memory/index`, `/api/memory/page`, `/api/context`, `POST /api/compact`, `POST /api/memory/remember` |
| Files | `POST /api/upload` (single octet-stream, 20 MB), `GET /api/fs`, `POST /api/fs/mkdir`, `GET/POST /api/fs/recents` |
| Sync | `GET /api/events` (SSE of pings: kind, turn id, session id), `GET /api/notifications` (full notification SSE) |
| Connections | `GET/POST /api/connections/lark`, `POST /api/connections/lark/claim`, `POST /api/gateway/rpc` |

`/api/interaction` subscribes before sending `interaction/start`, so no early notification is lost. SSE is one-way server to browser; sending a
message is an ordinary POST. A refused human action (`/api/task`, the capability writes) is answered with status 200 and
an `ErrorObject` body so the page shows the sentence verbatim.

**Capability center** (`capabilities.rs`): skills from `JEEVES_SKILL_DIRS` and MCP servers from `JEEVES_MCP_CONFIG` are listed as
`source: "builtin"` and refused by every write; only `<home>/skills/` and `<home>/mcp.json` are writable. The listing merges
the same way the app-server does (user file first, first name wins).

**Inbound webhooks** (`webhooks.rs`, `<home>/webhooks.json`): each hook has `name`, `secret` (`env:VAR` or literal), `prompt` (`{{payload}}` is
replaced by the request body, else the body is appended), and optional `session` (default `hook:<name>`), `tier`,
`enabled`, `signature`, `limit`, `callback`. An unknown or disabled name is 404, a wrong secret 401, a broken
file disables all hooks with a stderr line. With `signature: true` the caller sends `X-Jeeves-Timestamp` and
`X-Jeeves-Signature` (HMAC-SHA256 over `<timestamp>.<body>`, 300 s window) and each signature is accepted once; `limit` caps accepted
requests per rolling minute (429 with `Retry-After`); `callback` answers 202 and POSTs the result later. The turn is an
ordinary turn: tools keep their approvals and nobody is present to grant them.

`POST /api/gateway/rpc` forwards only `interaction/start`, `session/main`, `task/get`, `task/wake`, `approval/pending` and
`approval/respond`; every other method answers method-not-found.

## 7.5 `@jeeves/client`

The Node Runtime client lives in `integrations/channels/runtime-client/`. Its `protocol.ts` re-exports
the generated types from `protocol/schema/typescript/v2/`. The client and Web workbench share the
SSE parser and task event cursor in `protocol/typescript/`.

- `JeevesClient` assigns ids, keeps a pending table keyed by the id normalized to a string, parses each line as a
  notification, an error or a response, and rejects every pending request when the transport closes. `JeevesError`
  carries `code` and `data` with the predicates `isProvider`, `isNotImplemented`, `isInvalidParams`.
- `StdioTransport` spawns `jeeves app-server`, splits stdout into lines, forwards stderr to `onLog`, and `close()` only ends stdin so
  the server flushes its remaining frames. `findJeevesBinary` resolves `JEEVES_BIN`, then `target/{debug,release}/jeeves` found by
  walking up from the package, then `jeeves` on PATH.
- `HttpTransport` attaches to a running Runtime. It sends RPCs to `/api/gateway/rpc` and follows
  `/api/notifications` with the private Runtime bearer token when configured.

## 7.6 `@jeeves/dashboard`

Vue and Vite build the workbench and account pages into `web/dist/`. The Go gateway serves the workbench under the authenticated workspace path and forwards same-origin API requests to the tenant Runtime. Build the assets with `just dashboard-build`.

| Piece | Role |
|---|---|
| `main.ts`, `WebControls.vue`, `workspace.ts` | Mount the Vue controls and run the page controller |
| `api.ts`, `hosted.ts` | Typed workspace requests use the gateway session cookie. Expired sessions return to sign-in. Streams use the shared SSE parser. |
| `sync.ts` | Holds `/api/events` open, coalesces pings within 500 ms into one refresh, reconnects forever; a 15 s poll (`POLL_MS`) backs it up |
| `render.ts`, `timeline.ts`, `tasks.ts` | DOM construction without HTML strings |
| `markdown.ts` | Hand-written Markdown renderer that builds DOM nodes for model replies; no third-party parser |
| `ui/` | Shared components: `SetupWizard`, `SettingsPanel`, `TaskCenter`, `BrowserPanel`, `WorkspacePicker`, `AttachmentPicker`, with composables `useTasks` and `useBrowsers` |
| `sw.js`, `manifest.webmanifest` | PWA support with network-first pages and no API cache. The build stamps the entry and cache version together. |

See the [guide](../guide.md) for personal deployment setup and menu entries.

**Language.** The Chinese string is the dictionary key: `t("还没有会话")`, with `{n}` placeholders; `i18n.ts` holds the
English table. Static markup uses `data-i18n`, `data-i18n-title`, `data-i18n-placeholder`, `data-i18n-aria`. The language comes from
`localStorage` key `jeeves.lang`, otherwise from `navigator.language` (`zh` prefix means Chinese). Switching reloads the page. A
missing English entry falls back to Chinese with one `console.warn`. Model output, session and task titles are data and
never translated. `scripts/check_i18n.py` (`just check-i18n`) checks that every used key is translated, every translated key is used,
and placeholders match.

## 7.7 Chat gateways

`@jeeves/channels` holds `JeevesBridge` and adapters for Lark/Feishu and Telegram. The standalone
commands are `just gateway-lark` and `just gateway-telegram`. See the
[adapter settings](../configuration.md#channel-adapters).

| | Lark / Feishu | Telegram |
|---|---|---|
| Connection | Long-lived WebSocket with Lark's private protobuf frames (`pbbp2`, hand-written in `lark-proto.ts`) | Long polling; no public address needed |
| Allowlist | `LARK_ALLOW`: open_ids (`ou_…`) | `TELEGRAM_ALLOW`: numeric user ids |
| Message limit | 30000 characters | Platform limit |
| Timing | Persist the incoming frame before ACK, then process it. Heartbeat default 120 s (minimum 10 s) and reconnect backoff up to 30 s. | Poll the bot API; answer through the same Runtime turn |
| Other settings | `LARK_APP_ID`, `LARK_APP_SECRET`, `LARK_DOMAIN=lark` for the international domain, `LARK_DISPATCHERS`, `LARK_CLAIM` | `TELEGRAM_BOT_TOKEN` from the environment |

`bridge.ts` (`JeevesBridge`) holds the platform-independent part:

- **One main conversation.** The bridge serializes incoming requests and calls `interaction/start` in
  `jeeves-main`. The platform conversation ID is routing metadata for replies and approvals.
- The bridge correlates early notifications with the platform message and then follows the returned
  `turnId` to completion.
- `JeevesBridge` spawns its own app-server, or attaches to a Runtime when given `url` and `token`
  (`JEEVES_GATEWAY_URL`, `JEEVES_OPS_TOKEN`).
- `explain(error)` turns a `JeevesError` into a plain sentence for the chat.
- **Dispatchers.** The Runtime checks `JEEVES_DISPATCHERS` before exposing task delegation to group senders. The bridge includes
  the `InteractionSource` and derives a stable message ID for deduplication. `reports.ts` returns terminal task states to the originating thread.
  `approvals.ts` sends cards and authorized decisions go through versioned `approval/respond`.

`splitMessage(text, limit)` (`text.ts`) has one implementation: it cuts at a newline past the halfway point, else hard-cuts.

## 7.8 Runtime-hosted Lark gateway

Settings → Connections (`api/connections.rs`, `gateways.rs`) lets the owner run the Lark gateway from the dashboard without a
second terminal:

- Settings are stored in `<home>/gateways.json` (`enabled`, `appId`, `allow`, `dispatchers`, `domain`); the app secret goes to
  the secret store under account `gateway:lark:app-secret` and is never read back through the API. Ids in `allow` and `dispatchers` must
  start with `ou_`.
- `start` spawns `node integrations/channels/src/bin-lark.ts` (override with `JEEVES_GATEWAY_BIN`) with the Lark variables and
  `JEEVES_GATEWAY_URL` pointing at this Runtime, plus `JEEVES_OPS_TOKEN` when a token is set. The gateway attaches through
  `POST /api/gateway/rpc` and `/api/notifications`.
- **Claim flow**: with an empty `allow` the process runs with `LARK_CLAIM=1` and answers nobody; the first sender is reported on stderr as
  `claim candidate: <open_id>`, which the Runtime captures. `POST /api/connections/lark/claim` with that id writes it as the sole
  allowlist entry and restarts the gateway; it fails if an owner already exists or the id is not the waiting candidate.
- Status is `stopped`, `running` or `exited`, with the candidate and the last 20 stderr lines. Starting, exiting and stopping each print a
  stderr line. The gateway autostarts when `enabled` and an app id are set, and is stopped at shutdown.

## 7.9 Task browser worker

The model sees only `browser_observe` and `browser_act` (`jeeves-tools/src/browser.rs`, behind an injected host trait). Ownership, approved
origins, process lifetime and control generations live in `jeeves-app-server/src/browser/` (`api`, `process`, `store`); the `/api/browser*`
routes are transport only, and `browser/start` is a human-only call.

`@jeeves/browser-worker` is a Node/Playwright (Chromium) process, one browser context and one page per session:

- Transport: one JSON object per line over stdin/stdout; a line over 64000 bytes or an invalid request stops the worker, and stdin
  closing stops it too. The command contract is generated from `BrowserWorkerCommand`. Every response carries
  `info` (schema version, instance id, supported capabilities, `maxOrigins` 12,
  `maxFrameBytes` 800000), the session state, an optional frame, an error, and `uncertain`.
- The host applies a 20 s deadline to an operation and closes the worker on timeout.
- `network.ts` enforces the origin scope through a local proxy; `observe.ts` produces the observation and frames; `document.ts` tracks
  document changes so stale references fail; `session.ts` serializes commands and watches heartbeats.
- The runtime is opt-in: `JEEVES_BROWSER_RUNTIME` names a directory with `runtime.json` (Node and Playwright versions) and the
  worker; `JEEVES_BROWSER_HEADLESS` runs without a window. `just browser-runtime` prepares `desktop/src-tauri/browser-runtime/`
  (`scripts/prepare-browser-runtime.py`); `just smoke-browser` exercises the whole flow.
- The dashboard side is `ui/BrowserPanel.vue` and `useBrowsers.ts`: open from a task card, take over, return, pause, close,
  and [operate the remote browser or accept a task download](../protocol.md#remote-browser-input-and-downloads).

The policy (approvals, scope, uncertain effects) is documented with the loop and tools in [04-tools](04-tools.md) and
[06-app-server](06-app-server.md).

## 7.10 Desktop shell

The Tauri development shell opens the Go gateway and can start the private Runtime for a configured workspace. The hosted Mac device app pairs with the account and grants access to selected windows. See [Mac desktop app](../operations.md#mac-desktop-app) for build and test commands.

## 7.11 Container image

The root `Dockerfile` builds the private Rust Runtime with optional channel adapters. The Go gateway image contains the browser assets. See the [cloud deployment guide](../../control-plane/README.md).
