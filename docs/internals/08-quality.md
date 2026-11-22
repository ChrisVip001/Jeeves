# ⑧ Quality: How the System Proves It Still Works

English | [简体中文](08-quality.zh.md)

This layer has no product features. It decides whether the fail-open and never-run-bare rules of the
other chapters are actually true. Layers are ordered cheapest first; deterministic evals passing 100%
is the release gate.

Locations: `runtime/crates/jeeves-eval/`, `evals/`, `scripts/`, `justfile`, `.github/workflows/ci.yml`.

## 8.1 Layers

| Layer | Location | Shape | Blocks |
|---|---|---|---|
| Unit tests | each Rust crate, `npm test` and `go test ./...` | in-process, offline | yes |
| Deterministic eval | `evals/deterministic/*.jsonl` | a scripted model drives the real `run_turn` | yes (exit 0 = releasable) |
| Replay eval | `evals/replay/*.jsonl` | recorded dialogue plus an expected event sequence, same runner | yes |
| Long-task benchmark | `evals/long/tasks.jsonl` | same harness, asks whether autonomous task work got better | never |
| Judge | `evals/judge/quality.jsonl` | a real model answers, a judge model scores 0-10 | never |
| Smoke | `scripts/smoke*` | real processes, real stdio and HTTP | release gate runs a subset |
| Coverage | `cargo llvm-cov` in CI | line-coverage floor of 60% | yes |

Cost and determinism are inversely related. Unit tests and evals are deterministic (the model is fake),
but evals run the real `run_turn` against a real `state.db`. Judge scores and benchmarks are trends,
not gates, because a real model's output is unstable.

## 8.2 Deterministic Eval

Entry: `jeeves eval [path]` (default `evals/deterministic`; `just eval`). Implementation:
`runtime/crates/jeeves-eval/src/` (`scenario.rs`, `lib.rs`, `jsonl.rs`, `report.rs`).

### Scenario format

A case is one JSON line in a `.jsonl` file (`#` comment lines allowed). Fields of `Scenario`:

| Field | Purpose |
|---|---|
| `id`, `description` | name and one-line intent |
| `message` / `responses` / `interrupt_after_deltas` / `cwd` | single-turn shorthand |
| `turns[]` | multi-turn: each turn has `message`, `session_id`, `responses`, `apply_patch`, `interrupt_after_deltas`, `cwd`, `restart`, `resume_task`, `approval` |
| `settings` | per-case Settings overrides (window, consolidation, retrieval, iterations, tokens, context window, compaction threshold, exec/write/delegate/plan switches, exec allowlist, tool-result budget, soul mode, tool policy, skill budgets, approval mode and timeout, auto-continue and step slice) |
| `files` | files written into the case home before the run |
| `prereq` | `python3`, `sh` or `sandbox`; a missing prerequisite is reported as skipped, never as passed |
| `known_gap` | a behavior not built yet; a failure does not block, a pass does, so the marker cannot go stale |
| `task` | a long task created before the first turn (title, goal condition, acceptance, budget, round cap) |
| `recovery` | tags a long-benchmark case as an interrupt-recovery case |
| `plan` | the planner's scripted reply, queued before the first round |
| `expect` | the assertions below |

A response is `{"text":"…"}` or `{"tool_use":{"name":"…","args":{…}}}`. `{{home}}` in `files`, `cwd`
and tool `args` expands to the case's temporary home with forward slashes, so it stays valid JSON on
Windows.

Task cases create the task under the main conversation and queue its independent execution through
`task/start`. Use a turn's `approval` script for the human decisions expected by that case.

The mock queue is the turns' `responses` flattened in execution order, with `plan` first. Extra model
calls (summaries, consolidation) are queued where they happen, so a case documents the
real call order. An empty queue is an error, not a repeat of the last reply.

`restart` reopens the runtime in-process on the same home; it does not kill an OS process.
`resume_task` calls the human-only `task/resume` path with `message` as the control reason. Both are
evaluator-only fields, not protocol or model tools.

### Assertions

