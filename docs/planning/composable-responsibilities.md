# Implementing recurring work from chat

English | [简体中文](composable-responsibilities.zh.md)

Open implementation plan. Follow [Chat, work and delivery](../design/main-entry.md) and the [Runtime architecture](../architecture.md). The unit of ownership is one responsibility in one user's Runtime. An occurrence can finish without opening an agent thread.

## Product and execution boundary

Keep the user's words, stable duty ID and confirmed revision. Rust owns the schedule and time zone, source identities, evidence fields, conditions, output, recipient, approval, expiry, stop rule and per-run budget. Keep writing instructions as text. Show one card in chat and My Work derived from the validated fields. Show defaults such as timezone and cadence before activation. Ask only about ambiguity that changes behavior. A model may propose new work or an edit. A human activates the reviewed version. Previewing evidence uses permitted reads without advancing the live cursor, sending, or consuming a first match.

| A user can combine enabled capabilities | Engineering supplies each capability once |
| --- | --- |
| Day, cadence, date window, source subset, length, wording and an existing delivery destination | Rust calendar windows, read-only connector bindings, evidence identity and deterministic filters |
| Report each week, stop at expiry, or notify once on the first observed price below a limit | Rust typed comparison with currency, stable product identity, shipping and a persisted first-match state |
| Ask for a local draft or review the exact text before posting to a verified group | Platform adapter, sandbox where execution is required, account/recipient resolution, last-moment admission and receipt verification |

An unobservable fact stays unknown. A new booking, purchase or write to another system requires a supported action and human enablement. Private sources remain scoped to the originating user and permitted audience. Local memory failure lets the turn proceed. A missing required business source yields an incomplete result or a visible skip. The gateway routes to the owning Runtime. Device-only work waits for that device under its agreed catch-up policy.

## Stage 1 — Review a contract before activation

Start with the existing human registration path. In `protocol/src/protocol/v2.rs` and `protocol/src/export.rs` declare a read-only contract preparation method. In `jeeves-app-server/src/responsibility.rs` share normalization and validation with registration. Wire dispatch and the authenticated cockpit handler in `jeeves-ops/src/api/responsibilities.rs`. Return the normalized contract. Registration still checks expiry at activation, unique identity and all registration rules. Repeated reviews create zero duties, wakes or approvals. Verify invalid input and round-trip equality through the HTTP entry in `scripts/smoke-web.py`. This first slice reviews fields. The sourced sample output arrives in Stage 3.

Extend the reviewed contract with typed source selectors, window, predicate, output, destination and stop rule. Put editable prose in a separate field. Add an incremental migration in `jeeves-state/migrations` for immutable revisions and their binding to each occurrence. Exercise the previous schema on an isolated copy. An edit uses its expected revision. Two competing edits produce one winner and one visible conflict. Keep the occurrence key stable across renames and revisions. Regenerate both schema formats with `just write-app-server-schema` after wire changes.

## Stage 2 — Execute one occurrence

Extend `jeeves-state/src/responsibilities.rs`, `weekly_reports.rs`, the existing wake/completion stores, and `jeeves-app-server/src/responsibility.rs`, `scheduler/` and `wake.rs`. Atomically claim the intended schedule slot or a stable source event ID and bind the approved definition revision. Preserve configured time zone, daylight-saving and missed-run semantics per duty. Event sources without a replay-stable ID can only offer scheduled observation. Use existing scheduler ownership and local storage per tenant.

Separate observations, produced drafts, approvals and confirmed delivery. Check source identity and budget before producing output. Persist send intent and recheck live revision, permission, deadline, draft digest, connector identity and recipient just before dispatch. On success commit local terminal state, evidence/receipt, any consumed first match and visible activity together. A pending external send after a crash enters the existing reconciliation path for human review. Pausing prevents new admissions and unsent old approvals. Display in-flight uncertainty separately. Recovery may clear only its own hold. Test duplicate triggers, offline catch-up, pause/resume, revoked grants, restart during send and rollback on terminal-write failure. Deterministic reads may run without a model.

## Stage 3 — Chat creation and Web review

