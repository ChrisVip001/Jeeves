# Conversation Runtime contract and acceptance

English | [简体中文](conversation-runtime.zh.md)

The main conversation can deliver a result immediately or continue the same goal in a durable task. This document records the implementation contract and acceptance evidence for all four phases. See [Chat, work and delivery](main-entry.md) and the [long-task roadmap](../planning/long-task-roadmap.md).

## One request that changes over time

| What the person says | What Jeeves does | What the person receives |
|---|---|---|
| Sends two screenshots. “Which is better for home use?” | Read the images and apply the person's stated needs. Ask one specific question if a missing detail could reverse the recommendation and available evidence cannot resolve it. | An answer in chat with the reason for the choice and any consequential uncertainty. |
| “Check the delivered price too. Include shipping.” | Compare the same variants using authorized sources. Ask for the delivery region only if it materially affects the answer. | A comparison in the same conversation with sources, lookup time and clearly identified unknown charges. |
| On iPhone after starting on Web, “Watch the second one for three days. Tell me if it drops below 1,800. Don't buy it.” | Resolve the reference to the compared variant. Carry forward the agreed price basis, threshold, end time, notification destination and purchase restriction. Create one durable assignment. | An acknowledgement in the originating conversation and one item in My work. The person can amend it and receive the result there. |

An explicit request can already confirm the goal. For an ambiguous request, investigate with the permissions at hand, state reversible assumptions, and clarify any remaining choice that changes the outcome, cost or authority. Goal confirmation and approval of a concrete external action are separate decisions. A comparison should succeed in chat even when it takes several tool calls. A request with a dedicated workspace or lasting verification criteria can start directly as a task.

## Refactor the execution contract

**Conversation and task ownership.** Private main chat selects inline delivery or delegation from the requested outcome. Per-operation admission depends on the user, source, workspace, device grant and action. Keep the restriction on mutations while an earlier effect is unresolved. The same Rust driver should serve conversation work, durable work and recovery. The app-server owns admission and scheduling. Shells submit human requests and render committed results. [Current tool assembly](../../runtime/crates/jeeves-app-server/src/turn/execution.rs) · [Architecture](../architecture.md)

**Moving work into a task.** Bind the originating message, confirmed goal, current amendments, evidence references, settled actions, remaining work and return destination in one recoverable transition. Retrying the request must recover the same task even if the process stopped between creation and dispatch. Keep prior measurements attached to the version they checked. A revised threshold supersedes the old one. Reuse the durable handoff and source ownership checks. An assignment retains the original authority boundaries through the transition. [Main-chat handoff](../../runtime/crates/jeeves-app-server/src/interaction/task_tool.rs) · [Task handoff](../../runtime/crates/jeeves-app-server/src/task/handoff.rs)

**Tool calls and recovery.** Give each proposed invocation a stable identity. Persist dispatch intent immediately before handing an effectful call to its executor, then commit its observed outcome before advancing to a dependent action. A crash between dispatch intent and a confirmed outcome leaves that call uncertain. Preserve proof of calls never admitted and results already committed. Independent reads can finish and report separately while their model-facing results retain request order. Return typed execution status, readable output and evidence references from the tool boundary. Derive task steps, notifications and verification inputs from those records. Remove status inference from text prefixes and reconstruction of executed calls from the model transcript. Codex's before-dispatch cancellation and Pi's explicit tool error flag informed these choices. [Loop](../../runtime/crates/jeeves-loop/src/lib.rs) · [Operation records](../../runtime/crates/jeeves-state/src/operations.rs) · [Task observations](../../runtime/crates/jeeves-agent/src/observe.rs)

**Model requests and context.** Record each request's purpose, execution owner, termination and provider-reported usage. At the adapter boundary normalize full input, output and cache breakdown. Charge the task for full input plus output once per request, including any planning, summarization or evaluation it invokes. Keep context occupancy, cumulative task consumption and monetary cost distinct. Preserve unknown usage on failed requests. Use current instructions, amendments, settled results, pending decisions and addressable evidence to assemble context for both continuation and recovery. Retain file, source and receipt references through compaction. Evaluate the benefit and cost of model-generated summaries against this baseline. OpenCode's structured summaries and Pi's file references are relevant precedents. [Provider usage](../../runtime/crates/jeeves-provider/src/lib.rs) · [Budget](../../runtime/crates/jeeves-loop/src/budget.rs) · [Current projection](../../runtime/crates/jeeves-loop/src/projection.rs)

**Delivery and verification.** Use the same typed criteria for a main-chat assignment, an explicit task and a resumed run. Derive criteria from the outcome the person requested. Return a sourced conclusion for research, an openable artifact for documents, the tested version and observed checks for code, and a service receipt for an authorized send. Record subjective review separately from mechanical checks. An acceptance command runs under normal execution policy. Replace the main-chat-only `file_checks` contract with the common verification types. Failed criteria drive correction in the same task and remain visible alongside the later measurement. [Existing verifier](../../runtime/crates/jeeves-agent/src/verify.rs) · [Delivery regression](../../runtime/crates/jeeves-app-server/src/interaction/tests/deliverables.rs)

## Implementation order and exit criteria