| Assertion | Meaning |
|---|---|
| `reply_contains` / `reply_not_contains` | the final reply does / does not contain |
| `tools_used` | ordered tool names across turns; empty = unchecked |
| `tool_output_contains` | some tool output contains the fragment (called versus actually succeeded) |
| `iterations` | number of loop rounds |
| `consolidation_new_facts` | facts distilled across consolidation notifications |
| `approvals` | how many approval questions were asked (`0` is a real assertion) |
| `events` | ordered subsequence of wire events, plain (`toolStarted`) or qualified (`toolStarted:run_command`, `toolCompleted:run_command:ok`) |
| `interrupted` | whether the last turn was interrupted |
| `prompt_contains` / `prompt_not_contains` | against all model requests concatenated |
| `last_prompt_contains` / `last_prompt_not_contains` | against the last model request only |
| `task_status` / `task_checks_passed` / `task_artifacts` | the created task's final status, whether every acceptance condition passed, and the deliverable count |

The prompt assertions check what the model was actually shown: window, summaries, skill bodies.

### Runner

Per case: check `prereq`, create a temporary home, write `files`, open the server with a dummy key,
then replace the provider with a recording wrapper around the `Mock` (`jeeves-provider`), which
captures every request. Each turn runs the real `run_turn` through an event sink; interrupts go
through `interrupt_turn`, and `apply_patch` goes through the real `config/write`. The suite prints one
line per case, writes `eval_report.json`, `eval_report_<suite>.json` and appends to `eval_runs.jsonl`
under the home, and exits with the number of failures. A report write failure only warns.

### Writing a case

One case pins one behavior. Choose assertions by intent: behavior with `reply_*` and `tools_used`,
wiring with `prompt_contains`, safety boundaries with `tool_output_contains`. Add `prereq` for anything
the machine may lack. Run `jeeves eval evals/deterministic/<file>.jsonl`; the line must be a single line
of JSON.

## 8.3 Replay Eval

Entry: `jeeves eval --replay [path]` (default `evals/replay`; `just eval-replay`). Implementation:
`runtime/crates/jeeves-eval/src/replay.rs`.

A replay line has `id`, `schemaVersion`, `recorded.turns[]` (`say`, `replies`, `approval`, `cwd`) and
`expect`, plus optional `settings`, `files`, `prereq`, `task`. It is converted into the same
`Scenario`, so runner, assertions and reports are shared. The loader refuses a line without
`schemaVersion` and a line with empty `expect.events`.

Events match as an ordered subsequence, so extra streaming deltas do not fail a case. Within a turn,
`toolStarted` events appear as the run progresses and `toolCompleted` events together at the end;
expected sequences follow that order. A scripted approval answer is `yes`, `no` or `silence`
(`remember`: `once`, `session`, `always`).

The five files each pin one boundary: `compaction`, `approval` (silence becomes a refusal), `exec`
(incremental `toolDelta` before `toolCompleted`), `mcp` (disconnect and rebuild) and `continuation`
(step budget exhausted with an open todo list).

## 8.4 Long-Task Benchmark

Entry: `jeeves bench [path]` (default `evals/long`; `just bench`). Cases use the deterministic format.
Two numbers stay separate: task success (cases that reach the outcome their file declares) and
interrupt recovery (the subset tagged `recovery`). It records and always exits 0; its report lands in
`eval_report_long.json` besides the shared files.

## 8.5 Judge

Entry: `jeeves judge [path]` (default `evals/judge`; `just judge`). Cases carry `message` and
`judge.criteria`. A real provider resolved from the environment answers one turn; a grader model
scores it against a fixed rubric and returns JSON `{"score": 0-10, "reason": …}`. The grader should not
be the model under test; `JEEVES_JUDGE_MODEL` overrides the default. A score outside 0-10 fails judging
instead of entering the average. Without an API key the whole group is skipped and the command returns
0.

## 8.6 Live Reliability Example

`runtime/crates/jeeves-eval/examples/live_reliability.rs` is an explicitly invoked, budgeted run against a real
provider. Required flags: `--home`, `--budget-cny` (above 0, at most 10) and `--out`; `--suite` picks
`baseline`, `expanded`, `memory`, `research`, `research-guarded`, `support` or `research-long`.
Retained outputs live in `evals/results/`. `just test-research-publication` exercises the research
case's publication gate offline.

The [November 14 archive manifest](../../evals/archive/2026-11-14/sources.json) identifies the
validation report and source commit for each of the four originals. To verify their SHA-256 values in a fresh
clone, run `(cd evals/archive/2026-11-14 && shasum -a 256 -c SHA256SUMS)` from the repository root.

## 8.7 Smoke Scripts

Smokes use the real `jeeves` binary, because they test the process boundary that in-process evals skip.

