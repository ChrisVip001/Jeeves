# ③ Model and loop: `jeeves-provider` and `jeeves-loop`

English | [简体中文](03-model-and-loop.zh.md)

`jeeves-provider` translates between Jeeves's one dialect and each vendor's API. `jeeves-loop` holds the working memory and repeats "ask the model, run tools, feed results back" until it should stop. The loop never learns which vendor it talks to: every vendor difference is confined to the wire adapters.

Directories: `runtime/crates/jeeves-provider/src/` and `runtime/crates/jeeves-loop/src/`. The loop is driven by `jeeves-app-server` (see [06-app-server](06-app-server.md)); tools come from [04-tools](04-tools.md).

## 3.1 The intermediate dialect

`lib.rs` defines the only types the loop and the tool layer know: `Message { role, content: Vec<ContentBlock> }`, `CreateRequest`, `CreateResponse { stop_reason, usage, content }`, `ToolSchema`, `Usage`.

- `ContentBlock` is internally tagged (`type`, snake_case), so its serialized form is byte-identical to Anthropic's content blocks. `anthropic.rs` therefore needs almost no translation.
- Besides `Text`, `ToolUse` and `ToolResult`, it carries `Image` (base64, no path), and continuation blocks that must be replayed verbatim: `Thinking` / `RedactedThinking` (Anthropic wire), `OpenAiReasoning` (Chat Completions `reasoning_content`), `ResponseItem` (Responses encrypted reasoning). A wire never sends another wire's continuation blocks; Anthropic and Chat Completions reject a `ResponseItem` or a Responses-tagged `ToolUse.extra`, and Anthropic rejects `OpenAiReasoning`, rather than silently dropping them.
- `ToolUse.extra` is an opaque slot, skipped in serialization when `None`. Gemini thinking models attach a `thought_signature` to a tool call and require it back unchanged; on the Chat Completions wire it travels as `extra_content`.
- `StopReason` has a `#[serde(other)]` catch-all so a new vendor value cannot crash a turn. `UncertainEffect` is set by the loop, never by a wire.
- `Usage.input_tokens` counts full input. The Anthropic adapter adds fresh, cache-read and cache-creation tokens. OpenAI already includes cached input in its total. Cache details are recorded separately. Each model invocation retains its purpose, owner, termination and reported usage. Known full input plus output counts once toward the task budget. Interrupted requests without a provider measurement retain unknown usage.

## 3.2 The provider catalog

`PROVIDERS` in `lib.rs` is the compile-time table; each row is a `ProviderInfo`: id, `Wire`, `key_env`, `base_url`, default `model` and `small_model`, an approximate `context_window`, and `vision`. Read the table itself for the current vendors and defaults; do not copy them into prose.

`Wire` has three values, and its only job is to pick the HTTP client:

| Wire | Client | Used for |
|---|---|---|
| `Anthropic` | `anthropic.rs` | Anthropic and the vendors that expose an Anthropic-compatible endpoint |
| `Responses` | `responses.rs` | OpenAI |
| `OpenAi` | `openai.rs` | Chat Completions-compatible gateways, including local Ollama |

An empty `key_env` declares a local keyless endpoint (`needs_key()` is false); `resolve_with` and the Chat Completions client both branch on it.

`catalog.rs` extends the table at boot from `<home>/models-catalog.json`: a row with a known id patches that provider field by field (`deny_unknown_fields`, so a typo is an error, not a silent no-op); an unknown id appends a new provider, which needs at least `model` and `wire`. A broken file or row costs one stderr line and keeps the built-in table. The merged `Catalog` is immutable after boot.

Context windows are deliberately conservative: underestimating only compacts earlier, overestimating overflows. `vision` gates a local, readable refusal of images for text-only providers instead of a remote 400.

## 3.3 Resolving a client

`resolve_with(settings, catalog)` returns `Resolved { provider_id, client, model, small_model, vision, context_window }`:

