# ⑦ Shells: CLI, Dashboard Backend, Front Ends, Gateways, Desktop

English | [简体中文](07-shells.zh.md)

Every shell is a client of the same Runtime ([06-app-server](06-app-server.md)) and speaks the
protocol ([01-protocol-and-config](01-protocol-and-config.md)). A shell renders, transports and
exposes native capabilities; it never re-implements agent behavior
([architecture](../architecture.md)).

Directories: `jeeves-rs/jeeves-cli/`, `jeeves-rs/jeeves-ops/`, `jeeves-ts/packages/{client,dashboard,gateway,browser-worker}`,
`desktop/src-tauri/`, plus `Dockerfile` and `compose.yaml`.

## 7.1 The `jeeves` binary

`jeeves-cli/src/main.rs` dispatches on the first argument:

| Invocation | Behavior |
|---|---|
| `jeeves` | Terminal chat: opens the Server in-process, attaches the memory worker, runs `repl::run` |
| `jeeves app-server` | JSON-RPC over stdio; attaches the memory worker and the scheduler |
| `jeeves dashboard` | `jeeves_ops::serve` (HTTP + SSE) |
| `jeeves schedule acknowledge <name>` | Releases an interrupted scheduled slot; any other `schedule` form is refused (jobs fire inside the Runtime) |
| `jeeves home export <dir>` / `import <dir>` | Moves one home (7.3) |
| `jeeves memory [rebuild\|backfill]` | Memory maintenance (7.3) |
| `jeeves skill [list\|export\|install\|update]` | Skills (7.3) |
| `jeeves mcp [list\|login <name>\|serve]` | MCP client config and the memory MCP server (7.3) |
| `jeeves connector [list\|feishu …]` | The human's own path to the Feishu connector (7.3) |
| `jeeves eval [path]` / `eval --replay [path]` / `judge [path]` / `bench [path]` | Quality commands, see [08-quality](08-quality.md); the exit code is the verdict (`bench` always exits 0) |
| `jeeves --version` / `-V` | `jeeves <version>` |

Anything else fails with the usage text. On Windows the binary refuses to start (exit code 2): the exec
sandbox has only macOS and Linux backends, hooks run through `sh`, and the Windows credential store
cannot be read back.

`home()` is `JEEVES_HOME`, otherwise `./.jeeves` in the current working directory. The `app-server`, `memory`,
`dashboard` and REPL paths write that value into `settings.home`, so an explicit `JEEVES_HOME` always wins.
`dashboard` does not create the directory itself; the spawned app-server does.

## 7.2 Terminal chat

`repl.rs` runs each turn with `run_turn` inside `tokio::spawn` and draws frames from an `EventSink` channel as they arrive:
streamed text is printed immediately, and the other notifications (memory gate, tool start/finish,
consolidation, turn summary line) are drawn as one-line small text. The reply is printed whole when the turn
streamed nothing.

Slash commands: `/help`, `/new`, `/sessions`, `/memory [term]`, `/compact [focus]`, `/goal <condition>`
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

`jeeves dashboard` runs `jeeves_ops::serve`. The crate is a client of its own `jeeves app-server` child:
`AppServer::spawn` starts `current_exe() app-server` with the same `JEEVES_HOME` (and defaults `JEEVES_WRITE=1` and
`JEEVES_APPROVAL=on-request` when unset), correlates requests by id, and broadcasts notifications to subscribers. Errors from the
app-server are passed back verbatim as the protocol's `ErrorObject`.

**Startup.** Binding validation, then the dashboard freshness check (`freshness.rs`: `dist/main.js` must exist and not be
older than `src/`; skipped when no `src/` exists), then the first free port among `JEEVES_PORT` (default 7777) plus the next nine.
It prints `Jeeves cockpit → http://<host>:<port> …`, which the desktop shell waits for. The hosted gateway starts after the port is known.
Ctrl-C or SIGTERM stops accepting, stops the gateway, and drains turns in flight for up to 25 s.

**Auth.** `JEEVES_OPS_HOST` (default `127.0.0.1`) must be loopback unless `JEEVES_OPS_TOKEN` is set, otherwise startup fails.
With a token, every `/api/*` route needs `Authorization: Bearer <token>` (constant-time comparison; no local bypass).
Outside that layer: static files, `GET /healthz` (a bounded `session/capabilities` probe returning version and
protocol version, or 503), and `POST /api/hook/{name}`, which proves itself with the hook's own secret. All responses carry
`Cache-Control: no-store`; there is no CORS layer because every view loads from this server; the body limit is 25 MB.

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

`/api/turn` subscribes before sending `turn/start`, so no early notification is lost. SSE is one-way server to browser; sending a
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

