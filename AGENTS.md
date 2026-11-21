# AGENTS.md

English | [简体中文](AGENTS.zh.md)

Jeeves is a personal assistant built to complete complex, long-running tasks across deployments. Rust owns agent logic and state.
Go owns the unified gateway and cloud control plane. TypeScript covers the browser and the channel adapters.
Read [docs/architecture.md](docs/architecture.md) before changing `jeeves-rs/`;
this file is the working standard for AI coding assistants and human
contributors alike.

## Choosing the next agent improvement

Choose work from the [long-task roadmap](docs/planning/long-task-roadmap.md). Start with an assignment, its constraints and the evidence the user will accept. Follow it across planning, execution, interruption, recovery and delivery. Preserve the same engine contract on personal machines, servers and hosted installations. Record scripted regressions, real-model performance and deployment tests separately.

## Repository layout

```
jeeves-rs/
  jeeves-protocol/      the cross-language contract, single source of truth
                        (schema/ artifacts are checked in)
  jeeves-config/        JEEVES_* environment variables, read once at boot
  jeeves-state/         SQLite + FTS5(trigram); the append-only ledger; memory-backend contract & conformance
  jeeves-provider/      12 providers (incl. local Ollama), three wire formats + SSE, retry + token estimation
  jeeves-tools/         tool registry (schema-checked) + built-in tools, sandboxed exec, subagents,
                        approval, the todo list, the background-job host trait
  jeeves-agent/         deterministic task helpers (verify / observe / plan / paths), no network
  jeeves-connector/     calendar/email/Feishu connectors: allowlisted, drafts by default
  jeeves-loop/          the agent loop (cancellable)
  jeeves-mcp/           MCP client (stdio/HTTP) + browser OAuth + memory server (`jeeves mcp serve`)
  jeeves-memory/        local retrieval + consolidation + Skills + the OKF bundle in `home/memory`
                        (bundle.rs, mirror.rs)
  jeeves-app-server/    JSON-RPC over stdio, one execution runtime per state.db, owning the whole turn:
                        approval, todo injection, background jobs, the goal loop; the ledger
                        writer, the background memory worker, the memory center, and prompt assembly
                        (stable prefix + per-turn tail)
  jeeves-ops/           private Runtime HTTP API (axum + SSE)
  jeeves-cli/           the jeeves binary (REPL / dashboard / eval / mcp / skill)
  jeeves-eval/          deterministic evals + judge + release gate
jeeves-ts/                 @jeeves/client, @jeeves/channels, @jeeves/dashboard, @jeeves/ui,
                        @jeeves/browser-worker
  packages/gateway/     Go account, session, device pairing and Runtime routing service
control-plane/          Go tenant provisioning and model usage service
desktop/                Tauri Mac device app and source-development Runtime shell
evals/                  deterministic, replay, long-task and judge scenarios (JSONL)
docs/                   guide / architecture / configuration / protocol / operations / limitations (+ internals/)
                        internals/ — the implementation tutorial (one chapter per layer)
scripts/                schema generation, docs drift checks, end-to-end smokes and local fixtures
```

## Commands

