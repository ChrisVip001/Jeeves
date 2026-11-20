# Main entry: one conversation and independent task threads

English | [简体中文](main-entry.zh.md)

The Runtime already provides the main conversation and independent task sessions. The iPhone task list and the Mac mode switch below share the dashboard frontend.

The phone task list and Mac Cowork read the same execution conversation and artifacts. The main chat keeps the complete approval preview, including the publishing account, channel, destination and final text. Reply with `approve <requestId> v<version>` or `deny <requestId> v<version>`. A task revision retires its outstanding send approval. The completion receipt creates one original-chat result for that execution.

## 1. Purpose

People can chat, ask questions, delegate research or coding, plan a purchase or work on a creative project in the same place. Jeeves opens a task thread when the person delegates work.

## 2. Why tasks have their own threads

An extended collaboration can span dozens of exchanges. Keeping its transcript together makes earlier decisions easier to find, keeps confirmations visible in the main conversation and gives each task its own model context.

## 3. Structure

On iPhone, the main chat is the only place to assign work, send follow-ups or answer a task’s question. Jeeves routes a follow-up or answer to the existing task. The main chat also receives progress, requests for confirmation and results.

From the main chat, the person can browse all task conversations under **In progress**, **Needs your decision** and **Completed**. The list and its detail view are read-only. Opening a task shows the same full conversation, details and result visible to the user in Mac Cowork, as they become available. The message composer stays in the main chat. A signed push opens a separate review of the same approval request and version. From that review the owner may decide once or follow up in the main chat with the reviewed task selected.

On Mac, a switch at the top left lets the person choose **Assistant** or **Cowork**. Assistant shows the same main chat, relevant information and recent activity. Cowork is the workbench. Each task has its own conversation, and opening it on either device reads the same persisted task and conversation. Delegating work keeps the current view open.

| Part | Contents |
|---|---|
| Main conversation (`jeeves-main`) | Everyday chat, questions, assignments, progress, requests for confirmation and results |
| Task thread | The working conversation for one task, with its own context. On Mac the person can continue it in Cowork. |
| History | Search and a timeline by day, including archived tasks |

The main model uses the `task` tool when the person gives an explicit assignment. Discussion and progress questions stay in chat. A follow-up goes to the original task. If its reference is ambiguous, Jeeves asks which task the person means. Progress, requests for confirmation and results return to the main chat. Each external action still requires its applicable authorization.

### Main-chat model and task references

DeepSeek V4.1 Flash, API ID `deepseek-flash`, is the first candidate to evaluate for main-chat intent recognition. This choice is scoped to evaluation. Background tasks keep their execution model, development uses GPT-6.2 Sol / high, and the default configuration stays in place. [DeepSeek model list](https://api-docs.deepseek.com/api/list-models/)

To repeat the opt-in Flash intent check, run `cargo run -p jeeves-provider --example deepseek_intents -- <jeeves-home> <flash-profile-id>` from `jeeves-rs/`. It makes one request to the official endpoint with four synthetic messages covering conversation, assignment, an explicit task reference and an ambiguous reference. It checks the proposed intent and target. The desktop acceptance exercises routing, approval and delivery separately with local fixtures.

When the person says “Hold off on sending that one. Make it a draft,” the Runtime resolves the draft request against the selected task or an explicit human reference. A backward reference may use the immediately preceding completed private interaction. If several actions fit, the user chooses an action ID. The task ID stays fixed across iPhone and Mac. The send action has its own record. If two targets fit, the main chat asks the person to choose using distinguishable subjects or task names. Recency alone cannot resolve that choice. Both targets stay untouched until the person answers.

The Runtime verifies task ownership, the action’s association with that task, current action state and permissions. It applies the change only to a unique, still-valid target. Task progress cannot establish whether mail was sent or cancelled. Whether the send can be stopped depends on which commits first, the draft change or the sender’s claim. The reply reflects the durable action record and any acceptance receipt. See the [first architecture acceptance scenario](../planning/execution-plan.md#first-architecture-acceptance-scenario-turn-a-queued-send-into-a-draft-pending).

### Task notifications

**Needs your decision** has the highest notification priority in Jeeves. On iPhone, the person answers in the main chat, which routes the reply to the waiting task. When a task reaches **Completed**, Jeeves proactively notifies the person once at normal priority and reports the result in the originating chat. Routine progress stays quiet between agreed milestone reports. These priorities govern how Jeeves presents updates. System notifications always respect the person’s iOS notification settings.

## 4. Where work happens

| Use | Place | What the person sees |
|---|---|---|
| Conversation | Main chat | An ordinary reply |
| Research | Main chat for simple questions, a task for delegated research | Findings in the main chat |
| Coding | Task thread and workspace | Changes and tests, with confirmation before merging |
| Long-running work | Task thread | Approach at kickoff, progress at checkpoints and early notice of risks |
| Monitoring prices or time | Scheduled or event-driven task | An update when the trigger fires |
| Comparing and buying | Task thread | A confirmation showing the item, total and payment method before purchase |
| Video and creative work | Task thread with materials and previews | The person decides on the subject and publication |

### Working on a long task

At kickoff, Jeeves briefly explains how it intends to approach the assignment and when the person can expect an update. It adjusts routine steps as it learns. Before a decision materially changes the agreed goal, cost, permissions or commitments, it asks the person to decide. As soon as the promised result looks at risk, it reports what is at risk and what can still be delivered.

## 5. Remaining interface work

The phone task list and Mac Cowork show the same execution session. The main chat collects task notices and receipts. Search and chronological browsing of earlier tasks remain planned.

## 6. Related documents

The [visual design](DESIGN.md) and [voice guide (Chinese)](voice.zh.md) describe the task rail, cards and confirmation copy. Runtime routing, task ownership and live approvals are documented in the [main interaction protocol](../protocol.md#main-human-interaction).
