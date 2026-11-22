# Jeeves long-task capability roadmap

English | [简体中文](long-task-roadmap.zh.md)

For the Runtime changes that let ongoing work grow out of the main conversation, see the [conversation-to-delivery contract and acceptance](../design/conversation-runtime.md).

This roadmap covers long-running work. An assignment can span reasoning rounds, tools, human decisions and Runtime restarts. A person should be able to amend it, see what remains and verify the outcome from artifacts or external receipts. A personal computer, a self-hosted server and an isolated hosted Runtime all carry the same task contract. [Conversation and delivery](../design/main-entry.md) · [Architecture](../architecture.md)

## Own a recurring assignment

Use a Monday client meeting as the next end-to-end acceptance case. On Mac the person asks, “Give me a one-page brief before every meeting. Focus on unresolved items from last time and keep it between us.” The invitation and prior email are available on Friday. The client changes the agenda over the weekend. On Monday the person has only their phone. Before the meeting they should receive a brief for the revised agenda, be able to trace a claim to its source, and amend it in the original task.

| Capability | Observable outcome |
|---|---|
| Remember a preference and its scope | The second meeting inherits the one-page limit and open items. “Include prices just this time” affects one brief. Each factual claim points back to the calendar or correspondence. |
| Keep working between conversations | A reschedule moves preparation to the new meeting time. Replaying the same event updates the brief once. With sufficient evidence Jeeves delivers directly. If two clients have the same name and the records cannot distinguish them, it asks one disambiguating question. |
| Reach the person when a decision is due | The phone shows the change and outcome and can follow up on the original task. If the person later asks to send the brief, show the recipients and final text, obtain approval and check the delivery receipt. |

This scenario awaits acceptance. Count missed changes, unnecessary interruptions, wrong or duplicate deliveries, and sourced briefs delivered before the meeting. Exercise two meetings against synthetic calendar and mail first, then verify authorized live connections, overnight operation and a physical iPhone separately.

## Measure progress against a verifiable deliverable

Consider an incident review. The person specifies the data scope, anonymization requirements and deliverables. The assistant gathers figures from authorized sources and produces an evidence table and a report. It identifies missing data and carries the original constraints into resumed work. A reviewer can recalculate the figures, trace each conclusion to its source and open the final files. If publication is requested, check the approved version against a receiver-side receipt. A task status, a model's assertion or a tool-call count alone cannot establish that the report is usable.

## Current baseline

The desktop and phone tests start with two independent assignments. A person finds one by title, reads and saves the written report, reloads its draft and asks for a revision from the receipt. The test compares the file text, restarts the isolated Runtime and checks the delivery and file version survive. It also verifies the next approval carries the reviewed request and version. It uses a scripted model and Chrome. Physical phone, live model performance and overnight tests are separate. Run `python3 scripts/smoke-interaction.py --browser chrome` to repeat it. [Browser acceptance](../../scripts/smoke-task-receipt.mjs)

Preference retrieval still has a repeatable gap. After “remember I prefer green tea,” a new session asks “what beverage should you order for me?” The lexical retrieval case remains a known gap. The client meeting scenario should also paraphrase preferences and inspect which record actually reached the assistant. [Memory evaluation](../../evals/deterministic/memory.jsonl)

The Runtime has durable tasks, versioned plans, workspaces, budgets, waits, checkpoint recovery and typed acceptance checks. A main-chat handoff can now carry a named output file and required literal text into that contract. In the scripted incident-review case the worker claims completion without producing the file. The first measurement fails, the worker writes the report on the second round and the Runtime records a passing receipt under the same task ID. Further work covers semantic requirements, research evidence and executable tests through this entry point. [Observed outcomes](../../evals/results/complex-task-handoff-2026-11-21.json) · [Main-chat acceptance test](../../runtime/crates/jeeves-app-server/src/interaction/tests/deliverables.rs) · [Task protocol](../protocol.md)

This iteration fixed the loss of original instructions after failed acceptance. The incident fixture writes figures and an incomplete report, interrupts the correction, reopens the same database and resumes. It checks that the model still sees the original anonymization constraint and that the numeric field and report content pass acceptance. Actual compliance with an anonymization requirement needs separate content evaluation. [Continuity regression](../../evals/deterministic/task-assignment.jsonl)

The November 14 research evaluation with a real model recorded five exact source matches out of six quotations on a repeat attempt. The separate review failed during response decoding. The comprehensive quality check failed. A new research benchmark should grade these two failure modes separately and include previously unseen material. [Recorded research outcomes](../../evals/results/long-research-validation-2026-11-14.json)

## Work order and completion criteria

| Order | Change | Evidence of completion |
|---|---|---|
| 1 · Own a recurring personal assignment | Take on the weekly client brief from main chat using the existing calendar, email, recurring responsibilities and task receipts. | Complete the two meetings above. Record each delivery, change and interruption, and verify that the one-off preference expires before the next meeting. |
| 2 · Form a verifiable task from an assignment | Carry the requested outcome, deliverables, constraints and checks into a durable task from main chat. Reuse the task protocol, typed checks and budgets. The person retains control of goal activation and permissions. | Submit a research assignment and a coding assignment through the main interaction entry. Show the agreed checks in task details. An empty report or failing test keeps the task open. A correction delivers under the same task ID. Ordinary questions still receive direct answers. |
| 3 · Preserve amendments through interruption | Extend the original-assignment regression to cover changed requirements, incoming evidence, budget exhaustion and next-day recovery. | Amend a running task, wait for external input and restart the Runtime. On recovery inspect the amendment, completed steps and remaining budget. Continue the same task, explain exhausted budgets and reconcile unknown writes. |
| 4 · Verify the entire delivery | Trace research to authorized sources, code to a worktree and tests, and delivery to approved content and a receiver. | Run a real model on held-out assignments. Publish results per case with artifacts, citations or tests, failure reasons, elapsed time, usage and interventions. Separately drop the final delivery reply and count effects at the receiver. |
| 5 · Verify equivalent deployments | Run the same revision and acceptance contract in a standalone process, container and isolated hosted instance. Mac, browser and phone clients assign, approve and inspect work. | Change clients and open the same task and artifacts. Recover after worker restart and verify tenant isolation. Record physical-device push, host sleep and real overnight runs separately. |

## Evidence for each iteration

Freeze the input and acceptance criteria. Reproduce the first failing step, fix it and rerun through the same entry point with a repeatable regression. Report independently verified completions, duplicate external effects, recoveries out of attempts, and reasons for human intervention. Record model, deployment, source data and cost. Distinguish simulated time, process restart and actual overnight operation. Run `just check` as the release gate, with `just bench` and `just smoke-recovery` for long-task and recovery observations.
