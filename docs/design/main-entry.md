# Chat, work and delivery in Jeeves

English | [简体中文](main-entry.zh.md)

## 1. Start with a message

Talk to Jeeves in your own words. It can answer a question in place or take responsibility for an outcome, from a quick comparison to work that takes several days. You can describe the situation before you know exactly what to ask for.

The Web and iOS workspaces share one Runtime conversation. Feishu retains its verified chat and sender and can follow up on work assigned from that origin. The paired Mac device grants access to selected windows when an assignment needs it. Chat and My work are the everyday entry points. The menu provides account, device and connection management. Messages lead with the request and reply. The timestamp reveals the execution details.

While the cloud workspace starts, you can draft your request. Reloading the tab keeps that draft, and Send becomes available when the conversation opens.

### Interface boundary

The composer shows the working directory and the selected task for follow-ups. When an action needs approval, the conversation shows its scope and preview for the user to decide. Operators configure model routing, runtime limits and permission policies through the Runtime management interfaces. See the [guide](../guide.md) for personal deployment setup.

## 2. Design principles

Jeeves separates what a person asks from how the work is carried out:

| Layer | What it is | Visible to the person |
|---|---|---|
| **Request** | Whatever the person says, in their own words — a question, a vague wish, a picture with a few words, or a detailed assignment | Their own words in the conversation |
| **Goal** | The outcome Jeeves extracts from the request and, when needed, clarifies into something concrete | Confirmed in the conversation before work proceeds |
| **Delivery** | The result: an answer, a sourced comparison, a file, tested code, or the receipt of an authorized action | Shown in the conversation |

How a goal is executed is an internal scheduling decision, not a concept the person has to name. A quick reply happens in place. Work that needs several steps, outside sources or an approval can also run inside the conversation. Only work that must survive a restart, own a working directory, or be measured against durable checks becomes a **task thread** — a persistent record with a budget, checkpoints and its own return address. A task thread is one way to carry out a goal, not another word for the goal itself. Every thread opens on its latest outcome, pending decisions and deliverable files, and decisions return to the person who asked.

## 3. From request to delivery

Jeeves identifies the intended outcome from the conversation and carries the person's constraints into the goal. It can investigate and proceed on reversible assumptions. If a missing detail would materially change the outcome, cost or permission and the available context cannot resolve it, Jeeves asks a focused question. The choice between answering in place, executing inside the conversation, and opening a task thread follows from the goal, not from a button the person presses.

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

After comparing prices from a configured read-only quote source, you can ask Jeeves to watch one variant for a fixed number of days. The assignment keeps the source, variant and currency. Jeeves checks the price plus shipping hourly. You can open the same item in **My work** on your phone and change the threshold. The first verified total strictly below the current threshold returns to the original chat with its source. An equal total, unknown shipping or an expired watch produces no alert. Watching a price authorizes no purchase.

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

Jeeves keeps working in the same task as tool results arrive. Approvals still come to you. A task pauses when work reaches a resource limit, when you stop it or when an external action needs review. [Runtime limits](../configuration.md#behavior-knobs)

Assignments made in chat, explicitly created tasks and resumed runs share delivery criteria. The Runtime can check files and required text, JSON values, quotations against saved sources, exit codes of specified tests, and operation records or receipts. Acceptance commands use the workspace execution permissions. A failed check returns to the same task for correction within its round limit. Task details retain the criteria and measurements from each round.

A follow-up after delivery opens a new run in the same task and workspace. The earlier receipt stays attached to its run. The follow-up starts without file checks, so its output is marked unverified.

At kickoff, Jeeves briefly explains how it intends to approach the assignment and when the person can expect an update. It adjusts routine steps as it learns. Before a decision materially changes the agreed goal, cost, permissions or commitments, it asks the person to decide. As soon as the promised result looks at risk, it reports what is at risk and what can still be delivered.

When a required tool encounters a problem, Jeeves reports the affected operation and the evidence from the current assignment.

## 5. Remaining interface work

Mac and phone can search task titles and open the same execution session. The main conversation collects task notices and receipts.

## 6. Related documents

The [visual design](DESIGN.md) and [voice guide (Chinese)](voice.zh.md) describe the task rail, cards and confirmation copy. Runtime routing, task ownership and live approvals are documented in the [main interaction protocol](../protocol.md#main-human-interaction).