| Recipe | Script | What it starts |
|---|---|---|
| `smoke` | `scripts/smoke.sh` | real `jeeves app-server` fed JSON-RPC over stdin, with `fake_sse_server.py` as provider |
| `smoke-resume` | `scripts/smoke-resume.sh` | app-server killed with `kill -9` mid-turn, then a fresh process resumes the checkpoint (POSIX) |
| `smoke-recovery` | `scripts/smoke-recovery.py` | SIGKILL at dispatch, effect and checkpoint boundaries with local provider and MCP fixtures (POSIX) |
| `smoke-web` | `scripts/smoke-web.py` | authenticated dashboard HTTP: settings, policy, streaming, approval |
| `smoke-interaction` | `scripts/smoke-interaction.py` | main conversation to task workspaces, follow-up and worker approval |
| `smoke-dashboard` | `scripts/smoke-dashboard.sh` | `jeeves runtime` driven with curl (HTTP to JSON-RPC, SSE) |
| `smoke-gateway` | `scripts/smoke-gateway.sh` | app-server plus the TypeScript gateways |
| `smoke-connector` | `scripts/smoke-connector.sh` | `jeeves connector feishu` against `fake_feishu_server.py` |
| `smoke-business` | `scripts/smoke-business.sh` | task API, inbound webhooks, tier routing (`fake_callback_server.py` for callbacks) |
| `smoke-desktop` | `scripts/smoke-desktop.py` | app-server with a scripted provider, as the desktop shell uses it |
| `smoke-browser` | `scripts/smoke-browser.py` | browser worker tests plus the offline browser/task flow (Playwright runtime from `browser-runtime`) |

Shared helpers are in `scripts/lib.sh`. All smokes run offline with isolated homes.

## 8.8 CI and `just check`

`.github/workflows/ci.yml` jobs:

| Job | What it runs |
|---|---|
| `rust` | `cargo fmt --check`, `clippy --workspace --all-targets -D warnings`, `cargo test --workspace` |
| `rust-coverage` | `cargo llvm-cov --workspace --fail-under-lines 60` |
| `windows` | build `jeeves-cli`; tests excluding `jeeves-tools`, then `jeeves-tools` skipping sandbox and spill tests |
| `schema` | `scripts/write_schema.py --check` |
| `docs` | `check_tool_docs.py`, `check_doc_paths.py`, `check_env_reads.py`, `check_i18n.py`, `check_config_docs.py` |
| `ts` | `npm ci`, `npm run typecheck`, `npm test` at the workspace root |
| `desktop` (macOS) | `just desktop-check`, `just smoke-desktop` |
| `gate` | build `jeeves-cli` and the dashboard, then `jeeves eval`, `jeeves eval --replay`, `smoke.sh`, `smoke-recovery.py`, `smoke-web.py` |

`just check` is the local superset: `fmt-check`, `check-app-server-schema`, `check-doc-paths`,
`check-doc-links`, `check-env-reads`, `check-tool-docs`, `check-config-docs`, `check-i18n`,
`ts-check`, `eval`, `eval-replay`, then `clippy -D warnings` and `cargo test --workspace`. Run it
before committing. Gateway, dashboard, connector, business, interaction, resume and browser smokes are
local recipes only.

### Doc checks

| Script | Fails when |
|---|---|
| `check_doc_paths.py` | a backticked repo path in a doc (outside `planning`) does not exist |
| `check_doc_links.py` | a relative link is dead, or an English doc lacks a `.zh.md` twin with the same heading levels |
| `check_tool_docs.py` | the tool inventory and counts in the tools doc or READMEs differ from the code's registry |
| `check_config_docs.py` | a `JEEVES_*` read by code is missing from `.env.example` or the configuration docs, or the reverse |
| `check_i18n.py` | the dashboard's English table lacks a used string, keeps an unused one, or placeholders differ |
| `check_env_reads.py` | process-environment reads appear outside `jeeves-config` and the host-level allowlist in the script |

### Protocol artifacts

`just write-app-server-schema` regenerates the TypeScript types and JSON Schema from
`jeeves-protocol`; `just check-app-server-schema` is the drift check. See
[01-protocol-and-config](01-protocol-and-config.md).

## 8.9 Known Gaps

See [limitations](../limitations.md). The judge needs a real key and is not run in CI. Smokes outside
the release gate are not run in CI.

Related: [architecture](../architecture.md), [protocol](../protocol.md),
[configuration](../configuration.md), [operations](../operations.md),
[03-model-and-loop](03-model-and-loop.md), [06-app-server](06-app-server.md),
[07-shells](07-shells.md).
