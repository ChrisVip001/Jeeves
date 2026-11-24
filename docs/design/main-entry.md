# Jeeves design: conversation, work and delivery

English | [简体中文](main-entry.zh.md)

This is the single final design document for Jeeves. It defines the product model, the runtime
contract with acceptance evidence, recurring work, and where the visual and voice specifications
live. Implementation detail stays in the linked runtime guides and the protocol.

## 1. Start with a message

Talk to Jeeves in your own words. It can answer a question in place or take responsibility for an
outcome, from a quick comparison to work that takes several days. You can describe the situation
before you know exactly what to ask for.

The Web and iOS workspaces share one Runtime conversation. Feishu retains its verified chat and
sender and can follow up on work assigned from that origin. The paired Mac device grants access to
selected windows when an assignment needs it. Chat and Work are the everyday entry points. The
menu provides account, device and connection management. Messages lead with the request and reply.
The timestamp reveals the execution details.

While the cloud workspace starts, you can draft your request. Reloading the tab keeps that draft,
and Send becomes available when the conversation opens.

The composer shows the working directory and the selected task for follow-ups. When an action needs
approval, the conversation shows its scope and preview for the user to decide. Operators configure
model routing, runtime limits and permission policies through the Runtime management interfaces.
See the [guide](../guide.md) for personal deployment setup.

## 2. Request, goal, delivery

Jeeves separates what a person asks from how the work is carried out:

| Layer | What it is | Visible to the person |
|---|---|---|
| **Request** | Whatever the person says, in their own words — a question, a vague wish, a picture with a few words, or a detailed assignment | Their own words in the conversation |
| **Goal** | The outcome Jeeves extracts from the request and, when needed, clarifies into something concrete | Confirmed in the conversation before work proceeds |
| **Delivery** | The result: an answer, a sourced comparison, a file, tested code, or the receipt of an authorized action | Shown in the conversation |

How a goal is executed is an internal scheduling decision, not a concept the person has to name.
A quick reply happens in place. Work that needs several steps, outside sources or an approval can
also run inside the conversation. Only work that must survive a restart, own a working directory,
or be measured against durable checks becomes a **task thread** — a persistent record with a
budget, checkpoints and its own return address. A task thread is one way to execute a goal, not
another word for the goal itself. Every thread opens on its latest outcome, pending decisions and
deliverable files, and decisions return to the person who asked.

Jeeves identifies the intended outcome from the conversation and carries the person's constraints
into the goal. It can investigate and proceed on reversible assumptions. If a missing detail would
materially change the outcome, cost or permission and the available context cannot resolve it,
Jeeves asks a focused question. The choice between answering in place, executing inside the
conversation, and opening a task thread follows from the goal, not from a button the person presses.
The result fits the work: a conclusion with sources, a file, a new or updated program with
verification, or the receipt for an authorized action. For an external action, the status comes
from the service's response or confirmed record.