1. Look up `settings.provider` in the catalog; unknown ids list the valid ones.
2. **The key has exactly one source**: `settings.api_key`, a runtime snapshot that `jeeves-config::models` fills from the active `models.json` entry's credential pointer (environment variable, keychain or fallback file; see [01-protocol-and-config](01-protocol-and-config.md)). There is no implicit fallback to `info.key_env`: a turn that runs only because the shell happens to export a variable is the hardest kind of bug to trace. A keyless provider gets an empty string. A missing key yields `no_key_message`, which says where to get one and which `/model add` command to run.
3. `settings.model` / `small_model` / `base_url` override the catalog row; `small_model` falls back to the row's value, then to the main model. `vision` and `context_window` come from the profile's declaration first, then the row.
4. `capabilities::validate` checks declared model capabilities (positive window, a known `reasoningEffort` that requires the Responses wire) before any client is built.
5. Build the client for the wire with `settings.llm_timeout_secs` (one timeout per client, no per-request override) and `settings.llm_retries`. Anthropic gets `with_thinking`; Responses gets `with_reasoning`; Chat Completions gets `reasoning_content` echo for DeepSeek models.
6. Wrap it in `capabilities::Checked`, which refuses before any paid call when a tools request goes to a model declared `tools=false` or to a model that needs the Responses wire.

`Resolved` has a hand-written `Debug` that omits the client, so a key can never reach a log. `resolve()` resolves against the built-in table only, for tests and callers without a home.

## 3.4 The `Provider` trait

```rust
trait Provider: Send + Sync {
    fn create(&self, req) -> Pin<Box<dyn Future<Output = Result<CreateResponse, ProviderError>> + Send + '_>>;
    fn stream(&self, req, on_text: TextSink) -> Pin<Box<…>> { Box::pin(self.create(req)) }
}
```

Two methods only; a smaller interface lets more providers plug in. The futures are boxed by hand because `Resolved` stores an `Arc<dyn Provider>` and `async fn` in traits is not object-safe. `stream` calls `on_text` as text arrives and still returns the fully assembled `CreateResponse`, so the loop is identical on both paths; the default falls back to `create`.

## 3.5 The three wires

### Chat Completions (`openai.rs`)

`to_openai` and `from_openai` are the whole translation, both pure functions.

- `system_text()` becomes `messages[0]`. Assistant text and all its tool calls merge into one assistant message (`content` null when empty); `extra` goes to the call's `extra_content`; `OpenAiReasoning` goes to `reasoning_content`.
- A user message carrying `ToolResult` blocks becomes one `role: "tool"` message per result; any plain text in that message is dropped, since this wire has no shape for both. A plain user message with images becomes array content with data-URI parts.
- The token limit is sent as `max_completion_tokens`; `tools` only when non-empty. No `cache_control`: unknown fields are a 400 on some gateways.
- `from_openai` turns a body with an `error` key (OpenRouter answers rate limits with 200 + error) or without `choices` into `ProviderError::Api`. `stop_reason` is inferred: `ToolUse` if there are tool calls, else `EndTurn`.
- The one parameter retry lives in non-streaming `create_inner`: a 400 whose body mentions `max_completion_tokens` or `max_tokens` is resent once with the old `max_tokens` name, for older gateways. It is separate from the backoff in 3.6.
- Streaming adds `stream_options.include_usage`; without it streamed usage is all zeros and the trace costs are fake. Content deltas go to `on_text`; tool calls are keyed by `index` in a `BTreeMap` (id/name overwrite, `arguments` fragments append) and parsed only at the end with `tool_input`. The stream is complete only after `[DONE]` or a `finish_reason`; otherwise it is `Incomplete`. `finish_reason` maps to `StopReason` (`tool_calls`/`function_call` to `ToolUse`, `length` to `MaxTokens`, `stop` to `EndTurn`).
- An empty key sends no `Authorization` header: an empty `Bearer ` header makes some gateways answer 401. Local endpoints take this path.

### Anthropic (`anthropic.rs`)

- Headers are `x-api-key` and a pinned `anthropic-version`. `tools` is omitted when empty (some endpoints reject an empty array).
- With a `cache_prefix`, `system` becomes two text blocks, the prefix carrying `cache_control: ephemeral`, and the last non-thinking block of the last message gets a second breakpoint (see 3.8).
- Extended thinking is enabled only for the configured main model and only when the budget is at least 1024 and below `max_tokens`; otherwise the request goes out without thinking instead of failing. Thinking blocks, with signatures, are kept in working memory and sent back first in the assistant turn.
- Streaming events: `message_start` gives input and cache tokens, `content_block_start` opens a thinking/tool slot, `content_block_delta` carries `text_delta` (to `on_text`), `input_json_delta`, `thinking_delta`, `signature_delta`; `message_delta` gives the stop reason and output tokens; `error` becomes `ProviderError::Api`. A stream without `message_stop` is `Incomplete`.

### Responses (`responses.rs`)

