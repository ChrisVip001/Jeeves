# Chat and Activity in Jeeves

English | [简体中文](main-entry.zh.md)

## 1. Two destinations

Jeeves opens in Chat. Ask a question, delegate an assignment or change direction there. Activity keeps delegated work under Needs your decision, Running and Finished. Jeeves answers ordinary questions directly and creates a task for sustained work. The person can move on to another subject as soon as they delegate.

## 2. Design references

Grok lets people describe a recurring job in chat and reopen an individual run to continue the conversation. In Jeeves each receipt leads back to the work it reports on. [Automations in Grok](https://x.ai/news/grok-automations)

Grok Bot presents the recipients and draft of an outgoing message for review in chat. A new message can redirect work in progress. In Jeeves the conversation therefore carries the current decision and its preview. [Messaging and collaborating with Grok Bot](https://docs.x.ai/grok-bot/chat-and-collaboration)

A dot accepts a goal in conversation, continues between conversations and reports scheduled updates in chat. Tasks retain their own context. Jeeves gives talking to the assistant and finding work across assignments separate destinations. [Getting started with your dot](https://help.openai.com/en/articles/20001530-getting-started-with-your-dot)

## 3. From assignment to delivery

| Intent | Interaction |
|---|---|
| Delegate | In Chat, ask for a one-page summary of open issues for Monday’s client meeting. Attach a file beside the composer. Chat options hold the model, workspace and permissions. |
| Change direction | Send an amendment. If the reference is ambiguous, Jeeves asks which assignment you mean. Selecting Follow up on a receipt puts the original task’s name in the composer. |
| Decide | Expand Pending decisions in Chat, review the exact action and choose Allow once or Deny. Older requests are available as expandable history. |
| Review results | Each receipt shows its run’s outcome. Activity lists other assignments. On Mac, selecting one opens its working conversation. On a phone, inspect it and return to Chat with that task selected. |
| Manage Jeeves | The menu holds settings, connections, schedules and advanced tools. A signed notification still opens its corresponding approval request. |

### Main-chat model and task references

DeepSeek V4.1 Flash, API ID `deepseek-flash`, is the first candidate to evaluate for main-chat intent recognition. This choice is scoped to evaluation. Background tasks keep their execution model, development uses GPT-6.2 Sol / high, and the default configuration stays in place. [DeepSeek model list](https://api-docs.deepseek.com/api/list-models/)

To repeat the opt-in Flash intent check, run `cargo run -p jeeves-provider --example deepseek_intents -- <jeeves-home> <flash-profile-id>` from `jeeves-rs/`. It makes one request to the official endpoint with four synthetic messages covering conversation, assignment, an explicit task reference and an ambiguous reference. It checks the proposed intent and target. The desktop acceptance exercises routing, approval and delivery separately with local fixtures.

When the person says “Hold off on sending that one. Make it a draft,” the Runtime resolves the draft request against the selected task or an explicit human reference. A backward reference may use the immediately preceding completed private interaction. If several actions fit, the user chooses an action ID. The task ID stays fixed across iPhone and Mac. The send action has its own record. If two targets fit, the main chat asks the person to choose using distinguishable subjects or task names. Recency alone cannot resolve that choice. Both targets stay untouched until the person answers.

The Runtime verifies task ownership, the action’s association with that task, current action state and permissions. It applies the change only to a unique, still-valid target. Task progress cannot establish whether mail was sent or cancelled. Whether the send can be stopped depends on which commits first, the draft change or the sender’s claim. The reply reflects the durable action record and any acceptance receipt. See the [task mail contract](../configuration.md#calendar-and-email).

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

When an assignment explicitly names an output file, main chat can hand off a file check. If the person also specifies literal text that must appear, the check measures that text in the file. The Runtime records each result against the task's workspace and sends a failed check back for correction within the task's round limit. The task detail shows the recorded criteria and verdicts. Source accuracy, code behavior and other semantic requirements need their own evidence.

A follow-up after delivery opens a new run in the same task and workspace. The earlier receipt stays attached to its run. The follow-up starts without file checks, so its output is marked unverified.

At kickoff, Jeeves briefly explains how it intends to approach the assignment and when the person can expect an update. It adjusts routine steps as it learns. Before a decision materially changes the agreed goal, cost, permissions or commitments, it asks the person to decide. As soon as the promised result looks at risk, it reports what is at risk and what can still be delivered.

## 5. Remaining interface work

The phone task list and Mac Cowork show the same execution session. The main chat collects task notices and receipts. Search and chronological browsing of earlier tasks remain planned.

## 6. Related documents

The [visual design](DESIGN.md) and [voice guide (Chinese)](voice.zh.md) describe the task rail, cards and confirmation copy. Runtime routing, task ownership and live approvals are documented in the [main interaction protocol](../protocol.md#main-human-interaction).
