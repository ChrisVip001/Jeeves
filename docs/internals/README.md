# Internals

English | [简体中文](README.zh.md)

How Jeeves is built, module by module: what each layer does, why it is shaped that way, and where
to change it. Each chapter states only the current implementation; it is not an API reference
(the contract is [protocol](../protocol.md)) and it does not retell control flow the code already
shows. The reader can read Rust and wants to change behaviour, add a tool or a provider, or
understand what one turn does.

## Chapters

Chapter order is dependency order: a chapter builds on the ones before it.

| # | Chapter | Covers |
|---|---|---|
| 1 | [Protocol and config](01-protocol-and-config.md) | the single protocol source and its generation pipeline; `Settings`, `BOUNDS`, the settings patch layer |
| 2 | [State](02-state.md) | `state.db`, migrations, the ledger, facts and search, queues |
| 3 | [Model and loop](03-model-and-loop.md) | providers and wire formats, `resolve`, SSE, the agent loop and its interruption points |
| 4 | [Tools](04-tools.md) | the tool contract, the inventory, per-turn capability filtering, sandboxed exec, approval, the credential vault |
| 5 | [Memory](05-memory.md) | retrieval, consolidation, compaction, skills, the memory bundle and its worker |
| 6 | [App server](06-app-server.md) | the turn pipeline, tasks and interactions, permissions, background jobs, capacity caps, drain |
| 7 | [Shells](07-shells.md) | CLI, the dashboard backend, the web client, gateways, the desktop shell, the browser worker |
| 8 | [Quality](08-quality.md) | eval scenarios and assertions, replay, the judge, smokes, the CI gate and the doc checks |

## Jump table

| To do this | Read |
|---|---|
| add a tool | 4, then 6 for how the table is assembled |
| add a model vendor | 3 |
| change prompts or memory behaviour | 5, then 6 for prompt assembly |
| change the protocol | 1, then 6 for dispatch |
| add a CLI subcommand or a route | 7 |
| add an acceptance case | 8 |

## Other documents

| Document | Home of |
|---|---|
| [architecture](../architecture.md) | principles, crate graph, key mechanisms, the multi-tenancy model |
| [protocol](../protocol.md) | methods, notifications, error codes |
| [configuration](../configuration.md) | every `JEEVES_*` variable |
| [limitations](../limitations.md) | what Jeeves does not do yet |