Stateless: every request is sent with `store: false` and `include: ["reasoning.encrypted_content"]`, and the encrypted reasoning items come back as `ResponseItem` blocks that local working memory replays on the next request. A response whose reasoning lacks the encrypted continuation is an error. Streaming forwards text and refusal deltas to `on_text` and returns the assembled response from the terminal `response.completed` / `response.incomplete` event; a stream that ends without one is `Incomplete`.

Malformed tool arguments never fail the response: `tool_input` decodes them (empty means `{}`, an object encoded twice as a string is decoded once more) and otherwise keeps the call with a marked stand-in input, which the registry answers with an `Error:` result naming what was received, so the model can call again.

### SSE framing (`sse.rs`)

`data_lines` turns the HTTP byte stream into one `data:` payload per item and drops `event:` lines, comments and blank lines. At EOF a final unterminated `data:` line is still yielded. It does not interpret `[DONE]`; each wire decides. Framing is the only thing shared by all wires.

## 3.6 Retries and errors

`ProviderError` separates `Http { status, body, retry_after_secs }`, `Network`, `ContextOverflow`, `Api` (2xx with a wrong body), `Parse` and `Incomplete` (a stream that closed before its terminal event). Every HTTP status of 400 or above goes through `from_http_retry`, so overflow detection has one implementation: only 400/413/422 whose body matches a phrase list (English and Chinese) become `ContextOverflow`.

`retry.rs` retries only 429, 5xx, network errors and `Incomplete` (`retryable()`); other errors would fail identically. The invariants:

- **Bounded**: `settings.llm_retries` attempts, exponential backoff from 500 ms capped at 8 s per wait and 30 s total; a server wait is honored under the same caps: `retry-after-ms` first, then `Retry-After` as seconds or an HTTP date.
- **Visible**: each retry emits a `RetryNotice`. The outlet is a task-local (`with_note_sink`) rather than a `Provider` parameter, so the trait stays a stable contract and a turn's retries cannot leak into another concurrent turn. With no outlet it falls back to stderr.
- **Never switches vendors**: that is a different decision.
- `with_retries` wraps getting the response. `stream_with_retries` starts a streamed call again when it ends `Incomplete` before any text reached `on_text`; once text was shown, the break is reported instead, because a retry would print it twice.
- `ContextOverflow` is never retried here; app-server compacts history and re-runs the loop once (see [06-app-server](06-app-server.md)).

## 3.7 Token estimation

`tokens.rs` answers one question, how close a request is to the window, so the caller can decide when to compact. Billing and trace numbers always come from provider-reported `usage`; the estimate only needs the right magnitude.

- The encoder is cl100k with its vocabulary compiled into the binary (no network), a cross-vendor approximation that overestimates Chinese. Early compaction is acceptable; underestimating (overflow, a wasted request) is not.
- `estimate_text`, `estimate_messages` (per-message overhead) and `estimate_tools` (per-request overhead) exist because tool declarations are a real share of the context once MCP tools are connected. Images are estimated from byte length, clamped.
- The loop also keeps the last request's estimate and measured prompt size in `LoopResult`; app-server uses their ratio to calibrate the next turn.

## 3.8 Prompt segmentation and cache breakpoints

`CreateRequest` splits the system prompt into `cache_prefix` (soul, model identity, method paragraph: byte-identical within a session) and `system` (per-turn tail: clock, todos, memory hits, skill bodies, summary).

- Anthropic: `cache_control` at the end of the prefix and on the last message. Other wires: `system_text()` concatenates, adds no field, and relies on their automatic prefix cache, which needs only that the prefix not move.
- The tail is never inside the cache boundary; a forced compaction rebuilds only the tail so the prefix still hits.
- The clock in the tail has day granularity: an hour-granular timestamp would invalidate cache entries by itself.
- Cache read/creation tokens are written to the usage ledger, so hit rate is measured.

## 3.9 `Mock`

`mock.rs` implements `Provider` from a scripted queue: `Mock::new` (responses), `with_outcomes` (successes and failures, e.g. `context_overflow()`), `streaming` (text in two-character chunks with a 5 ms await between, so interrupts have a deterministic point to cut in), `text` / `tool_use` helpers. An exhausted queue is an error rather than a reused reply. `received` records every request for prompt assertions. Tests and evals swap the whole network layer through it.

## 3.10 The loop: `Turn`

`jeeves_loop::run(Turn)` takes everything one loop needs:

- the model client, model name, a fitted output ceiling and optional resource budgets (see [the model catalog](../configuration.md#the-model-catalog-models-catalogjson) for how the catalogue and remaining context choose it);
- `system_prefix` and `system` (see 3.8), `history` (already trimmed by app-server) and `user_message` kept separate because they come from different places (database versus request), plus `user_images` that ride only on the first user message;
- `tools` (`ToolRegistry`) and `ctx` (`ToolCtx`);
- outlets and controls: `observer`, `on_text`, `interrupt`, `checkpoint`, `steering`, `resume`;
- `tool_result_budget` (3.16).

`resume` continues a checkpointed working memory.

## 3.11 Events and the two text outlets

`LoopEvent` is `Llm`, `Text`, `ToolStart`, `ToolDelta` (live output of a still-running tool), `Tool`; tool events carry the model's `tool_use_id`, since several calls can be in flight.

`on_text` and `observer` are different channels. `on_text` is the UI's text stream. `observer` is the server's event stream (notifications, trace). A `Text` event and `on_text` are two destinations for the same delta. The loop always attaches an internal recorder, and picks `client.stream` when either outlet exists and `client.create` when neither does; the logic after the call is identical. Because of the recorder, the already-streamed text can be recovered on interrupt even with no `on_text`.

## 3.12 One iteration

Per iteration, in order:

1. Interrupt check (3.13).
2. Steering: messages added through `turn/steer` while the turn runs are folded in as ordinary user messages at this boundary, so finished tool results stay valid. `Steering` is one list with two readers: the loop drains what it has not seen; app-server reads all of it to write the transcript.
3. Wrap-up decision (3.14).
4. `trim_tool_results` (3.16), then build the `CreateRequest` (no tools on a wrap-up call) and record the local estimate.
5. Call the model, racing the interrupt. Add usage, emit `Llm`, push the assistant content.
6. No tool uses: return. Otherwise write a checkpoint, run the tools, push one user message of `ToolResult` blocks, and write another checkpoint **before** the next model call, so a crash resumes at exactly that boundary without re-running finished tools. A checkpoint failure aborts the turn.

Independent reads run concurrently. The client can receive each result as soon as it is recorded. The model receives results in proposal order. Writes, commands and delegation each run exclusively. An uncertain external effect stops the loop with `StopReason::UncertainEffect` at a durable checkpoint for reconciliation.

## 3.13 Interruption

`Interrupt` is an atomic flag plus a `Notify` (no `CancellationToken`; one flag does not justify a dependency). `wait()` registers `notified()` before re-checking the flag, which closes the lost-wakeup race.

Three safe points: the start of an iteration; the model call (`select!` against the call, and the losing future is dropped, closing the connection so no tokens burn in the background); tool execution (checked before each batch, and raced while it runs, where dropping the `JoinSet` aborts tasks in flight and the already-finished ones are still reported).

An interruption is not an error: it returns `Ok(LoopResult { interrupted: true, .. })` with the text streamed so far (or a one-line placeholder), the tool calls that actually ran, and the iterations completed. The layer above persists it as is. Compaction before the loop is not part of the race; see [limitations](../limitations.md).

## 3.14 Exits and wrap-up

The loop delivers a reply when the model requests no tools and there is no pending user input. Cancellation, uncertain effects, exhausted resources, repeated tool batches and provider errors have distinct exit paths. Tool calls from a truncated model response are discarded. A reply cut by the output limit (`MaxTokens`, not the turn's token budget) is continued up to twice: the loop sends a note asking the model to carry on without repeating itself, and the delivered reply joins the pieces. A third cut ends the turn with `MaxTokens`. A reply that ends the turn with no visible text and no tool call (for example only a thinking block) gets one note asking the model to answer; if the next reply is empty too, the turn ends with a fixed line saying the model returned no text, so the person never sees an empty reply.

When the context estimate crosses its threshold, the request keeps the current instructions and recent complete tool exchanges, with excerpts from older messages. Excerpts are budget-truncated, so the compaction note separately retains two kinds of fact deterministically: the source URLs, file paths and receipt ids seen so far, and each tool's latest failure reason that no later success replaced (keyed by tool name; a newer failure of that tool replaces the older one, and a later success clears it). A compacted turn therefore does not retry a call it already knows fails. The full transcript remains available to checkpoint and account for the work. On a provider overflow during execution, the Runtime can retry that request once with less context and the settled tool results.

## 3.15 Loop guard

`guard.rs` keeps a model from repeating tool calls without progress. `StallGuard` is a pure detector over `(tool, args, result)` hashes in request order, created per turn (cross-turn repetition is a different mechanism in app-server).

| Criterion | Trigger |
|---|---|
| Repeated | The same call and result a 3rd consecutive time (a single repeat may be a harmless recheck) |
| Cyclic | The last `2p` calls are pairwise identical for `p` in 2 to 4 (alternation resets the consecutive count, so it needs its own test); a window that is all one call is "repeated", not a cycle |
| Failing | The same tool failed a 3rd consecutive time with any arguments; a success of that tool resets its count |

Two kinds of call are left out of the repeat and cycle rows. A successful poll (`job` with `action=output`, `job_output`, or a tool named `…_poll` or `…_get_result`) is not counted, because waiting on a running job returns the same text until it moves. A successful `write_file`, `edit` or `apply_patch` clears the window, so rerunning the same test after a change starts counting again.

On a hit, only the text fed back to the model changes: a `[guard]` reminder is appended (take another route or tell the user where you are stuck), and for a repeated call whose result is long enough, the body is replaced by a short reference stub. `LoopResult.tool_calls` and `Tool` events keep the real result. Hit count and the last note go to `LoopResult.guard`. Three successive batches with exclusively repeated or cyclic calls stop for review. A failing streak only adds a reminder; it never counts toward that stop, because a failing test command is ordinary output.

That reminder states the move the failure actually calls for, not a generic "take another route". `FailureKind` classifies the streak deterministically from the tool's own error text, never from the model's account of it:

| Kind | Read from | What the reminder asks for |
|---|---|---|
| `Transient` | timeout, connection reset, 429, 5xx | One more attempt is reasonable; if it fails again, say so instead of retrying |
| `Missing` | no such file, not found, 404 | Stop guessing names or paths; list or search the containing location once |
| `Permission` | 403, 401, unauthorized, missing credential or approval | Retrying cannot fix it — tell the user what access is needed |
| `Arguments` | schema mismatch, missing field, parse failure | Re-read the schema and the error, send one corrected call |
| `State` | conflict, 409, precondition, already exists, locked | Re-read the current state before calling again |
| `Unclassified` | unrecognized, or a streak whose causes disagree | Keeps the existing generic reminder |

The kind is held over the streak: the causes within one tool's run of failures must agree, and a mixed streak degrades to `Unclassified` rather than advising for whichever error came last. A success of that tool clears the recorded cause.

## 3.16 Tool-result budget

`run_command` truncates its own output, but MCP tool output is unbounded, which makes tool results the one unattended growth point within a turn. Before each request, `trim_tool_results` checks the total size of results in working memory. When it exceeds the total cap, it stubs eligible results **oldest first** until it fits; the newest `keep_recent` results and `run_command` results are never touched, and only results over the per-result threshold qualify. Limits and defaults are in [configuration](../configuration.md).

- Age, not size, is the order: the result the model is reasoning about right now should not be the first to disappear.
- The stub (`jeeves_tools::spill::stub`) saves the full original to disk **before** cutting, then keeps head and tail as whole lines plus an omission note. Its target size is clamped well below the threshold, otherwise a result could be eligible but impossible to stub.
- Idempotent: it runs every iteration and skips results that are already stubs.
- Never silent: replacements and a still-over-budget outcome each print a line. When the protected results alone exceed the cap, it says so instead of exceeding the budget quietly.
- Off when the totals are zero.

## 3.17 `LoopResult` and invariants

`LoopResult` carries `stop_reason`, `reply`, `tool_calls` (`ToolOutcome`: id, name, output, duration), `iterations`, summed `usage`, the complete working `messages` (for the trace), `interrupted`, the guard report, and the estimated and measured input tokens.

When the first operation is admitted, remaining calls in its proposal are recorded as unexecuted. Each call records dispatch intent before execution and settles before its dependent action can start. Task steps use the [durable operation results](04-tools.md#the-registry-contract).

Invariants:

- The loop sees only the intermediate dialect; vendor differences live in the wires.
- Interrupts, wrap-ups and uncertain effects return `Ok(LoopResult)`. Provider errors and checkpoint failures return `Err`, which the layer above handles (context overflow is compacted and retried once there).
- Every model call's usage is accumulated; every `tool_use` gets a `tool_result`.
- Retries are bounded and visible; streaming and non-streaming yield the same `CreateResponse`.
- Tool results fed back may be altered by the guard and the budget; the recorded outcomes are not.