For interactive Mac testing with the user, use their running Jeeves window and Runtime. Record the app PID before the first interaction so subsequent actions target the same instance. For the checkout’s bundled app, use [the shared Mac launch command](docs/operations.md#mac-desktop-app). Keep the window open for the user after testing. The isolated smoke recipe exercises restart and fixture scenarios separately.

Use the repository's existing commands and tools. When introducing a tool,
explain which requirement the existing workflow cannot meet.

```sh
just check                    # every gate: fmt + schema drift + tool/config/i18n docs drift
                              # + ts-check + eval + replay eval + clippy -D warnings + cargo test
just check-tool-docs          # tool table in 06-tools §6.0 / READMEs vs. the code's tool list
just check-i18n               # every dashboard t("…") string vs. the English table in i18n.ts
just eval / just judge        # deterministic eval (exit 0 = releasable) / judge scores
just write-app-server-schema  # required after any protocol change;
                              # just check-app-server-schema is the CI drift check
just smoke smoke-recovery smoke-web smoke-gateway smoke-dashboard   # end-to-end smokes
```

## Non-negotiable invariants

- **The protocol has one definition**: wire types live in `jeeves-protocol` (RPC envelopes and protocol modules); after changing them run
  `just write-app-server-schema` and commit the artifacts. Wire fields are
  camelCase, integers are `i32`, new types go into the `export.rs` list.
  Change generated files only through their source and the repository's generator.
- **Tool errors are text, not exceptions**: `ToolRegistry::execute` never
  returns Err.
- **Fail-open paths need tests**: local memory lookup failures and timeouts must let the turn continue with its existing permissions.
- **Zero clippy warnings** (`-D warnings`); target <500 lines per crate module.
- **Environment variables supply defaults; saved settings override supported values**; `config/write`
  patches are validated and rejected wholesale — invalid values never reach
  settings.json.
- **No heavy new dependencies**: check the workspace for an equivalent first
  (URL codecs and hashing already exist here).
- **Model credentials** follow the profile’s explicit source. See [credential handling](SECURITY.md#credential-handling). Responses and logs carry only the source pointer.
- **Message drafts**: `send_message` writes to the outbox.
- **Extensions are injected**: the tool layer knows only traits (`SubagentRunner`,
  `ApprovalBroker`, `HookRunner`, `JobRegistry`); every implementation lives in
  app-server and `ToolCtx` only gains fields. The dependency points one way — the
  other way is a cycle.
- **One engine, many shells**: the Rust `app-server` owns logic, state and
  permission decisions; `jeeves-protocol` is the only contract. Web and desktop
  are adapters (render + transport + native capabilities) and never re-implement
  agent behaviour.   Platform differences live in the shell/adapter only: no
  `#[cfg(target_os = …)]`, capability checks or state in the engine, and no
  shell importing another shell. The only engine exceptions are OS-capability
  adapters — credential storage (`jeeves-config/src/models.rs`) and opening the
  OAuth browser (`jeeves-connector/src/feishu_oauth.rs`); `check_architecture`
  allowlists exactly those two files, and adding another needs a reason here.
  All WebView frontends live under `jeeves-ts/` and
  share the dashboard's `src/ui` components plus the generated protocol types; native mobile shells will
  share the same engine and contract through FFI, not the Vue components.
  Adding a platform adds a shell, never a second engine.
- **Capability defaults** follow [configuration](docs/configuration.md). Exec and subagents are opt-in. The interactive dashboard enables file tools with on-request approval by default.
- **Human authority**: approval decisions and goal activation belong to the user. The main chat may delegate spans of a human message. The memory tool can maintain facts and skills within the turn’s permissions. Enabling the external memory MCP also gives its clients explicit remember and forget operations.
- **Never silent**: retries, circuit breaking, skipped duplicate writes, goal
  rounds, finished background jobs each owe the
  user a stderr line or a protocol notification. Watching a frozen screen and
  guessing why is the class of problem this round kept fixing.

## Codebase is the agent's memory

- No ordinary `//` / `/* */` comments in Rust or TS source; `check-comments`
  enforces this. Doc comments (`///`, `//!`, `/** */`) are structural and stay.
- A repeated mistake gets fixed by changing the structure or adding a failing
  lint/test — never by leaving a note for the next agent to copy. Add an
  intentional exception only to `scripts/check_comments.py` `ALLOWLIST`, with a
  reason.

## Avoid unnecessary defensive code

- Before the first production release, switch callers to the new contract and
  remove superseded routes, implementations, and instructions in the same change.
- Validate untrusted input at system boundaries; inside the system, rely on
  established types, contracts, and invariants instead of repeating checks.
- Add fallbacks, retries, compatibility paths, or defaults only for an existing
  contract, a demonstrated failure mode, or a task requirement. Preserve the
  fail-open and configuration validation requirements above.
- Propagate errors with useful context; do not swallow errors or make invalid
  state look successful. Follow the existing tool error and fail-open contracts.
- Prefer the simplest implementation that meets current requirements. Avoid
  speculative abstractions and extension points. Review new checks and error
  handlers for a concrete boundary, failure mode, or documented requirement.
- If a proposed fix would expand the requested scope or change a contract
  beyond what the task authorizes, explain the trigger, impact, and tradeoffs
  and ask the user to decide before implementing it.
- Extract a function when it removes duplication or gives complex logic a
  name that makes the caller easier to follow. Keep simple guard clauses inline.
- Use comments for constraints, reasons, or surprising behavior that the code
  alone does not explain.

## Testing

Behavior changes come with tests: unit tests live inside each crate,
cross-module assembly behavior lives in `evals/deterministic/*.jsonl` (one
case per line; multi-turn, prompt assertions, interruption, and config
injection are supported). Reproduce a bug in a test before fixing it. A
failing `jeeves eval` blocks release.

Exercise the real implementation and assert observable behavior. Avoid copying
business logic into the test to compute the expected result. Use a test double
when an external dependency or failure needs to be controlled. For example,
simulate a memory lookup failure and assert that the turn continues.

## Docs

User-visible behavior changes update the README and the matching `docs/`
 booklet in the same change. Documentation is written in English as the
 primary language with a `.zh.md` Chinese twin per file; update both
 together. One fact per home, no control-flow narration.

## Documentation rules

- A new fact is written in exactly one place; everywhere else links to it.
- Plans and reviews live in a `planning` folder under docs only while open: once landed, the decision goes
  into the formal docs and the file is deleted (history stays in git).
- Every English doc has a `.zh.md` twin with the same heading structure;
  `just check-doc-links` enforces it, plus that relative links resolve.