`POST /api/gateway/rpc` forwards only `turn/start`, `interaction/start`, `session/main`, `task/get`, `approval/pending` and
`approval/respond`; every other method answers method-not-found.

## 7.5 `@jeeves/client`

The TypeScript client of the protocol. Types come from the generated artifacts through `protocol.ts` alone
(`jeeves-protocol/schema/typescript/v2`); every other file imports from it.

- `JeevesClient` assigns ids, keeps a pending table keyed by the id normalized to a string, parses each line as a
  notification, an error or a response, and rejects every pending request when the transport closes. `JeevesError`
  carries `code` and `data` with the predicates `isProvider`, `isNotImplemented`, `isInvalidParams`.
- `StdioTransport` spawns `jeeves app-server`, splits stdout into lines, forwards stderr to `onLog`, and `close()` only ends stdin so
  the server flushes its remaining frames. `findJeevesBinary` resolves `JEEVES_BIN`, then `jeeves-rs/target/{debug,release}/jeeves` found by
  walking up from the package, then `jeeves` on PATH.
- `HttpTransport` attaches to a running Runtime instead of spawning one: it reads `/api/notifications` and sends with an optional
  Bearer token.

## 7.6 `@jeeves/dashboard`

A Vue 3 + Vite front end; the backend is `jeeves-ops` (7.4). `npm run build` type-checks (`vue-tsc`), runs Vite and
`copy-assets.mjs` into `dist/`, which `jeeves dashboard` serves (`just dashboard-build`; `JEEVES_DASHBOARD_DIR` overrides the location).

| Piece | Role |
|---|---|
| `main.ts`, `WebControls.vue`, `workspace.ts` | Mount the Vue controls and run the page controller |
| `api.ts` | Typed `fetch` wrappers over the routes above; `talk()` reads the SSE body by hand |
| `sync.ts` | Holds `/api/events` open, coalesces pings within 500 ms into one refresh, reconnects forever; a 15 s poll (`POLL_MS`) backs it up |
| `token-dialog.ts` | One dialog for simultaneous 401 responses, asking for the deployment token |
| `render.ts`, `timeline.ts`, `tasks.ts` | DOM construction without HTML strings |
| `markdown.ts` | Hand-written Markdown renderer that builds DOM nodes for model replies; no third-party parser |
| `ui/` | Shared components: `SetupWizard`, `SettingsPanel`, `TaskCenter`, `BrowserPanel`, `ModelPicker`, `PermissionPicker`, `WorkspacePicker`, `AttachmentPicker`, with composables `useTasks` and `useBrowsers` |
| `sw.js`, `manifest.webmanifest` | PWA: `/api/*` is never cached, pages are network-first, other static assets cache-first; `VERSION` is bumped with every shipped front-end change |

The **setup wizard** (`SetupWizard.vue`) starts at the model step (provider, model, API key, base URL) when no model is
configured, then asks for a permission mode and optional notes about the user, saved with `remember`.

**Language.** The Chinese string is the dictionary key: `t("还没有会话")`, with `{n}` placeholders; `i18n.ts` holds the
English table. Static markup uses `data-i18n`, `data-i18n-title`, `data-i18n-placeholder`, `data-i18n-aria`. The language comes from
`localStorage` key `jeeves.lang`, otherwise from `navigator.language` (`zh` prefix means Chinese). Switching reloads the page. A
missing English entry falls back to Chinese with one `console.warn`. Model output, session and task titles are data and
never translated. `scripts/check_i18n.py` (`just check-i18n`) checks that every used key is translated, every translated key is used,
and placeholders match.

## 7.7 Chat gateways

`@jeeves/channels` holds `JeevesBridge` and two platform adapters: Lark/Feishu (`lark.ts`, `bin-lark.ts`) and WeChat Official Account
(`wechat.ts`, `bin-wechat.ts`). Standalone runs: `just gateway-lark`, `just gateway-wechat`.

| | Lark / Feishu | WeChat |
|---|---|---|
| Connection | Long-lived WebSocket with Lark's private protobuf frames (`pbbp2`, hand-written in `lark-proto.ts`) | HTTP callback server (`WECHAT_PORT`, default 8080; path `WECHAT_PATH`, default `/wechat`); needs a public address |
| Allowlist | `LARK_ALLOW`: open_ids (`ou_…`) | `WECHAT_ALLOW`: OpenIDs |
| Message limit | 30000 characters | 600 characters |
| Timing | A data frame is ACKed first, then processed; heartbeat default 120 s (minimum 10 s); reconnect backoff doubles up to 30 s | Passive reply only if the turn finishes within 4000 ms, otherwise `success` first and the answer follows as customer-service messages (needs `WECHAT_APP_SECRET` and a verified account) |
| Other settings | `LARK_APP_ID`, `LARK_APP_SECRET`, `LARK_DOMAIN=lark` for the international domain, `LARK_DISPATCHERS`, `LARK_CLAIM` | `WECHAT_TOKEN`, `WECHAT_APP_ID`, `WECHAT_AES_KEY` (43 characters; plain, compatible and secure modes) |