| Intent | Interaction |
|---|---|
| Delegate | Tell Jeeves what you need in Chat and attach files if needed. The task title summarizes the outcome and the task keeps the original request. A bound project folder is visible above the message. Each task Jeeves starts appears under its reply as a card with a live status; select it to open the task in Work. The card returns after a reload. A new assignment in a clean Git repository uses its own checkout. Select it to switch back to automatic workspaces. |
| Decide | One pending decision opens with its action and exact preview. Allow once or deny the current request. |
| Recover | A stopped work page offers [operation review](../operations.md#reviewing-unknown-operations) and an explicit resume on the same assignment. |
| Find work | Search titles in Work or follow a receipt. On a wide screen the list stays on the left and the open item fills the right; on a phone the list opens the item as its own page. Reload, back and forward retain the destination. |
| Inspect delivery | The latest terminal run has its own receipt. Results and files precede expandable conversation history. Replies from older runtimes are labeled separately. |
| Review a file | Open a delivered file in the right pane on Mac or full screen on phone. For repository tasks, switch to Line changes to see additions and deletions with old and new line numbers. Save a copy from the text preview. |
| Amend | Reply directly in the work page. Drafts are scoped to their chat or assignment within the current browser tab and survive reload. |
| Manage the account | The profile leads with the account and entry to the assistant. Account and security groups devices, password changes and two-factor authentication. |

### Main-chat model and task references

DeepSeek V4.1 Flash, API ID `deepseek-flash`, is the first candidate to evaluate for main-chat
intent recognition. This choice is scoped to evaluation. Background tasks keep their execution
model, development uses GPT-6.2 Sol / high, and the default configuration stays in place. To repeat
the opt-in check, run `cargo run -p jeeves-provider --example deepseek_intents -- <home> <profile>`
from the repository root.

When the person says “Hold off on sending that one. Make it a draft,” the Runtime resolves the draft
request against the selected task or an explicit human reference. A backward reference may use the
immediately preceding completed private interaction. If several actions fit, the user chooses an
action ID. The task ID stays fixed across iPhone and Mac. The send action has its own record. If two
targets fit, the main chat asks the person to choose using distinguishable subjects or task names.
Recency alone cannot resolve that choice. Both targets stay untouched until the person answers.

The Runtime verifies task ownership, the action's association with that task, current action state
and permissions. It applies the change only to a unique, still-valid target. Task progress cannot
establish whether mail was sent or cancelled. Whether the send can be stopped depends on which
commits first, the draft change or the sender's claim. The reply reflects the durable action record
and any acceptance receipt. See the [task mail contract](../configuration.md#calendar-and-email).

After comparing prices from a configured read-only quote source, you can ask Jeeves to watch one
variant for a fixed number of days. The assignment keeps the source, variant and currency. Jeeves
checks the price plus shipping hourly. You can open the same item in Work on your phone and
change the threshold. The first verified total strictly below the current threshold returns to the
original chat with its source. An equal total, unknown shipping or an expired watch produces no
alert. Watching a price authorizes no purchase.

**Needs your decision** has the highest notification priority. On iPhone, the person answers in the
main chat, which routes the reply to the waiting task. When a task reaches **Completed**, Jeeves
notifies once at normal priority and reports in the originating chat. Routine progress stays quiet
between agreed milestones. System notifications follow the person's iOS settings.

## 3. Where work happens

| Use | Place | What the person sees |
|---|---|---|
| Conversation | Main chat | An ordinary reply |
| Research | Main chat for simple questions, a task for delegated research | Findings in the main chat |
| Internal and local search | Main chat or task thread over connected internal sources and local files | Sourced findings from intranet and local documents |
| Documents (slides, spreadsheets, Markdown) | Task thread with source material | A delivered file the person can open, review and revise |
| Coding | Task thread and workspace | Changes and tests, with confirmation before merging |
| Long-running work | Task thread | Approach at kickoff, progress at checkpoints and early notice of risks |
| Monitoring prices or time | Scheduled or event-driven task | An update when the trigger fires |
| Compare prices | Main chat or task thread | Options with current prices, availability and sources |
| Place an order | Task thread with a supported, configured purchase channel | Confirm the item, total and payment method before purchase. A successful order returns a receipt. |
| Video and creative work | Task thread with materials and previews | The person decides on the subject and publication |

Assignments made in chat, explicitly created tasks and resumed runs share delivery criteria. The
Runtime can check files and required text, JSON values, quotations against saved sources, exit codes
of specified tests, and operation records or receipts. Acceptance commands use the workspace
execution permissions. A failed check returns to the same task for correction within its round
limit. Task details retain the criteria and measurements from each round. A follow-up after
delivery opens a new run in the same task and workspace. The previous run's checks do not carry
over. When the follow-up itself names a file, required text or test command, those become the new
run's checks and a failure is corrected in the same task; otherwise the output is marked
unverified. A task run that used tools but has no checks and no `/goal` condition gets one
self-check round before delivery, within its round limit. Jeeves rereads the original assignment,
opens what it produced, fixes any gaps and states what it could not verify. The task timeline
records the round. The result is still marked unverified, because a self-check is not a measurement.
Correction stops early when it is not getting anywhere. After the first correction round, a round
that ends with the same failed check or goal verdict as the round before, or that ran no tool
successfully, counts as stalled. Two stalled rounds in a row hand the task back to the person as
needing attention, with the repeated reason, instead of spending the rest of the round limit.

When an outcome is clear but no mechanical check can measure it, Jeeves may draft one "done when"
sentence as it opens the task. A draft judges nothing: the task runs as an unjudged one and its
card asks the person to use the standard or skip it. Confirming it makes it the task's goal
condition, so later rounds are judged against it; skipping drops it. Either answer removes the
draft, so the card never asks twice, and a condition the person set themselves is never replaced.
Goal activation stays the person's: the model can propose the wording, never adopt it.

### Working on a long task

At kickoff, Jeeves briefly explains how it intends to approach the assignment and when the person
can expect an update. Before a decision materially changes the agreed goal, cost, permissions or
commitments, it asks the person to decide. As soon as the promised result looks at risk, it reports
what is at risk and what can still be delivered. A task pauses at a resource limit, on a human stop,
or when an external action needs review. [Runtime limits](../configuration.md#behavior-knobs)

Mac and phone can search task titles and open the same execution session. The main conversation
collects task notices and receipts.

## 4. Runtime contract

The main conversation can deliver a result immediately or continue the same goal in a durable task.
This section records the implementation contract; acceptance evidence is in section 5.

**Conversation and task ownership.** Private main chat selects inline delivery or delegation from
the requested outcome. Per-operation admission depends on the user, source, workspace, device grant
and action. Keep the restriction on mutations while an earlier effect is unresolved. The same Rust
driver serves conversation work, durable work and recovery. The app-server owns admission and
scheduling. Shells submit human requests and render committed results.
[Tool assembly](../../runtime/crates/jeeves-app-server/src/turn/execution.rs) ·
[Architecture](../architecture.md)

**Moving work into a task.** Bind the originating message, confirmed goal, current amendments,
evidence references, settled actions, remaining work and return destination in one recoverable
transition. Retrying the request must recover the same task even if the process stopped between
creation and dispatch. Keep prior measurements attached to the version they checked. A revised
threshold supersedes the old one. Reuse the durable handoff and source ownership checks. An
assignment retains the original authority boundaries through the transition.
[Main-chat handoff](../../runtime/crates/jeeves-app-server/src/interaction/task_tool.rs) ·
[Task handoff](../../runtime/crates/jeeves-app-server/src/task/handoff.rs)

**Tool calls and recovery.** Give each proposed invocation a stable identity. Persist dispatch
intent immediately before handing an effectful call to its executor, then commit its observed
outcome before advancing to a dependent action. A crash between dispatch intent and a confirmed
outcome leaves that call uncertain. Preserve proof of calls never admitted and results already
committed. Independent reads can finish and report separately while their model-facing results
retain request order. Return typed execution status, readable output and evidence references from
the tool boundary. Derive task steps, notifications and verification inputs from those records.
Remove status inference from text prefixes and reconstruction of executed calls from the model
transcript. [Loop](../../runtime/crates/jeeves-loop/src/lib.rs) ·
[Operation records](../../runtime/crates/jeeves-state/src/operations.rs) ·
[Task observations](../../runtime/crates/jeeves-agent/src/observe.rs)

**Model requests and context.** Record each request's purpose, execution owner, termination and
provider-reported usage. At the adapter boundary normalize full input, output and cache breakdown.
Charge the task for full input plus output once per request, including planning, summarization or
evaluation it invokes. Keep context occupancy, cumulative task consumption and monetary cost
distinct. Preserve unknown usage on failed requests. Use current instructions, amendments, settled
results, pending decisions and addressable evidence to assemble context for both continuation and
recovery. Retain file, source and receipt references through compaction. Evaluate the benefit and
cost of model-generated summaries against this baseline.
[Provider usage](../../runtime/crates/jeeves-provider/src/lib.rs) ·
[Budget](../../runtime/crates/jeeves-loop/src/budget.rs) ·
[Projection](../../runtime/crates/jeeves-loop/src/projection.rs)

**Delivery and verification.** Use the same typed criteria for a main-chat assignment, an explicit
task and a resumed run. Return a sourced conclusion for research, an openable artifact for
documents, the tested version and observed checks for code, and a service receipt for an authorized
send. Record subjective review separately from mechanical checks. An acceptance command runs under
normal execution policy. Failed criteria drive correction in the same task and remain visible
alongside the later measurement.
[Verifier](../../runtime/crates/jeeves-agent/src/verify.rs) ·
[Delivery regression](../../runtime/crates/jeeves-app-server/src/interaction/tests/deliverables.rs)

## 5. Acceptance evidence

Four phases moved the design into the product. Each row states the exit criterion and the recorded
local evidence. Deterministic regressions, real-model trials and deployment checks are recorded
separately; do not treat one as proof of another.

| Phase | Exit criterion | Evidence |
|---|---|---|
| 1 · Deliver in chat | A screenshot question, a sourced comparison and a previewed one-off approval complete in the composer with zero tasks created. A denial leaves the receiver untouched; a successful tool returning text beginning with `Error:` is recorded as successful. | Passed 2026-11-23. `just smoke-interaction --browser chrome`; all eight `just smoke-recovery` scenarios passed. |
| 2 · Continue across devices | Replay the Web handoff, change the threshold on iPhone and restart. Assert one task, the same variant and shipping basis, the latest threshold and one notification for the qualifying event. A different Feishu chat or sender cannot continue the assignment. | Passed with `just smoke-price-watch --ios-simulator <UDID>`. 1,660 + 40 shipping produced no alert; 1,659 + 40 produced one sourced receipt in the original chat. |
| 3 · Recover, account, verify | Interrupt before dispatch, after dispatch and after settlement. With a lost send receipt, reopen the saved report and reconcile only the send. Two provider usage formats stop at the same budget boundary after restart. After compaction and amendment, later requests carry the new constraint and an early source. Wrong JSON, an unsupported citation and a failing test each keep the delivery open for correction. | Passed 2026-11-23 with `just smoke-runtime-contract`. |
| 4 · Switch clients and storage | Migrate a copy of existing Mac data, then repeat assignment, approval and delivery across self-hosted and isolated hosted Runtimes. Web and iOS share the conversation; Feishu keeps its origin; Mac acts only on the selected window within its grant. | Passed 2026-11-23. `just check`, `just smoke-account`, `just smoke-interaction --browser chrome`, `just smoke-price-watch --ios-simulator <UDID>`, `just smoke-mac-window`. |

A v24 store migrated to schema 4 and reopened with all gateway accounts, tasks, artifacts,
completions, verification records and approvals preserved. All 26 historical operations without
executor previews remained unverified. In the live Feishu group the mentioned bot replied in the
original chat and the sender, chat and message IDs matched the completed Runtime receipt.

For model trials, count avoidable questions, unnecessary tasks, lost amendments, unsupported
conclusions and independently verified deliveries. For receiver tests count actual external
effects. Record model, elapsed time and usage. Distinguish simulated time, actual overnight
operation and physical-device observations.

## 6. Recurring work

Describe the schedule or event, the desired result and where it should appear. Jeeves proposes a
card for the owner to review. A later request can amend, pause or resume the same duty. Work
shows the confirmed definition and run history. A new account starts with no recurring duties.

**Composition.** A trigger can be an exact time, a delay after approval, an interval, a calendar
schedule or an event from a signed source. Deterministic actions can render reminders, build a
digest from approved sources with fixed arguments, verify saved evidence or test structured records.
The read action calls one enabled read-only tool and returns its structured result as text. A digest
lists several sources, each an approved read-only tool plus arguments, or the earlier named
calendar/task/mail sources for compatibility. A record action can test the incoming event, the
latest signed record within a freshness limit, or a record fetched from an approved read-only tool
on each run. The review card names the exact tool, arguments, comparison, units or fields, and
destination. The `capabilities` result lists every available read tool with its input schema.

A model draft can reference a reviewed version of an installed skill. The owner separately enables
the source and connector capabilities. Adding a task from existing capabilities requires no engine
change. A new provider authentication flow, source protocol or external write needs a connector
implementation.

**Delivery.** Local delivery appears in the main chat or remains a private draft. An outbound
connector adapter exposes its destination through the same catalog and uses the existing approval
and receipt boundary. The Feishu integration runs only when the owner enables it. Each external
send needs approval for the exact content and destination, with the current revision and connector
scope rechecked immediately before dispatch.

**Execution and recovery.** Timer and signed-event admission record one wake per occurrence. The
timer admits at most one recent occurrence within a 15-minute catch-up window. Signed events advance
the cursor and create wakes in one transaction. Repeating an event or timer slot adds no delivery.
A revision change holds outstanding wakes for review. A lapsed execution lease or interruption
leaves a review entry. Run history shows blocked, skipped, dispatched and interrupted occurrences.
Previews perform read-only checks and do not advance clocks or event cursors.
[Runtime protocol](../protocol.md)

**Upgraded duties.** A weekly research report keeps its verified evidence action, seven-day window
and authorized URLs; the saved recipient becomes a connector destination. A configured morning
briefing is paused as a proposed private digest until the owner confirms sources, time and
destination. A price watch is paused; observations, history, threshold revisions and receipts
remain, and resuming requires a newly confirmed structured rule.

**Acceptance.** `just smoke-recurring-work` creates, previews, amends and pauses a digest and runs a
signed-source event case with zero model requests. `just eval-morning-briefing` and
`just smoke-price-watch` upgrade a v14 database and inspect paused duties across restarts.
`just smoke-weekly-report` covers the legacy registration entry point, approval, a loopback Feishu
receiver and crash reconciliation. The
[real-model expense-mail acceptance](../../evals/results/recurring-event-live.json) records the
natural-language proposal with synthetic mail metadata.

## 7. Presentation and voice

The [visual design system](DESIGN.md) owns colour tokens, typography, spacing, components and
layout. The [voice and reporting guide](voice.zh.md) owns how every line of copy is written, the
report card structure and the baseline persona. Both are reference manuals for a different
audience than this design; they are not repeated here. For a task rail, card or confirmation
change, read those two guides before editing a component or a prompt.

Task ownership, routing and live approvals are specified in the
[main interaction protocol](../protocol.md#main-human-interaction).
