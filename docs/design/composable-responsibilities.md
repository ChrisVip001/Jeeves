# Recurring work from the main conversation

English | [简体中文](composable-responsibilities.zh.md)

An owner can describe a recurring private digest in chat, preview actual source items and activate it. A later message can change the time or sources, pause the duty or resume it. The card shows the assumed time, zone, destination and expiry. My Work lists the confirmed duties and their run history. [Runtime protocol](../protocol.md)

## What the owner can compose

A private digest can read a configured calendar, unread mail and open Jeeves tasks. Calendar windows cover today, tomorrow, this week and next week in the agreed time zone. A list or a paragraph per source includes record identifiers. A required read that fails leaves a visible run for review. The model proposes supported fields. Rust validates them, binds the decision to one revision and produces the digest with deterministic reads. Source options, predicates, destination adapters and independently verifiable effects require engineering support. An arbitrary prose request cannot install a tool or enable a connector.

The clock checks eligible minutes while the Runtime is running. The configured morning briefing retains its separate sleep and startup catch-up policy. A late or duplicate decision cannot modify a more recent revision. The local digest and its main-chat delivery record commit together. Every external weekly post still needs the connector's live checks, matching approval and receipt. [Execution and approvals](../protocol.md)

## How the existing duties moved

| Duty | Clock and preserved evidence | How to operate it |
| --- | --- | --- |
| Weekly report | Migration 0008 transfers research task, recipient, slot, body, approval identity and saved outcome to the shared run ledger. Interrupted dispatch remains held for human review. | Review the verified draft, then approve the exact Feishu post. |
| Morning briefing | Migration 0009 introduces clock ownership. An enabled briefing claims its local 08:00 slot and common wake together. Schedule completion and run settlement share a transaction. | Existing settings own the time zone and outbox or notification destination. Use a private digest for other times and source combinations. |
| Price watch | Migration 0010 binds the responsibility to the original watch task. It retains quote evidence, shipping, currency, threshold revisions and the next observation time. The first qualifying observation commits with its chat delivery. | Select the watch in My Work and describe a new limit. A result returned while the limit was being changed cannot trigger the superseded terms. |

## Reproducing the fourth request

The acceptance input asks for next week's meetings and current unfinished tasks, in two sourced paragraphs in this chat every Friday afternoon. Its wording and the scripted model replies live in the test fixtures. No fourth task handler was added.

- Run `just smoke-recurring-work`. Chrome submits the request, previews it twice, activates it, changes it to Thursday with meetings only and pauses it. The fixture runs the same flow through the local gateway and a TLS gateway with a Go control plane. Both use disposable accounts, a loopback calendar, synthetic tasks and a controlled clock. Check the browser-visible history against SQLite. The duplicate Friday and superseded Friday produce no extra delivery. The next Thursday after pausing is silent. A differently worded request previews today's calendar as a list and can be dismissed.
- In each fixture the duty is `planning-digest`. Revisions 1 and 2 produce two distinct confirmed occurrences in total, both delivered locally. Revision 3 is paused. The body cites the calendar UID and start time, and the first revision also cites the open task ID. The preview leaves the clock, grants and outbound receipts untouched. The fixture counts webhook attempts separately.
- The [budgeted real-model conversation](../../evals/results/recurring-work-live.json) records the actual assistant replies, proposed definitions and simulated owner decisions. DeepSeek Flash proposed Friday at 15:00 in Shanghai, explicitly identified the assumed time, reduced the amended output to one calendar paragraph and requested confirmation to pause. This run measures interpretation. The browser fixtures measure delivery and restart behavior.
- Run `just smoke-weekly-report`, `just eval-morning-briefing`, `just smoke-price-watch` and `just check` for the migrated duties and repository gates. The Feishu receiver and quote provider are local test services. Hosted routing was exercised on loopback with a development tenant binding. These checks use isolated state.

## Composing a trigger with an action

The owner can ask for an exact time, a delay after approval, an interval, a calendar schedule or an event from a signed connector. For example, a connector that publishes the sender and attachment media types can trigger a private chat reminder only when both fields match. Its rules and output fields become a reviewed definition. The Runtime can use the same record action for a different connector, fields or values. Event previews use the most recent sample available to the account. They never advance the source cursor or send a message.

The registered action describes its inputs, review fields and effect. A deterministic action runs without a model. A bounded model draft can reference a version of an installed skill captured in the review. The host applies current source and connector policy on each occurrence, keeps the approved skill text fixed for that revision and meters model requests. Feishu posts continue to use an individually approved draft and a verified receipt.

The timer and event paths both record a unique wake before execution. A timer resumes with at most one recent occurrence in a 15-minute catch-up window. A signed source advances its cursor together with its wakes, and duplicate events return an acknowledgement without another execution. An expired lease or an interrupted dispatch becomes a review entry. Morning and price watches retain their compatibility clocks and source-specific delivery receipts, and prepare their results in the common worker. The hosted development smoke exercises account-to-Runtime routing and signed event ingress on loopback.

- Use `just smoke-recurring-work --case event` to reproduce the expense-mail acceptance. A sender mismatch and a MIME-type mismatch are skipped. The matching event delivers one private chat message with the original source reference. A duplicate before and after restart and a new event after pausing add no deliveries. The fixture checks zero model requests while processing events and zero outbound messages.
- The [real-model acceptance](../../evals/results/recurring-event-live.json) records the model choosing the configured event source, converting PDF to the MIME value and producing a pending review. The fixture used synthetic mail metadata.
- See [inbound webhooks](../configuration.md) for operator configuration of a signed event adapter.
