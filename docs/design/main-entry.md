# Chat, work and delivery in Jeeves

English | [简体中文](main-entry.zh.md)

## 1. Start with a message

Talk to Jeeves in your own words. It can answer a question in place or take responsibility for an outcome, from a quick comparison to work that takes several days. You can describe the situation before you know exactly what to ask for.

The Web and iOS workspaces share one Runtime conversation. Feishu retains its verified chat and sender and can follow up on work assigned from that origin. The paired Mac device grants access to selected windows when an assignment needs it. Chat and My work are the everyday entry points. The menu holds the account, tools and settings. Messages lead with the request and reply. The timestamp reveals the execution details.

While the cloud workspace starts, you can draft your request. Reloading the tab keeps that draft, and Send becomes available when the conversation opens.

### Interface boundary

The intended shape is a restrained workbench: the goal sits at the top, and the conversation is where a person states an outcome, sees decisions and takes delivery. Configuration — models, permissions, schedules, usage and audit — belongs in the CLI, `models.json`, `model-routes.json` and the `/api/*` surface, not in cockpit panels. Approval cards belong inside the conversation. The current build still ships Settings, model/permission pickers and a capability centre as an interim step toward that boundary; treat them as transitional, not as the target to extend.

## 2. Design references

Grok allows people to reopen an automation run and continue its conversation. A dot takes assignments in chat, works between conversations and brings decisions back to its owner. Each Jeeves assignment therefore has a return address and opens on the latest outcome, decisions and files. [Grok automations](https://x.ai/news/grok-automations) · [Working with a dot](https://help.openai.com/en/articles/20001530-getting-started-with-your-dot)

## 3. From assignment to delivery

Jeeves identifies the intended outcome from the conversation and carries the person's constraints into the assignment. It can investigate and proceed on reversible assumptions. If a missing detail would materially change the outcome, cost or permission and the available context cannot resolve it, Jeeves asks a focused question.

The result fits the work. Jeeves brings back a conclusion with sources, a file, a new or updated program with verification, or the receipt for an authorized action. For an external action, the status comes from the service's response or confirmed record.

| Intent | Interaction |
|---|---|
| Delegate | Tell Jeeves what you need in Chat and attach files if needed. The task title summarizes the outcome and the task keeps the original request. A bound project folder is visible above the message. A new assignment in a clean Git repository uses its own checkout. Select it to switch back to automatic workspaces. |
| Decide | One pending decision opens with its action and exact preview. Allow once or deny the current request. |
| Recover | A stopped work page offers [operation review](../operations.md#reviewing-unknown-operations) and an explicit resume on the same assignment. |
| Find work | Search titles in My work or follow a receipt. Mac and phone open the same work page. Reload, back and forward retain the destination. |
| Inspect delivery | The latest terminal run has its own receipt. Results and files precede expandable conversation history. Replies from older runtimes are labeled separately. |
| Review a file | Open a delivered file in the right pane on Mac or full screen on phone. For repository tasks, switch to Line changes to see additions and deletions with old and new line numbers. Save a copy from the text preview. |
| Amend | Reply directly in the work page. Drafts are scoped to their chat or assignment within the current browser tab and survive reload. |
| Manage the account | The profile leads with the account and entry to the assistant. Account and security groups devices, password changes and two-factor authentication. |

### Main-chat model and task references

DeepSeek V4.1 Flash, API ID `deepseek-flash`, is the first candidate to evaluate for main-chat intent recognition. This choice is scoped to evaluation. Background tasks keep their execution model, development uses GPT-6.2 Sol / high, and the default configuration stays in place. [DeepSeek model list](https://api-docs.deepseek.com/api/list-models/)

To repeat the opt-in Flash intent check, run `cargo run -p jeeves-provider --example deepseek_intents -- <jeeves-home> <flash-profile-id>` from the repository root. It makes one request to the official endpoint with four synthetic messages covering conversation, assignment, an explicit task reference and an ambiguous reference. It checks the proposed intent and target. The desktop acceptance exercises routing, approval and delivery separately with local fixtures.

When the person says “Hold off on sending that one. Make it a draft,” the Runtime resolves the draft request against the selected task or an explicit human reference. A backward reference may use the immediately preceding completed private interaction. If several actions fit, the user chooses an action ID. The task ID stays fixed across iPhone and Mac. The send action has its own record. If two targets fit, the main chat asks the person to choose using distinguishable subjects or task names. Recency alone cannot resolve that choice. Both targets stay untouched until the person answers.

The Runtime verifies task ownership, the action’s association with that task, current action state and permissions. It applies the change only to a unique, still-valid target. Task progress cannot establish whether mail was sent or cancelled. Whether the send can be stopped depends on which commits first, the draft change or the sender’s claim. The reply reflects the durable action record and any acceptance receipt. See the [task mail contract](../configuration.md#calendar-and-email).

### Task notifications

**Needs your decision** has the highest notification priority in Jeeves. On iPhone, the person answers in the main chat, which routes the reply to the waiting task. When a task reaches **Completed**, Jeeves proactively notifies the person once at normal priority and reports the result in the originating chat. Routine progress stays quiet between agreed milestone reports. These priorities govern how Jeeves presents updates. System notifications always respect the person’s iOS notification settings.

## 4. Where work happens

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

### Working on a long task

When an assignment explicitly names an output file, main chat can hand off a file check. If the person also specifies literal text that must appear, the check measures that text in the file. The Runtime records each result against the task's workspace and sends a failed check back for correction within the task's round limit. The task detail shows the recorded criteria and verdicts. Source accuracy, code behavior and other semantic requirements need their own evidence.

A follow-up after delivery opens a new run in the same task and workspace. The earlier receipt stays attached to its run. The follow-up starts without file checks, so its output is marked unverified.

At kickoff, Jeeves briefly explains how it intends to approach the assignment and when the person can expect an update. It adjusts routine steps as it learns. Before a decision materially changes the agreed goal, cost, permissions or commitments, it asks the person to decide. As soon as the promised result looks at risk, it reports what is at risk and what can still be delivered.

When a required tool encounters a problem, Jeeves reports the affected operation and the evidence from the current assignment.

## 5. Remaining interface work

Mac and phone can search task titles and open the same execution session. The main conversation collects task notices and receipts.

## 6. Related documents

The [visual design](DESIGN.md) and [voice guide (Chinese)](voice.zh.md) describe the task rail, cards and confirmation copy. Runtime routing, task ownership and live approvals are documented in the [main interaction protocol](../protocol.md#main-human-interaction).
