# Protocol reference (app-server v2)

English | [简体中文](protocol.zh.md)

`jeeves app-server` speaks **newline-delimited JSON-RPC 2.0** on stdin/stdout:
one request per line; responses and notifications interleave (notifications
before the response). Every type is defined in
`protocol/src/protocol/v2.rs` and `protocol/src/rpc.rs` and generated into TypeScript
(`protocol/schema/typescript/v2/`).
**Changing the protocol requires `just write-app-server-schema`.**

## Methods

| Method | Request → Response | Purpose |
|---|---|---|
| `turn/operations` | `TurnOperationsParams` → `TaskOperationsResponse` | inspect ordinary-chat uncertainty in one session, optionally one turn |
| `turn/reconcile` | `TurnReconcileParams` → `TaskOperationsResponse` | human evidence for an exact ordinary-chat operation; no execution or resume |
| `turn/start` | `TurnStartParams` → `TurnStartResponse` | run one conversation turn; the response follows all notifications. A second turn on a busy session is refused with `SESSION_BUSY` |
| `turn/interrupt` | `TurnInterruptParams` → `TurnInterruptResponse` | cancel a running turn; `interrupted:false` = already finished |
| `turn/steer` | `TurnSteerParams` → `TurnSteerResponse` | add a line to the **running** turn: it lands at the next round boundary, finished tools are never re-run |
| `interaction/start` | `InteractionStartParams` → `TurnStartResponse` | human entry to the main conversation: routes into discussion, clarification or independent tasks (see [Main human interaction](#main-human-interaction)) |
| `turn/follow-up` | `TurnFollowUpParams` → `TurnFollowUpResponse` | have the server start the next turn after this one (per-session subscription) |
| `turn/list` | `TurnListParams` → `TurnListResponse` | turns currently running, filterable by `sessionId` |
| `approval/respond` | `ApprovalRespondParams` → `ApprovalRespondResponse` | answer one approval request; `accepted: false` when it arrived too late. Optional `actor` (e.g. `lark:<open_id>`) is recorded in the approval log; absent = `owner`. Optional `version` must match the question's current version, else a text error (see [Pending approvals across a restart](#pending-approvals-across-a-restart)) |
| `approval/pending` | `ApprovalPendingParams` → `ApprovalPendingResponse` | live pending approval requests of a session |
| `approval/log` | `ApprovalLogParams` → `ApprovalLogResponse` | the approval audit: the last 100 records, four outcomes `approved` / `denied` / `timeout` / `voided` |
| `todo/view` | `SessionViewParams` → `TodoListView` | the current todo sheet; null when the model never wrote one |
| `task/create` | `TaskCreateParams` → `TaskEnvelope` | create a task: session optional, with acceptance, budget and working directory |
| `task/start` | `TaskStartParams` → `TaskEnvelope` | human-authorized durable start in a separate execution session |
| `task/wait` | `TaskWaitParams` → `TaskEnvelope` | declare one immutable external input condition before starting |
| `task/wake` | `TaskWakeParams` → `TaskWakeResponse` | an inbound channel message wakes the task waiting for that source and event key, once per source key (see [Durable external input](#durable-external-input)) |
| `task/wake` | `TaskWakeParams` → `TaskWakeResponse` | an inbound channel message becomes the input of the task waiting for that source and event key, once per source key |
| `task/signal` | `TaskSignalParams` → `TaskSignalResponse` | supply one correlated input event through an authorized host |
| `task/operations` | `TaskGetParams` → `TaskOperationsResponse` | the task's operation journal (see [Operation journal](#operation-journal-and-human-reconciliation)) |
| `task/reconcile` | `TaskReconcileParams` → `TaskOperationsResponse` | human evidence for one unresolved operation |
| `task/list` | `{cursor?}` → `TaskListResponse` | newest first, up to 100 tasks per page. Pass the returned `nextCursor` to read older tasks |
| `task/view` | `SessionViewParams` → `TaskViewResponse` | the session's current task card |
| `task/events` | `TaskEventsParams` → `TaskEventsResponse` | task events after a cursor, oldest first (see [Task event cursor](#task-event-cursor)) |
| `capability/list` | — → `CapabilityListResponse` | one card per model profile, MCP server and connector (see [Capability cards](#capability-cards)) |
| `capability/probe` | `CapabilityProbeParams` → `CapabilityProbeResponse` | re-check one card by id |
| `connector/tools` / `connector/tools/set` | `CapabilityProbeParams` / `ConnectorToolsSetParams` → `ConnectorToolsResponse` | inspect tools or change a user connection’s tool switches via authenticated `GET /api/connector/tools?id=…` and `POST /api/connector/tools/set` |
| `connector/catalog` | — → `ConnectorCatalogResponse` | curated connectors, setup methods, documentation and manifests, also at authenticated `GET /api/connector/catalog` |
| `connector/settings` | — → `ConnectorSettingsResponse` | human view of calendar/mail configuration, credential references only |
| `connector/settings/save` | `ConnectorSettingsSaveParams` → `ConnectorSettingsResponse` | atomically save exactly one validated section |
| `connector/oauth/start` | `ConnectorOAuthStartParams` → `ConnectorOAuthStartResponse` | prepare a real loopback OAuth authorization URL |
| `task/integrate` | `TaskIntegrateParams` → `TaskGitCheckpointResponse` | Merge a reviewed child head into its pending parent worktree |
| `task/gitCheckpoint` | `TaskGitCheckpointParams` → `TaskGitCheckpointResponse` | human-confirmed local commit of explicit paths in an inactive task worktree |
| `task/gitCheckpointPolicy` | `TaskGetParams` → `TaskGitCheckpointPolicyResponse` | inspect the task's automatic checkpoint opt-in |
| `task/gitCheckpointPolicy/set` | `TaskGitCheckpointPolicyParams` → `TaskGitCheckpointPolicyResponse` | human confirms exact file paths for automatic checkpoints, or disables the policy |
| `task/get` | `TaskGetParams` → `TaskEnvelope` | one task: acceptance, budget, spend, run count |
| `task/runs` | `TaskGetParams` → `TaskRunsResponse` | execution records: when each run started, ended, and why it stopped |
| `task/artifacts` | `TaskGetParams` → `TaskArtifactsResponse` | the deliverables (files that really exist) |
| `task/reviewCapture` | `TaskReviewCaptureParams` → `TaskReviewView` | attach two explicit Git commits to a completed older delivery |
| `task/suspend` | `TaskActionParams` → `TaskEnvelope` | pause: keep the checkpoint, lose no finished tool |
| `task/resume` | `TaskActionParams` → `TaskEnvelope` | continue from the checkpoint |
| `task/cancel` | `TaskActionParams` → `TaskEnvelope` | cancel (terminal, no auto-continue) |
| `session/permission/get` | `SessionPermissionGetParams` → `SessionPermissionResponse` | session policy, revision and capability flags; `mode:null` preserves legacy runtime rules |
| `session/capabilities` | `SessionCapabilitiesParams` → `SessionCapabilitiesResponse` | a shell declares its platform, transport and feature names; the engine returns `accepted` / `unsupported` |
| `session/permission/set` | `SessionPermissionSetParams` → `SessionPermissionResponse` | human-controlled policy change; requires matching `expectedRevision` and confirmation when elevating to full access |
| `session/main` | none → `SessionNewResponse` | idempotently open the persistent `jeeves-main` session |
| `session/list` | `SessionListParams` → `SessionListResponse` | the sole public conversation |
| `session/get` | `SessionGetParams` → `SessionGetResponse` | one session's metadata, using the same [SessionSummary](../protocol/schema/typescript/v2/SessionSummary.ts) as `session/list` |
| `session/update` | `SessionUpdateParams` → `SessionUpdateResponse` | rename / change directory (`null` = leave alone, `""` = clear). An unnamed session may get a short generated title after its first exchange; a name set here is never replaced |
| `session/messages` | `SessionMessagesParams` → `SessionMessagesResponse` | messages, newest first. On the newest task page, optional `activeInputs` lists the current instruction and accepted follow-ups until the turn enters history |
| `memory/search` | `MemorySearchParams` → `MemorySearchResponse` | keyword search over facts + episodes |
| `memory/list` | `MemoryListParams` → `MemoryListResponse` | recent facts (cursor-paged) |
| `memory/list-episodes` | `MemoryListEpisodesParams` → `MemoryListEpisodesResponse` | recent episodes |
| `memory/setValidity` | `MemoryValidityParams` → `MemoryValidityResponse` | human replaces fact validity bounds |
| `memory/provenance` | `MemoryProvenanceParams` → `MemoryProvenanceResponse` | inspect validity and extraction contexts |
| `memory/remember` | `MemoryRememberParams` → `MemoryRememberResponse` | store a fact |
| `memory/forget` | `MemoryForgetParams` → `MemoryForgetResponse` | delete by subject |
| `config/read` | `ConfigReadParams` → `ConfigReadResponse` | full configuration view |
| `config/write` | `ConfigWriteParams` → `ConfigWriteResponse` | apply a patch, hot-reload (persisted to settings.json) |
| `model/list` | `ModelListParams` → `ModelListResponse` | the model catalog |
| `model/profiles` | `ModelProfilesParams` → `ModelProfilesResponse` | configured profiles + the active one (**no secrets**, only a "where the key lives" pointer) |
| `model/save` | `ModelSaveParams` → `ModelSaveResponse` | create/update a profile and make it active; `id` + `makeActive` alone = switch |
| `model/remove` | `ModelRemoveParams` → `ModelRemoveResponse` | delete a profile and its credential |
| `model/routes` | — → `ModelRoutesResponse` | the routing table: `fixed` / `auto` plus three "tier → profile id" rows (an unconfigured tier is `null`) |
| `model/routes/save` | `ModelRoutesSaveParams` → `ModelRoutesResponse` | replace the whole table; tier names and profile ids are validated before writing |
| `usage/summary` | `UsageSummaryParams` → `UsageSummaryResponse` | the usage ledger: per-model aggregates + totals. **Accounting only, no enforcement** |
| `responsibility/reports` | `ResponsibilityReportsParams` → `ResponsibilityReportsResponse` | last 50 occurrences, delivery states and live approval cards |
| `responsibility/prepare` | `ResponsibilityContract` → `ResponsibilityContract` | validate and normalize the proposed definition for review |
| `responsibility/sample` | `ResponsibilityContract` → string | run a read-only preview for the proposed definition |
| `responsibility/amend` | `ResponsibilityAmendParams` → `ResponsibilityView` | replace a duty at its expected revision and hold outstanding wakes for review |
| `responsibility/register` | `ResponsibilityContract` → `ResponsibilityView` | human registration of a recurring duty |
| `responsibility/list` | `{}` → `ResponsibilityListResponse` | list contracts, stop state and last due minute |
| `responsibility/setPaused` | `ResponsibilitySetPausedParams` → `ResponsibilityView` | pause or resume an unexpired duty |
| `schedule/list` | `ScheduleListParams` → `ScheduleListResponse` | every declared job (stored and skill-owned) with runtime state and next run |
| `schedule/save` | `ScheduleSaveParams` → `ScheduleChangeResponse` | add or replace one stored job (validated; a skill-owned name is refused) |
| `schedule/remove` | `ScheduleRemoveParams` → `ScheduleChangeResponse` | delete one stored job (a skill-owned job is refused) |
| `browser/list` | `BrowserListParams` → `BrowserListResponse` | browser availability plus owned browser sessions (human-only) |
| `browser/start` | `BrowserStartParams` → `BrowserSessionView` | start a task browser (human-only) |
| `browser/control` | `BrowserControlParams` → `BrowserSessionView` | control a browser with the expected generation (human-only) |
| `browser/frame` | `TaskGetParams` → `BrowserFrameResponse` | one bounded on-demand frame (human-only) |
| `task/file` | `TaskFileParams` → `TaskFileResponse` | original bytes of a listed managed workspace file (human-only); cockpit posts `action: "file"` to `/api/task` |
| `memory/ledger` | `MemoryLedgerParams` → `MemoryLedgerResponse` | one session's ledger, newest first; with `turnId` also that turn's entries. Read-only |
| `memory/index` | — → `MemoryIndexResponse` | the bundle's catalog (`index.md`) plus one summary per page. Read-only |
| `memory/page` | `MemoryPageParams` → `MemoryPageResponse` | one page with body and `sources`; `page: null` when there is no such page |
| `context/usage` | `ContextUsageParams` → `ContextUsageResponse` | the per-section context breakdown; `usage: null` = nothing measured yet |
| `skill/stats` | — → `SkillStatsResponse` | aggregated `<home>/skill-hits.jsonl`. Read-only |
| `session/compact` | `SessionCompactParams` → `SessionCompactResponse` | fold older turns into the rolling summary **now**, Human-only: no tool reaches it |

Conventions: methods are `<resource>/<method>` (singular resource); payloads
are named `*Params` / `*Response` / `*Notification`; wire fields are
camelCase; integers are `i32` only (`i64` maps to ts-rs `bigint` while
`JSON.parse` yields `number` — a runtime mismatch); paged list methods (`memory/list`, `session/list`, `session/messages`) take
`cursor`/`limit` and return `data` + `nextCursor`.

### Using the goal loop

A goal is a clarified outcome, not necessarily a persisted object. A goal that runs inside the conversation never reaches this protocol. Only a goal that needs a durable task thread is stored as `goalCondition` on the task row, which is why there is no `goal/set` / `goal/view` method — a human writes it through
`/goal` or `task/create {goalCondition}`, and the loop reads the same row. The only protocol
surface left is the `GoalRound` notification (one per judgement):

```
you>  /goal make the tests green
       → task/create  {sessionId: "…", title: "make the tests green", goalCondition: "make the tests green"}
… the turn ends without satisfying it …
       ← GoalRound {turnId, round: 1, maxRounds: 5, status: "continuing", reason: "two red"}
… another round …
       ← GoalRound {turnId, round: 2, maxRounds: 5, status: "satisfied", reason: "all green"}
```

`TurnMeta.goalStatus` / `goalRounds` carry the outcome and the count into the turn's
metadata. `TurnMeta.routeReason` (optional) says how the model was chosen this turn —
`auto:reasoning → ds-r1` or `fallback: …`; empty when nothing was chosen.

`TurnStartParams.requestId` (optional, 1-120 characters of `[A-Za-z0-9_-]`) names the message a turn answers. A second `turn/start` with the same id in the same session replays the first turn's notifications and response without calling the model again; the same id with a different message or parameters is refused. The receipt lives in `interaction_receipts` under a `turn:` key, so it survives a restart. Without `requestId` nothing changes.

The optional `latestDelivery` on `task/artifacts` identifies the most recent terminal run and its recorded status, time and summary, including while a revision is running. In `latestDelivery` and the corresponding `session/messages` completion notice, `approvalDecisions` lists each recorded request ID, tool and outcome in decision order for that run. An empty list means no decisions were logged. An absent field means the run could not be identified. The worker’s reply remains available separately. An artifact’s optional `writtenText` is the complete text recorded at the write. A later partial edit clears it.

Tasks with an isolated Git worktree record their starting commit and snapshot changes against it at each delivery. An older delivery can attach two complete commit IDs from its repository through `task/reviewCapture`. Files outside the text and snapshot budgets may provide only a path and change type.

## turn/start notification sequence

```
turnStarted → retry* → textDelta* → toolStarted → toolDelta* → toolCompleted*
            → consolidationCompleted? → turnCompleted → (response)
```

`retry` can precede any model call (loop, summarizer), so it is
**insertable** rather than fixed in place. The full set of `ServerNotification` variants (discriminant `type`): `permissionChanged`, `turnStarted`, `textDelta`, `retry`, `approvalRequested`, `goalRound`, `toolStarted`, `toolDelta`, `toolCompleted`, `consolidationCompleted`, `turnCheckpointed`, `turnSteered`, `turnResumed`, `turnSuspended`, `taskUpdated`, `turnCompleted`, `error`.

* `retry`: one retry (attempt, reason, delay). Emitted **every time** — a retry
  is never silent; `meta.retries` carries the total.
* `approvalRequested`: a command is waiting for approval (tool, the command
  itself, why, deadline). Answer it with `approval/respond` or the turn stays
  blocked; **no answer is a refusal**.
* `textDelta`: streamed text increment; only with `stream:true`.
* `toolDelta`: a chunk of live tool output; progress, not a result.
* `cwd` (`TurnStartParams`, optional): the turn's working directory. Must be an absolute path that exists (fail-closed); `run_command` resolves its fallback chain as `args.cwd > cwd > process cwd`, and relative paths in `read_file`/`glob`/`grep resolve against it (session-level working-directory). Subagents inherit the parent turn's cwd.
* `toolStarted` / `toolCompleted`: tool begin and end (duration, status).
* `turnCompleted`: `reply`, `iterations`, `usage`, `meta` (`TurnMeta`: tools, model, latency, interrupted, retries, goal status, route reason and more).
  The meta is persisted with the conversation.

### `Fact.kind`

What a memory is about: `user` / `feedback` / `project` / `reference` /
`fact` (the fallback). It travels in `memory/search` and `memory/list`
responses and in the retrieval context; `memory/remember` may send it, and an
unknown value is normalised to `fact`.

## Error codes

| Code | Meaning |
|---|---|
| -32700 / -32600 / -32601 / -32602 / -32603 | standard JSON-RPC: parse / request / method not found / params / internal |
| `-32000` PROVIDER_ERROR | model-side failure (missing key, 401, rate limit…), with what to do |
| `-32001` TOOL_ERROR | tool execution failed (the error also returns to the model as text; the turn continues) |
| `-32002` NOT_IMPLEMENTED | defined in the protocol but not implemented; distinct from method-not-found |
| `-32003` SESSION_BUSY | one turn per session at a time; the message names the turn that holds the session |

## Clients

* TypeScript: `integrations/channels/runtime-client` (`StdioTransport` + `JeevesClient`;
  notifications dispatch on the `type` discriminant).
* The terminal REPL drives the same method surface in-process; the dashboard reaches it
  over HTTP (routes in `jeeves-ops/src/lib.rs`).

## Independent task execution

`task/create` only records a task. A human can call `task/start {taskId, message}` (or click **Start task** in the cockpit) to authorize its initial execution. The message contains the actual instructions; the cockpit uses the title and goal. One SQLite-backed worker consumes these requests. Creation alone never starts work, and there is no model tool for starting tasks.

`task/create` accepts `dependsOnTaskIds` with up to eight distinct, existing tasks in the same conversation. After `task/start`, the queue holds the new task until each predecessor has a completion receipt linked to passing acceptance checks. If a predecessor is already done, its current verified delivery qualifies. If a predecessor has been reopened, the queue requires a new verified delivery. A receipt that satisfies a dependency remains valid for that dependency. Task views expose outstanding IDs in `waitingForTaskIds`. The queue keeps waiting across restarts and respects human pauses and cancellations.

`AgentTaskView.executionSessionId` identifies its separate transcript; `sessionId` remains the originating conversation. `dispatchState` is pending / claimed / finished / interrupted, describing initial dispatch, not the task's final outcome. Paused and job-waiting tasks keep their normal task states. The original conversation can continue independently and receives bounded, owner-scoped task progress context. Working transcripts are not copied into chat. Output without acceptance is described as unverified in that progress snapshot.

Each task accepts one initial dispatch; a duplicate start is rejected. Pending dispatches survive restart. Claimed work is owned by checkpoint recovery; a claim with no checkpoint stops for review rather than being submitted again. A queued pause requires human resume; cancel prevents execution. `POST /api/task` accepts `action: "start"` with the same parameters. The TypeScript client exposes `TASK_START`.

This is a local worker, not an always-online service or a distributed lease system. It runs while app-server is alive and a model is configured. It does not automatically ingest external callbacks or grant capabilities. Unrelated manual turns retain their existing concurrency rules; concurrent changes to the same external resource still require coordination.

## Task event cursor

A client that was offline replays a task's timeline with `task/events { cursor?, taskId?, limit? }`.
Events come oldest first. `limit` is at most 500 (default 100). The response carries `nextCursor`, `hasMore`
and `latestCursor`; pass `nextCursor` back to continue. Cursor 0 (or none) starts from the beginning, and a
cursor past the end returns an empty page that keeps the cursor. Per-task order is each event's `seq`.

The `/api/events` knock stays a knock, but pings about a task now carry `cursor`, the newest event cursor.
A client compares it with its own and calls `task/events` only when it is behind.

The dashboard reads the same page over `GET /api/task/events?cursor=&limit=`. Each tab keeps its last cursor in memory, pulls from it on reconnect, on a knock and on the 5-second poll, and appends the result; only when the server's newest cursor is behind the tab's (the log was replaced) does it start over and reload the snapshot. `@jeeves/client` exposes the same logic as `TaskEventFollower`.

## Capability cards

`capability/list` returns a `CapabilityCard` for every configured model profile (`model:<id>`), MCP server
(`mcp:<name>`) and the Feishu connector (`connector:feishu`): `kind`, `title`, `identity?`, `toolSource`,
`state`, `detail?`, `secretPointer?`, `lastCheckedAt?`. `state` is `ready`, `unauthorized`, `expired`,
`missingScope`, `blockedByPolicy`, `offline` or `unknown`. A card never carries a key, only `secretPointer`
(`env:NAME`, `keychain`, `file`, `memory`, or unresolved `vault`; MCP OAuth tokens report `file`). The Feishu card reports `blockedByPolicy` when its recipient allowlist is empty; MCP reports `missingScope` only on an explicit insufficient-scope response during connection. Probing is on demand and each card is cached
in memory for 60 seconds; `capability/probe { id }` answers from that cache and rejects an unknown id.

The dashboard relays the same responses through authenticated `GET /api/capability` and `POST /api/capability/probe` with body `{id}`.

Authenticated `GET /api/connector/settings`, `POST /api/connector/settings/save` and `POST /api/connector/oauth/start` relay the human-only connector methods. OAuth initiation returns `authorizationUrl` only after binding the local callback; the user must open it on the Runtime host. The flow stores credentials in the existing credential store and never returns tokens. Only configured OAuth-enabled MCP servers and Feishu user identity support this flow.

To split a coding task, create a parent with `repo` and a final command check. While it is pending, call `task/create` for each child with `parentTaskId` and a title. Each child branches from the parent’s committed HEAD and has its own task ledger. Start the children through `task/start`; independent worktrees can write concurrently. A permitted coding CLI runs through the existing `run_command` background option. Its log and a bounded snapshot of tracked changes and untracked filenames appear in task artifacts. A failed or interrupted executor requires review.

After child completion, checkpoint the selected files and confirm each exact child SHA with `POST /api/task/integrate` or `task/integrate {parentTaskId,childTaskId,commit,confirmed:true}`. Conflicts stay with the human: merge the reviewed commit in the parent worktree, then confirm the same SHA again. Once all children are integrated, start the parent to verify the combined result against its own acceptance checks.

`POST /api/task/git-checkpoint` relays `task/gitCheckpoint {taskId, paths, confirmed}`. It requires `confirmed: true`, explicit relative file paths and the expected inactive linked task worktree and branch. Paths are literal, including non-ASCII names and deletions of selected tracked files. It records the commit SHA as a `git_checkpoint` event for handoff; it refuses existing staged changes and never pushes, stages unselected files or rewrites history. Automatic Git checkpoints are off by default. `GET /api/task/git-checkpoint-policy?taskId=…` reads the policy; `POST /api/task/git-checkpoint-policy {taskId,paths,enabled,confirmed}` opts in or disables while stopped. Once opted in, every durable execution boundary maps to a commit for only the chosen paths (unchanged boundaries reuse HEAD); checkpoint failure stops execution for review. Never select files containing secrets. Git history is local to the task branch.

## Durable external input

Two reserved source/key pairs opt an existing task into read-only polling: `connector:email` / `new-unread` and `connector:calendar` / `upcoming-15m`. Only a human-declared, suspended input wait is polled (once a minute, two concurrent polls at most). Mail captures UIDVALIDITY and UIDNEXT at the first successful poll and persists that baseline across restarts; only unread UIDs at or beyond the baseline can release it. Calendar selects a timed occurrence starting within 15 minutes, excluding all-day events. Changed account identity or mailbox generation retains the wait for review. Signals contain only item identity/timing metadata, never a message body; no new task is created.

Before starting a task, a human or authorized host may declare `task/wait {taskId, source, eventKey, description, expiresAt?}`. The condition is immutable and limited to one per task. `expiresAt` is an RFC 3339 input-arrival deadline, not a deadline for processing an already accepted event. An incomplete task can save its draft and suspend without consuming more model rounds while waiting. Companion chat remains independent.

`task/signal {conditionId, source, eventKey, eventId, data}` supplies a JSON value up to 16 KiB. Source and event key must match. Repeating the same source/event ID and data acknowledges the original event; changing its target or data is rejected. These fields correlate data; they do not authenticate a sender. Use the existing authenticated host transport, never expose this method as a public webhook or a model tool.

Feishu commits each inbound event fragment to its private, per-account SQLite inbox before acknowledging it. A write failure withholds the ACK. On restart, complete events resume under their original message IDs. Completed events retain a deduplication key and release their payload.

`task/wake {sourceKey, source, eventKey, data}` is the channel-facing form. `sourceKey` is `<channel>:<messageId>`. If a task is waiting for exactly this `source` and `eventKey` (for Feishu: `lark` and the chat id), the message is delivered to it through the same path as `task/signal` and the response has `woken: true`. A repeated `sourceKey` returns `duplicate: true`, delivers nothing and logs `duplicate wake skipped`. If no task waits, `woken` and `duplicate` are both false and the caller treats the message as an ordinary one. The Feishu gateway offers every accepted text message to `task/wake` first and falls back to a normal turn if the call fails.

A committed event wakes the exact waiting run once, including after restart. Its checkpoint retains the draft and full conversation; the input is explicitly untrusted data and grants no permissions. Input receipt is not task completion: the resumed execution must pass its original budget and acceptance checks. Human pause/cancel wins. Input received during a human pause remains stored until explicit `task/resume`; continuation waits for the previous actor to finish before committing its authorization and reopened checkpoint atomically.

## Operation journal and human reconciliation

`task/operations {taskId}` returns persisted operation identities, argument/output digests, intent or observed result, human evidence and the current checkpoint digest. Local operation keys are not provider idempotency keys. The automatic replay policy is `never`; an intent means execution may or may not have happened.

A human may stop the task, inspect the real external state, then call `task/reconcile {taskId, operationId, checkpointDigest, resolution, evidence}`. Resolution is `reconciled_success`, `reconciled_failure` or `not_executed`; evidence is 1–16384 bytes. The latest suspended execution and exact unresolved call/arguments must match. The evidence and a labelled tool-result receipt commit atomically with the checkpoint. Identical evidence can be acknowledged again; conflicting settlements are refused.

Reconciliation never executes a tool, clears a human pause or resumes a task. Every unresolved call must be reconciled before explicit `task/resume`. Human confirmation is not an observed successful tool return and does not satisfy `toolsOk`; original artifact and budget checks still apply. Recent operation evidence is reconstructed into durable handoffs independently of rolling summaries. These methods are authenticated human/host controls, never model tools.

`ToolOperationView.status` distinguishes dispatch intent, executor outcomes, human reconciliation, unexecuted calls and historical uncertainty. An old success or failure label without an executor preview returns `historical_unverified`. The original digest remains inspectable. Acceptance and recovery require independent evidence. A human reconciliation accepts `reconciled_success`, `reconciled_failure` or `not_executed`.

`ToolOperationView.origin` distinguishes an intent committed with its execution checkpoint (`checkpoint`) from pending metadata reconstructed from an unchanged stopped legacy checkpoint (`recovered_checkpoint`). Neither proves transmission. Human pause atomically fences the task, cancels durable waits and suspends its recoverable boundary; a failed commit preserves the original wait.

## Versioned verification evidence

`CheckView.artifactSha256`, when present, identifies the exact bytes read for a file measurement. File existence checks stream a regular file; substring checks use the same stable text read for matching and hashing. A changing or unreadable file fails its check. Historical fingerprints do not assert that a path still contains those bytes.

Nonempty acceptance results commit with an immutable task/run/round verification receipt, its file manifest and the current checks projection. A failed evidence commit stops completion for human review. Changed content creates another linked receipt, including an A→B→A reversion; immediate identical observations reuse the latest receipt. Receipt integrity covers its measurements, observation time and predecessor ID. A successful task completion receipt binds the corresponding exact-run verification ID. No archival file copy is created.

A model-only goal judgement stops the goal loop but does not establish independent acceptance. With no acceptance conditions, completion receipts and conversation progress use `produced_unverified`, and the cockpit labels the result accordingly even when a goal condition exists.

## Memory validity and derivation context

Human clients can replace a fact's whole validity window with `memory/setValidity`
(`MemoryValidityParams` → `MemoryValidityResponse`). Supply `factId`, `validFrom` and
`validUntil`; timestamps use RFC3339, null removes a bound, and an end must follow its
start. The start is inclusive and the end exclusive. These controls are not model tools.
`memory/provenance` (`MemoryProvenanceParams` → `MemoryProvenanceResponse`) reads the
bounds and recorded extraction contexts, including inactive facts when their ID is known.

The database records the claimed chat row IDs and SHA-256 digests of their role/content
as derivation context. This identifies the model's input batch, not which sentence proves
a fact. Replaying that exact context does not increase its confirmation count. Corrections
clear the previous claim's contexts; deletion cascades them. Legacy facts may have none.

Recall filters inactive facts from keyword/vector/recent recall. Bounded facts
stay out of generated static mirrors even while active. Prompt catalogs and
`read_page` check database-backed page sources at read time; missing, corrected or
bounded sources exclude the whole snapshot. Human pages without fact references remain
independent. Existing files, chat history, episodes, checkpoints and backups are not erased
or retroactively redacted. The temporal filter covers facts, not episodic summaries.

Background consolidation also records source-bound temporary observations.
The extraction contract separates durable `facts` from optional `states`. Each state carries
an exact `sourceRef`, a 4–400 character user quotation, model-reported confidence in [0,1]
and a lifetime of 1–24 hours. The reference must belong to the claimed input batch and match
the current stored user row's SHA-256; assistant text and mismatched quotations are refused.
Expiry is anchored to the source message, not extraction/retry time. Already expired sources
produce no temporary fact. Fact, validity and assessment commit atomically.

`memory/provenance` additionally returns optional `validityOrigin` (`human` or `consolidation`)
and each observation's optional `assessmentJson`, including quotation, source reference,
confidence and source-time expiry. Confidence is uncalibrated model judgment; matching a quote
does not verify the inferred meaning or a person's current mood. Replaying an observation does
not renew it. Human content/validity edits and deletions take priority; legacy unbounded claims
are not automatically reclassified. These metadata fields add no model control method.

Recall labels bounded claims as temporary and episodes as historical summaries. Fact validity
does not redact raw history or expire episodes. Model misclassification, semantic paraphrases,
explicit note tools and historical unbounded moods remain outside this automatic TTL guarantee.
Malformed extraction shapes retain source chats with retry backoff instead of silently marking
them consumed. Extraction instructions and JSON exchange data use separate message roles.

Consolidation distinguishes admitted extraction batches from completed rows. Exclusive
runtime startup releases only recorded unfinished claims. A retry retains its original source
set, even when new chats arrive or the new-work threshold changes; fresh admissions contain
at most 4,096 rows. Backoff updates the whole batch transactionally. A committed source-context
receipt prevents a repeated batch from inserting or replacing its episode summary, and survives
human episode deletion as a tombstone. Completed rows and legacy rows without admission metadata
are not reopened.

The memory worker retains database execution ownership while alive. Graceful shutdown waits
briefly, then cancels and joins a stalled extraction; dropping the worker aborts its loop.
A recovered model extraction may issue another read-only model request and incur usage; it
never replays task tools. Unreceived provider usage remains outside exact task accounting.

## Structured document acceptance

`jsonEquals` checks a parsed JSON value against the human's fixed contract, rather than
searching text for a number. Example acceptance criterion:

```json
{"kind":"jsonEquals","path":"report.json","pointer":"/totals/approved","expectedJson":"42"}
```

`pointer` uses RFC6901 (`~1` for slash, `~0` for tilde); an empty pointer selects the whole
document. `expectedJson` is an encoded JSON value, limited to 16,384 UTF-8 bytes, so `"null"`
is distinguishable from an omitted expectation. Missing fields do not equal null, and strings
do not equal numbers. Equality follows parsed JSON values, not financial decimal arithmetic.
Documents must be regular UTF-8 JSON files within the task workspace and at most 16 MiB.
Invalid pointers, expectations, documents, paths and unavailable measurements fail closed.

## Receipts and command acceptance

A tool saying "sent" and a read-back seeing the message are different facts. `receipts` (migration 0004)
holds one row per effect with a status: `claimed` (the tool reported success), `confirmed` (a read-back saw
it) or `failed`. Two acceptance kinds use them:

```json
{"kind":"receiptConfirmed","receiptKind":"feishu_send","refPrefix":"om_"}
{"kind":"commandSucceeds","argv":["cargo","test"],"cwd":"."}
{"kind":"sourceSupports","path":"claims.json"}
```

`sourceSupports` reads `path`, an array of `{claimId, text, sourceRefs, quote}`. Inside a task, `fetch_url` saves what it returned under `sources/` and indexes it in `sources/index.jsonl` (`url`, `fetchedAt`, `sha256`, `path`). Search leads are archived separately in `sources/search/index.jsonl` with the query; they cannot satisfy this check. The check passes only when every claim cites a saved snapshot, its `quote` appears verbatim in a cited snapshot, and every number in `text` appears exactly in the `quote`. A model-written summary is never evidence.

`receiptConfirmed` passes only when a `confirmed` receipt of that kind with a ref starting with `refPrefix`
exists. A receipt that is only `claimed` fails and the detail says so. `commandSucceeds` runs `argv` (an
argument list, quoted, never a shell string) in the task's directory or a subdirectory of it, through the same
session permissions, command allow rules, sandbox and network settings as `run_command`. If the active policy requires approval, the user sees the exact command. Exit 0 passes and any other exit code fails with the tail of the output. If exec is off, the
sandbox is missing, the command is refused or its outcome is unknown, the check fails with a detail starting
`skipped:` (for example `skipped: exec off`). Skipped is never a pass. Output is kept in `checks/` inside the
task directory. A missing field in either condition is an error naming the condition's number.

The check and SHA-256 measurement use the same captured bytes. Existing verification receipts
bind that version to the task/run/contract; contradictory versions across checks cannot be
committed as successful evidence. This proves conformance to supplied values, not the truth
of an external source or a successful remote transaction.

## Ordinary-chat operation review

Use **Review chat operations** in the conversation header, or the human-only
`turn/operations` / `turn/reconcile` methods. Inspection is scoped to `sessionId`;
optional `turnId` selects a stopped ordinary-chat checkpoint. Task-owned operations continue
to use `task/operations` and `task/reconcile`. Each operation carries its own checkpoint digest
when an inspection spans multiple turns; clients must submit the selected operation's digest.

A new loop in the same conversation cannot bypass unresolved prior effects: it keeps only
known read tools and carries a bounded host bookkeeping note. This includes legacy checkpoints
without journal metadata and unclaimed open boundaries during startup. Human inspection stops
an unclaimed uncertain boundary before reconstructing labelled metadata; it never proves that
a request was transmitted. Corrupt old messages keep mutation access closed.

Evidence is immutable, identity-scoped and committed atomically with the exact checkpoint.
Same evidence/outcome can be acknowledged again; a stale digest or conflicting outcome is
rejected. The old turn stays stopped. After review, send a new instruction using the current
workspace; this route does not recover old permissions or workspace state. Human observations
do not count as observed tool successes. Separate conversations and uncooperative external
processes still have no global external-effect deduplication guarantee.

### Explicit continuation after a stop

`turn/interrupt` stops execution and does not automatically resume its checkpoint. A human may explicitly call `task/resume` for a known-outcome stopped checkpoint belonging to the latest task execution; authorization, activation and reopening commit atomically. Cancelled tasks, stale execution identities and unreconciled unknown tool outcomes remain blocked.

## Session permissions

`readOnly` admits reviewed built-in reads and tools declared as reads in the owner's [connector manifest](configuration.md#connector-manifests). `standard` requires a real turn approval for each effect invocation, approved once. Both disable delegation and session lifecycle/tool hooks so child execution cannot bypass approvals. `fullAccess` permits automatic execution within enabled capabilities while preserving hard denials, allowed roots and sandbox rules. It never enables `JEEVES_EXEC`, `JEEVES_WRITE` or other capability switches.

`session/permission/set` persists before acknowledgement and emits `permissionChanged`. Stale revisions, missing sessions and unconfirmed elevation are rejected; unreadable permission storage blocks tools. Recheck the revision after approval/resource waits and before invocation. Changes invalidate pending approvals; already-started effects are not rolled back. Only trusted hosts call this API; no model tool exposes it. The main session starts in `standard` mode; a session without a preset keeps legacy runtime rules (`mode:null`).

Presets constrain model tools and session hooks. Conversation ledgers, usage records and existing automatic memory consolidation remain internal Runtime persistence; read-only does not mean opening the data directory without writes. The protocol entry is stdio; the ops dashboard relays the same policy/config/model routes over authenticated HTTP (`/api/session/permission`, `/api/config`, `/api/model/catalog`).

## Capability negotiation

New shells call `session/capabilities` once at connection time. `protocolVersion` is bumped only on a breaking wire change; unsupported feature names come back explicitly rather than being ignored, so an older client talking to a newer engine never guesses. Platform and transport are diagnostic labels, never permission boundaries.

## Main human interaction


`session/main` (null params) idempotently opens `jeeves-main`, returning `SessionNewResponse`; only first creation initializes standard permissions. `interaction/start` accepts generated `InteractionStartParams`: `sessionId`, stable `messageId`, human `message`, optional `taskId`, `cwd`, and image `attachments`. It returns `TurnStartResponse` with normal notifications. Web uses authenticated `GET /api/session/main` and `POST /api/interaction` (SSE). The dashboard and gateway HTTP client read multi-line SSE events across network chunks and accept LF, CRLF, and CR line endings.

Clients can call `session/main` to recover the main conversation ID when a session lookup fails.

The main model answers discussions and asks for clarification in the conversation. Its interaction-scoped `task` tool can create a batch of up to four independent assignments or continue an owned task. Both operations require literal spans of the current human message. A span used for a follow-up cannot also create another task in the same interaction. New tasks carry document references and bounded main-conversation context. An explicit `taskId` selects an owned task. Pending follow-ups append; running ones steer at the next model boundary. Completed unchecked single-turn tasks reopen in the same workspace/context with a new run; other states require existing resume/reconciliation. New images for pending/running tasks are refused.

A task counts cumulative token, tool and elapsed-time usage at execution boundaries. Recovery leaves a legacy checkpoint suspended for review when its task owns the turn but its run identity is missing. Ordinary chat checkpoints with recorded tool results can still resume.

Task list, runs, artifacts and detail reads return an RPC error if their underlying query fails. A task with unreadable stored checks, acceptance or budget also reports an error. An absent task still has an empty run and artifact history.

For an owner-chat mail amendment, the task binding limits the candidate actions. A backward reference can use the immediately preceding completed private interaction’s task binding, subject or action ID. Multiple candidates require a choice.

A durable `(sessionId,messageId)` receipt replays identical completed retries; changed, failed or unfinished submissions refuse redispatch. Partial create/start failures are reported individually. Ambiguous transport failure requires inspection before sending new work. `task/create` without `cwd` allocates a distinct managed root; explicit project bindings remain supported. With `repo` (an absolute path to a git repository, chosen by the human) the task instead gets its own worktree `<home>/worktrees/<task-id>` on branch `jeeves/<task-id>`, which is its only working directory; `task/view` and `task/get` report `worktreePath` and `branch`. `task/create` also takes `codeChecks: [{name, argv, after}]` for a coding task: `after: "final"` becomes a `commandSucceeds` acceptance condition, and `after: "edit"` runs in the task directory after every `edit` / `apply_patch` (through the exec gate and sandbox, 60 s each), with up to 4 KB of its output appended to that tool's result; with exec off the result says `[check <name>] not run: exec off`. The worktree is created at `task/create`, reused if it exists, never deleted automatically; a `repo` that is not a git repository, or `repo` together with `cwd`, is refused with a text error. `task/start` accepts optional images.

`GET /api/notifications` carries authenticated `ServerNotification` independently of a submitted turn for worker progress/approval. `POST /api/task` with `{action:"artifacts",taskId}` adapts `task/artifacts`. `session/list` returns only `jeeves-main`. Task inspection reads internal execution history directly. The task tool lists the conversation's 20 newest tasks by creation time. Channel handoffs see only tasks from their verified chat and sender. The private main conversation handles immediate work with the enabled tools and session approval policy. An authorized interaction also exposes its own task handoff from the first request in eager and deferred mode. An earlier unresolved chat effect limits the table to unconditional reads until human review. In task sessions, read-only mode rejects memory mutations, standard mode requires approval, and full access follows the enabled tools. External memory MCP clients use the separately enabled memory service. `turn/start` routes to the same main interaction. Live pending questions can be recovered while the same Runtime is alive; missing/expired approval denies effects. Details: [internals/06-app-server.md](internals/06-app-server.md).

### Live pending approvals

`approval/pending {sessionId}` returns `ApprovalPendingResponse.approvals`: the live request, execution session, owning conversation and optional current task view. Web adapts it at authenticated `GET /api/approval/pending?sessionId=...`; Web and Mac repair the task panel from this snapshot on opening/reconnecting. A pending task outside the recent-list window is included with its approval. The approval audit log is never treated as a pending queue.

### Pending approvals across a restart

A question waiting for a human is also stored in `state.db` (migration 0003, `pending_approvals`). When the
process restarts, questions that have not expired come back from `approval/pending` with `restored: true`
and `request.version` raised by one; expired ones are dropped. `ApprovalRequestedNotification.version`
starts at 1. A client echoes it as `version` in `approval/respond`. A stale version gets a text error
(`Error: this approval is now at version N, not M ...`) and leaves the question in place.
The first answer wins. A later answer gets `accepted: false`, and `answeredBy` (the first answerer's `actor`, such as `telegram:7` or `lark:ou_…`) when someone answered, so every channel can say "already answered by …". Gateways pass `<channel>:<id>` as the `actor`.

A restored question has no live tool call waiting on it. A yes records a single-use grant for the same tool
and the same action preview, which the resumed turn spends when it asks again; a no records nothing, so the
resumed turn asks again as usual. Grants themselves (`action_grants`) match only when account, resource,
action and destination are identical and, if the grant carries a content hash, the content is too. An
expired or revoked grant never matches.

`request.expiresInMs` is time remaining at snapshot collection, not a renewed deadline. The deadline starts when the question is registered and answering checks it even if the waiting future has not run yet. Closed receivers are not live. Request ids include process/time identity and a counter; clients must echo the opaque id, never generate it or reuse an old process's id. The first delivered answer consumes the request; expired, cancelled, revoked and duplicate answers return `accepted:false`. Owner-policy changes invalidate child-worker questions. Client event/action versions prevent an old in-flight snapshot from restoring a question that has just been answered. A Runtime restart restores no approval grants or pending questions; interrupted effects still follow checkpoint/operation reconciliation.

### Device pairing and revocation

On the operator's cockpit connection, compare the device's Ed25519 public key with the value shown on that device over a trusted channel. Send the standard padded base64 of the 32 public-key bytes as `deviceId` to `device/pair`, together with `expectedGeneration` (0 on first pairing) and at least one capability from `browserControl`, `desktopControl` and `approvalReview`. Keep the private key on the device. The operator can inspect grants with `device/list` and revoke one with `device/revoke` using its ID and current generation. HTTP maps these to `POST /api/device/pair`, `GET /api/device` and `POST /api/device/revoke` under cockpit authentication. Use a trusted encrypted connection when administering a remote cockpit.

Give the device its returned `grantId` and `generation`. Its first operation is a capability presence report to `POST /api/device/report` (RPC `device/report`). It signs the UTF-8 compact JSON array `["jeeves.device.report.v1",deviceId,grantId,generation,capability,sequence]` with Ed25519. Send those five fields plus the standard padded base64 `signature`. Use positive, strictly increasing sequence numbers within each grant. A successful report records and returns the sequence. Each pairing issues a fresh grant ID and resets the sequence. Revocation and re-pairing advance the generation. The active grant and its sequence survive runtime restarts.

The device endpoint accepts presence reports only. Control input, desktop actions and approval decisions require their own operation-specific authorization.

### Task approval notifications

On iPhone, open the cockpit as a Home Screen web app on a trusted HTTPS origin. On Mac, use Safari. Set the [Web Push contact](configuration.md) on the Runtime before enrollment. In **Settings → Approval notifications**, display the device public key. Compare it on the trusted operator cockpit and grant `approvalReview`. Back on the receiving device, check pairing, then press the enable button to request system permission. The device keeps its non-exportable Ed25519 signing key in IndexedDB. The Runtime creates its VAPID signing key on explicit setup and stores it and subscription credentials in its secret store. The subscription accepts Apple HTTPS push-service endpoints. [Apple platform requirements](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/)

A lock-screen notification opens the requested task’s approval. Open it on the enrolled device and read the action, target and scope before allowing once or denying. Offline, expired, answered or revoked requests cannot be submitted. A notification in flight during revocation may still appear. The cockpit rechecks the grant generation and live request when opened and again at submission. A lost delivery response is recorded as an attempt and is not automatically retried. Pending approvals remain accessible from the cockpit.

Run `just smoke-approval-notice` for offline cold launch, review and enrollment checks with a disposable browser, a fake subscription and a loopback cockpit. Lock-screen delivery requires a separate physical iPhone check.

All four endpoints require cockpit authentication. `POST /api/approval/push-key` returns the VAPID public key, or null when disabled. `POST /api/approval/push-subscribe` verifies an Ed25519 signature over the compact JSON array `["jeeves.approval.push.v1",deviceId,grantId,generation,sequence,endpoint,p256dh,auth]` and uses the grant’s increasing sequence. The encrypted notice contains only the task, turn, request, version and recipient grant identifiers. `GET /api/approval/notice` reads the live request and `POST /api/approval/notice` submits a once-only decision. Both verify the signature over `["jeeves.approval.notice.v1",deviceId,grantId,generation,taskId,turnId,approvalRequestId,approvalVersion,approved]` with `approved` set to null for reads. Signatures use standard padded base64. RPCs are `approval/pushKey`, `approval/pushSubscribe`, `approval/notice` and `approval/noticeDecision`.

### Global emergency stop

Use **Emergency stop** in the cockpit header to stop all task browsers managed by the current Runtime. It disconnects workers, prevents new launches, and holds affected tasks across restarts. A page may already have received an interrupted input. Review unknown effects in the task’s operations ledger before continuing.

Only a cockpit operator can release the stop. Release permits new launches. Open a fresh browser for each affected task so the engine can collect a new observation. The task browser’s **Pause** control still applies to that task alone.

Operator RPCs are `executor/control`, `executor/stop` and `executor/release`. The HTTP equivalents are `GET /api/executor/control`, `POST /api/executor/stop` with `{}`, and `POST /api/executor/release` with `{"expectedGeneration":N}`. Use the generation returned by the latest status read. Repeating stop while frozen returns the same generation. Every successful release advances it. Device presence reports and unrelated conversations remain available. Stop also revokes the selected native window. Select a new window after release.

### macOS single-window operation

Open the desktop window panel on a task card. Select the application and confirm the window title in the native Mac dialog. Only that task can use the selected window. The helper requires macOS 14 or later. Selection requires Accessibility permission. Screenshot preview additionally requires Screen Recording permission. The operator grants these in System Settings → Privacy & Security. For development, run `just desktop-window-helper`, set `JEEVES_DESKTOP_HELPER` to the absolute path printed by the build and restart the Runtime.

Observe lists available controls. You can press buttons, step vertical scroll bars or set plain text in an ordinary single-line field. Switching windows in the selected application, closing the window or taking over requires a fresh selection. Pausing a task revokes that task’s window. Each input consumes an observation. The panel shows the last 20 action receipts. An interrupted action with an unconfirmed effect remains unknown. Inspect the target before deciding how to continue.

**Capture selected window** previews a JPEG for local saving without selecting a model (`desktop/frame` with `localOnly: true`). If Stage Manager has moved the window off screen, the helper can bring its application forward. Capture requires a unique on-screen match for the confirmed window. To ask a vision model for advice, select a screen profile, preview the image and confirm the displayed destination. The model returns short advice with a risk label and element references validated against the current observation; advice is never executed automatically. Receipts omit pixels and typed values. Observing again, changing the screen profile, takeover and emergency stop invalidate the preview.

With Accessibility and Screen Recording already granted, run `just smoke-mac-window` for the native regression. It launches two disposable AppKit windows, checks the scroll position, verifies each preview by its color marker, and checks that switching windows revokes access and identical titles and bounds block capture.

For manual acceptance, run `just desktop-window-fixture` to open two disposable windows and start a separate loopback cockpit with a fixture task and local image receiver. Open the printed cockpit address in your browser and choose **Main conversation → Task progress** in the sidebar. Leave the fixture task pending. On the **Mac 单窗口验收 / Window acceptance** card, expand **Task window**, click **Choose a window on Mac** and select one JeevesWindowFixture window. Toggle the checkbox, change the ordinary field, scroll the list and check that the secure field offers no input action. Switch to the other fixture window and verify the next operation requires reselection. Select again, verify the preview contains only the chosen window and try takeover and emergency stop. Closing the fixture process discards all inputs.

The cockpit uses `GET /api/desktop/status?taskId=…` and `POST /api/desktop/{select,observe,act,takeover,model,frame,share}`. The corresponding operator RPCs use the `desktop/` prefix. The window selection and image consent last only in the current Runtime process.

Browsers and native windows share a control lease. The Runtime records the lease id, epoch, owner and reason in `control_leases`; every observation, action, preview and receipt echoes the current lease. A takeover revokes it, and a stale lease is rejected before any input reaches the target. Emergency stop revokes both browser and desktop leases. Releasing the stop never revives a lease: open a fresh browser or select a new window.

### Remote browser input and downloads

Open the task browser in your authenticated cockpit, choose **Take over**, then open the remote controls. The desktop and phone layouts operate the same task-scoped Chromium context. Click within the displayed image, enter text, scroll, or use Tab, Enter, Escape and Backspace. Input consumes one human frame and returns a new one. Refresh after the display changes or after 30 seconds. Returning control takes a fresh observation. Use a trusted encrypted connection for a remote cockpit. The paired-device presence endpoint does not grant cockpit access.

To attach an image, open the website’s file chooser in the remote controls, choose a PNG or JPEG of up to 1 MiB and 4096 pixels per side, then press **Upload image** to send it to the displayed origin. A chooser accepts one image and is bound to the current browser frame and control generation. The task agent cannot choose or upload local files.

Downloads started on allowed websites while the operator has control appear in the task browser. Choose **Save** to publish a completed file to the task’s artifacts or **Cancel** to discard it. Each browser context handles up to 16 downloads of at most 768 KiB each. Its temporary list ends with the browser. Saved artifacts survive restart under `<home>/artifacts/<taskId>/`. Each name includes a unique download ID, so two reports with the same name remain separate. If recording an accepted file is interrupted, the engine retries from its local copy at startup. It preserves an existing edited file for review.

The authenticated HTTP calls are `POST /api/browser/input`, `GET /api/browser/downloads?taskId=…` and `POST /api/browser/download` (RPCs `browser/input`, `browser/downloads` and `browser/download`). Input carries the current generation and frame ID. A file decision carries the task, generation, download ID and action. Refresh older optional browser components with `just browser-runtime` to enable the new operations.

### Task browser controls

Human-only `browser/list` accepts `BrowserListParams` and returns installation availability plus owned browser metadata. `browser/start` accepts `BrowserStartParams`; `browser/control` accepts `BrowserControlParams` with the expected generation; both return `BrowserSessionView`. `browser/frame` accepts `TaskGetParams` and returns a bounded, on-demand `BrowserFrameResponse`. They are authenticated client RPCs, never model tools. Web maps them to `GET /api/browser`, `POST /api/browser/start`, `POST /api/browser/control`, `GET /api/browser/frame`. Worker transport types share the Rust protocol definition, but worker commands are not public app-server RPCs.

## Recurring responsibilities

In the owner main chat, the model proposes changes with `recurring_work`. The cockpit reads `GET /api/responsibilities/reviews` and sends the selected card ID and boolean `accept` to `POST /api/responsibilities/decide`. Confirmation checks the revision and consumes the card in the same transaction.

The authenticated cockpit can post a proposed contract to `/api/responsibilities/prepare` to review the normalized fields. Preparation only checks the definition. Registration revalidates the contract, checks its ID is available and activates it.

The human cockpit can register a duty with `POST /api/responsibilities`, inspect it with `GET /api/responsibilities`, and send `{"id":"weekly","paused":true}` to `POST /api/responsibilities/pause`. These endpoints use the cockpit’s normal access controls. Models and chat gateways have no registration or pause method.

Registration requires a unique ID, a purpose, a future RFC 3339 expiry and at least one positive budget limit (`tokens`, `tools` or `wallMs`). A composable definition supplies `plan.trigger`, `plan.action` and `plan.delivery`. Read the versioned action schema from the `recurring_work` catalog. The Runtime returns a review `presentation` and validates the definition at confirmation and execution. The legacy digest and report fields remain accepted at the compatibility boundary. Sources identify evidence. Connector operations need independent authorization. An imported duty that needs revision returns `migrationNote`. [Execution and compatibility](design/main-entry.md#6-recurring-work)

### External delivery

Use `{ "kind": "connector", "connector": "feishu", "recipient": "oc_…" }` for an enabled destination. Retrieve the body, approval and outcome from `/api/responsibilities/reports?id=<duty-id>`. An approval binds the account, recipient and content. The engine rechecks the revision and scope immediately before sending. The Feishu adapter confirms delivery by reading back the same message ID. Interrupted sends and unknown outcomes go to human review.
