# Recurring work from the main conversation

English | [简体中文](composable-responsibilities.zh.md)

Describe the schedule or event, the desired result and where it should appear. Jeeves proposes a card for the owner to review. A later request can amend, pause or resume the same duty. My Work shows the confirmed definition and run history. A new account starts with no recurring duties.

## Compose a duty

A trigger can be an exact time, a delay after approval, an interval, a calendar schedule or an event from a signed source. Deterministic actions can render reminders and source summaries, verify saved evidence or test structured records. A record action can test the incoming event or have a timer check the latest signed record against a freshness limit. For example, ask Jeeves to tell you here when the latest available reading falls below a limit. The review card names the source, comparison, units or fields, and destination.

A model draft can reference a reviewed version of an installed skill. A skill can supply reusable instructions. The owner separately enables the source and connector capabilities. Jeeves can compose those capabilities from a natural-language request. A new provider authentication flow, source protocol or external write needs a connector implementation. Adding a task from existing capabilities requires no engine change.

Local delivery can appear in the main chat or remain a private draft. An optional outbound adapter exposes its destination through the same catalog and uses the existing approval and receipt boundary. The built-in Feishu integration runs only when the owner enables it. Each external send needs approval for the exact content and destination, with the current revision and connector scope rechecked immediately before dispatch.

## Execution and recovery

Timer and signed-event admission record one wake per occurrence. The timer admits at most one recent occurrence within a 15-minute catch-up window. Signed events advance the cursor and create wakes in one transaction. Repeating an event or timer slot adds no delivery. A revision change holds outstanding wakes for review. A lapsed execution lease or interruption leaves a review entry. Run history shows blocked, skipped, dispatched and interrupted occurrences. Previews perform read-only checks and do not advance clocks or event cursors. [Runtime protocol](../protocol.md)

## Existing duties after upgrade

| Stored duty | Upgrade behavior |
| --- | --- |
| Weekly research report | The verified evidence action retains its seven-day window and authorized URLs. The shared timer resolves the Runtime's local time zone. The saved recipient becomes a connector destination. Drafts and receipts remain available. |
| Morning briefing | The previous clock time and source set are preserved in a proposed private digest. The duty is paused until the owner confirms the output, sources and destination, then resumes it. |
| Price watch | The old monitor is paused. Quote observations, task history, threshold revisions and receipts remain. To resume, connect a structured source and confirm a new rule, including currency, product identity and shipping data where relevant. |

## Acceptance

- `just smoke-recurring-work` uses disposable Web accounts and a scripted model. It creates, previews, amends and pauses a calendar/task digest. Its separate expense-mail case configures a signed source, matches a finance sender and a PDF attachment, then delivers one sourced notification. Mismatches, replay across restart and an event after pause add no delivery. The event worker uses zero model requests.
- `just eval-morning-briefing` and `just smoke-price-watch` upgrade a v14 database and inspect paused duties and retained history across restarts. The latter also exercises a numeric predicate against a recent signed record on the shared timer.
- `just smoke-weekly-report` covers the legacy registration entry point, exact-content approval, a loopback Feishu receiver, budget checks and crash reconciliation.
- The [real-model expense-mail acceptance](../../evals/results/recurring-event-live.json) records the natural-language tool proposal with synthetic mail metadata. [Configure an inbound event source](../configuration.md)