| Phase | Change | Evidence required to close |
|---|---|---|
| 1. Complete work in chat | Carry a request through the shared driver to an in-chat answer, comparison or approved action. Establish typed status and durable call boundaries for effects before enabling them here. Switch the main-chat prompt and tool assembly together. | From the normal composer, answer the screenshot question, deliver the sourced comparison, and preview and approve a one-off action in place. Check that all three leave the task count unchanged. Denial leaves the receiver untouched. A successful tool returning text beginning with `Error:` is recorded as successful. |
| 2. Continue the same goal over time | Implement durable handoff, amendments and origin-bound delivery. Use the three-day watch as the first complete journey. | Replay the handoff message, change the threshold from iPhone and restart. Assert one task, the same variant and shipping basis, the latest threshold and one notification for the qualifying event. A different Feishu chat or sender cannot continue the assignment. |
| 3. Unify recovery, usage and verification | Move remaining callers to per-call results, per-request accounting, shared context assembly and typed delivery criteria. | Interrupt “write report, send to Feishu, archive” before dispatch, after dispatch and after settlement. With a lost send receipt, reopen the saved report, reconcile only the send and show that archiving never started. Equal usage expressed in two provider formats must stop at the same budget boundary after restart. After two compactions and an amendment, verify the latest constraint and an early source. Wrong JSON, an unsupported citation and a failing test each keep the relevant delivery open for correction. |
| 4. Cut over clients and storage | Switch protocol, generated artifacts and consumers together. Retire superseded prompts, routes, status parsers and duplicate accounting. Rehearse the database transition on a copy of existing Mac data. | Compare accounts, tasks, artifacts and receipts before and after migration. Preserve the uncertainty of historical records. Repeat assignment, approval and delivery across self-hosted and isolated hosted Runtimes. Web and iOS share the conversation, Feishu retains its origin, and Mac acts only on the selected window within its grant. |

## Acceptance and closeout

### Phase 1 · Deliver in the main conversation

Phase 1 passed local acceptance on 2026-11-23. Run `just smoke-interaction --browser chrome` to upload two different screenshots in the main composer, receive a recommendation and ask for delivered prices. The comparison includes two accessible quote sources, the lookup time and an unconfirmed cost. Approving the send produces one receiver record matching the preview. Declining a second send leaves the count at one. Every turn creates zero tasks. The receiver returns a successful receipt beginning with `Error:`; the execution notification and durable operation both record success. This test exercises Chrome and the Runtime with a deterministic local model, quote sources and receiver. All eight `just smoke-recovery` scenarios also passed.

### Phase 2 · Continue the same assignment across devices

Phase 2 passed a local cross-device test using `just smoke-price-watch --ios-simulator <UDID>`. Replaying the Web handoff created one three-day watch. The iPhone 17 simulator changed its threshold from 1,800 to 1,700. After Runtime restarts, 1,660 + 40 shipping produced no alert and 1,659 + 40 produced one sourced receipt in the original chat. Another restart preserved the same receipt. The test uses local model and quote fixtures and advances the next scheduled check. Scope tests exercise enqueue rollback and reject Feishu follow-ups from another sender or chat.

### Phase 3 · Recover, account for usage and verify delivery

Phase 3 passed local acceptance on 2026-11-23. Run `just smoke-runtime-contract` to reproduce these four checks. The test starts real Runtime processes with local model protocol fixtures and a local Feishu receiver.

- Kill the process before send dispatch, after dispatch but before receipt persistence, and after receipt persistence. With the receipt lost, reopen the original report from task artifacts. Only the send requires reconciliation and the archive remains unexecuted. In the other two cases recovery finishes the archive and each receiver gets the report once.
- Anthropic and OpenAI Chat Completions each report 10 total input tokens and 3 output tokens per completed request. Both tasks stop at 26 tokens after restart, before archiving.
- Amend the ceiling to 17 in main chat. Five subsequent model requests carrying compacted context contain both the new constraint and the earliest source URL.
- A wrong JSON value, a quotation absent from the saved source and a designated test with a failing exit status each fail verification. All three pass after correction in the same task, with both measurements preserved.

### Phase 4 · Switch clients and migrate storage

The following checks passed on 2026-11-23.

| Environment | Observed result |
|---|---|
| Copy of the existing Mac databases | The v24 Runtime store migrated to schema 4 and reopened. Column comparisons preserved 1 gateway account, 3 tasks, 3 artifacts, 8 completions, 2 verification records and 10 approvals. All 26 historical operations without executor previews remained unverified. [Migration rehearsal](../operations.md) |
| Self-hosted and isolated hosted Runtimes | The Web composer completed assignment and one-time approval. In the isolated hosted stack, the initial write and revision each required their own approval. The downloaded report matched the expected text. |
| Chrome and iPhone 17 simulator | Both clients continued the same main conversation. The simulator amended the watch created on Web, and the revised threshold survived a Runtime restart. |
| Native Mac windows | The executor operated in the selected window. Switching windows invalidated the previous grant. An ambiguous window match produced no preview. |
| Live Feishu group | After reconnection the mentioned bot replied “Jeeves 群聊已收到 P4” in the original group. The Feishu sender, chat and message IDs matched the completed Runtime receipt. Separate regression tests rejected continuation from a different sender or chat. |

The repository gate `just check` passed. Reproduce the client checks with `just smoke-account`, `just smoke-interaction --browser chrome`, `just smoke-price-watch --ios-simulator <UDID>` and `just smoke-mac-window`. Use `PW_HEADED=1` to see Chrome during the browser checks.

### Reproducing the checks

Use the normal product entry points. During cloud startup, type a draft, reload and send once when ready. Verify the composer shows the working directory and selected item. Inspect a result from chat, expand execution details at its timestamp, then open a durable item on its latest result, pending decision and files. An approval must show the exact action and preview. Routing, limits and permission presets belong to the management interface. [Interaction design](../design/main-entry.md)

Record deterministic regressions and process-fault tests separately from held-out real-model trials and deployment checks. For model trials, count avoidable questions, unnecessary tasks, lost amendments, unsupported conclusions and independently verified deliveries. For receiver tests count actual external effects. Record model, elapsed time and usage. Distinguish simulated time, actual overnight operation and physical-device observations. Use `just check` and the relevant recovery and interaction smokes for implementation changes.