Extend `jeeves-app-server/src/interaction/{mod,decision}.rs` with a structured proposal from the user's actual message and enabled capabilities. Bind human confirmation to the displayed revision. Use Runtime RPCs for prepare, sample, activate, amend, pause, resume and history. Keep activation and external sending outside model-controlled tool calls. Use `web/src/CoworkConversation.vue`, `web/src/ui/TaskCenter.vue`, generated types and the existing approval card. Localize the controls in both languages. Show source failures, no-match checks, completion receipts and interrupted sends in history. Opening the card cannot trigger work. Test ambiguous edits, a stale tab, unavailable sandbox and preview followed by activation.

## Stage 4 — Transfer the three existing duties

Run read-only comparisons on saved evidence in isolated homes. For each duty persist the old identity, last admitted occurrence and new execution owner in one cutover transaction. Both entry paths must honor that ownership before any claim. Keep the old owner for already admitted work until it settles or is held for reconciliation. A reversal before the next admission may restore the owner. After admission, retain that record and reconcile outstanding effects before switching again. Verify backup and recovery on copied databases before touching any live home. Remove superseded entry paths after their outstanding work clears.

| Cutover order and modules | Mapping and evidence required before switching |
| --- | --- |
| 1. Weekly report. `responsibility.rs`, `weekly_report.rs`, `jeeves-state/src/weekly_reports.rs`, `jeeves-agent/src/verify_sources/weekly.rs` | Map `weekly_report_json` to the research workspace, allowed saved source URLs, verified digest and Feishu recipient. Carry duty ID, last slot, wake keys, saved bodies, pending approvals and receipts. Keep local draft mode and the current interruption-to-review behavior. Compare sourced bodies. Pause or revoke while awaiting approval, crash after send, then prove no second POST in the loopback Feishu smoke. |
| 2. Morning briefing. `scheduler/morning.rs`, `scheduler/fire.rs` and schedule completions | Materialize only an enabled briefing and preserve its zone, 08:00 local date slot, catch-up and outbox/notify setting. Keep zero-model rendering, source-specific omissions and at most one notification attempt. Extend the existing calendar reader with explicit local date ranges for later compositions. Compare DST, same-day restart, unavailable email, ordinary schedules and notification failure in the morning eval. |
| 3. Price watch. `interaction/price_watch.rs`, task amendments, observations and receipts | Preserve task and operation IDs, source digest, model/specification, currency and shipping, active revision, expiry, next check and history. Transfer the polling clock once. Preserve suspended, finished and expired states. An existing price-evidence receipt proves the observation only. At ¥6,000 exactly there is no match. Race an old ¥6,000 check returning ¥5,800 against a new ¥5,500 limit. Expect no completion or send. Test first sample already below, missing shipping and restart with an uncertain delivery. |

Use the existing `just smoke-weekly-report`, `just eval-morning-briefing`, `just smoke-price-watch` and recovery fixtures. Acceptance compares observations, recipients, drafts and receipts, including the count of external attempts.

## Stage 5 — Held-out acceptance and release

From an ordinary new Web conversation say “Every Friday afternoon, summarize next week's meetings and my currently unfinished tasks in two paragraphs in this chat, with sources.” Place one meeting just inside the next local calendar week, one just outside, one open task and one completed task. Make Friday afternoon's chosen time explicit on the card. A second rendering with different wording and a changed date window must use the same capabilities. Keep this request out of bundled templates, router phrase matches, production prompts and task-specific branches.

1. Preview twice. Only the in-range meeting and open task appear, in two paragraphs with resolvable source references. No subscription, live cursor, approval or outbound message is created by preview.
2. Enable once, advance a controlled clock across Friday, replay the same trigger and reopen the store. Expect exactly one attributed main-chat result and one terminal occurrence. Check the source snapshot and receipt, plus absence of email or Feishu sends.
3. Say “move it to Thursday and only keep meetings.” Show the delta, confirm the new revision and exercise a future Thursday. The former Friday slot and task source produce no work. Pause in chat, verify the Web card and advance beyond another due time. Resume only future eligible slots.
4. Record deterministic Runtime/eval results, a real-model chat transcript with clarification and edit, and hosted/self-hosted Web observations separately. Include duty ID, revision, slot, source references, delivery record and external attempt count. Run schema drift checks, `just check` and the migration smokes. Close the plan after all three cutovers and the held-out conversation pass, with landed contracts documented in the bilingual formal docs.