`bridge.ts` (`JeevesBridge`) holds the platform-independent part:

- **Stateless sessions**: `sessionIdFor(conversation)` is the conversation id itself (for example `lark:<id>`).
- **One queue per conversation** (`#tails`): messages of a conversation run one at a time.
- Turns start with `stream: false`; a turn's `turnId` is matched to its notifications through `#inFlight` / `#completed`,
  so a notification that arrives before the response is not lost.
- `JeevesBridge` spawns its own app-server, or attaches to a Runtime when given `url` and `token`
  (`JEEVES_GATEWAY_URL`, `JEEVES_OPS_TOKEN`).
- `explain(error)` turns a `JeevesError` into a plain sentence for the chat.
- **Dispatchers** (`LARK_DISPATCHERS`, mirrored to `JEEVES_DISPATCHERS` for the Runtime): a dispatcher's group message goes to the main
  session `jeeves-main` with an `InteractionSource`; the platform message id (`safeId`) is the request id, so a redelivered event is a
  replay, not a second dispatch. `reports.ts` reports a dispatched task back into its thread when it reaches `done`, `failed`,
  `cancelled` or `needs_attention`; `approvals.ts` sends approval requests as cards, and a dispatcher's click answers once through
  `approval/respond`.

`splitMessage(text, limit)` (`text.ts`) has one implementation: it cuts at a newline past the halfway point, else hard-cuts.

## 7.8 Runtime-hosted Lark gateway

Settings → Connections (`api/connections.rs`, `gateways.rs`) lets the owner run the Lark gateway from the dashboard without a
second terminal:

- Settings are stored in `<home>/gateways.json` (`enabled`, `appId`, `allow`, `dispatchers`, `domain`); the app secret goes to
  the secret store under account `gateway:lark:app-secret` and is never read back through the API. Ids in `allow` and `dispatchers` must
  start with `ou_`.
- `start` spawns `node jeeves-ts/packages/channels/src/bin-lark.ts` (override with `JEEVES_GATEWAY_BIN`) with the Lark variables and
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

`desktop/src-tauri` is a separate Cargo workspace (package `jeeves-desktop`, Tauri 2) with a single `src/main.rs`. It holds no
agent logic and no protocol: it opens one window (1180×800, minimum 900×640) on the Runtime's own front end.

- With `JEEVES_RUNTIME_URL` set it attaches to that Runtime. Otherwise it starts `jeeves dashboard`, waits up to 30 s for the
  `Jeeves cockpit → ` line on stdout, and loads the announced URL; on failure it shows `shell/index.html` with the error. On exit it kills
  the child it started.
- The binary is `jeeves` beside the app executable in release builds (bundled as the `binaries/jeeves` sidecar through
  `sidecar.conf.json`); in debug builds `JEEVES_DESKTOP_BINARY` (an absolute path) or `jeeves-rs/target/debug/jeeves`.
- It sets `JEEVES_DASHBOARD_DIR` to the bundled `dashboard` resource and `JEEVES_BROWSER_RUNTIME` to the bundled `browser-runtime` when
  `runtime.json` exists there (`browser.conf.json` adds it to the bundle), unless the variables are already set.

Commands: `just desktop-dev`, `just desktop-build`, `just desktop-build-browser`, `just desktop-check`, `just smoke-desktop`.

## 7.11 Container image

`Dockerfile` has three stages: the Rust build of `jeeves-cli`, the Node 22 build of `@jeeves/dashboard`, and a Node 22 runtime image with
`bubblewrap` (the Linux exec sandbox) and CA certificates. The image runs as a non-root `jeeves` user with `JEEVES_OPS_HOST=0.0.0.0`,
`JEEVES_HOME=/home/jeeves/.jeeves` (a volume), `JEEVES_DASHBOARD_DIR` and `JEEVES_GATEWAY_BIN` set, port 7777, and `ENTRYPOINT ["jeeves"]` with `CMD ["dashboard"]`.
Because the host is not loopback, `JEEVES_OPS_TOKEN` is mandatory.

`compose.yaml` requires `JEEVES_OPS_TOKEN`, mounts the `jeeves-state` volume, uses `stop_grace_period: 30s` (the Runtime drains for up to 25 s), and
health-checks `/healthz`. Deployment steps are in [operations](../operations.md); settings are in [configuration](../configuration.md);
known gaps are in [limitations](../limitations.md).
