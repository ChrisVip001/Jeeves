# ⑨ Capability map

English | [简体中文](09-capability-map.zh.md)

Jeeves's runtime is made of nine surfaces: model, tools, skills, sessions, sandbox, storage, loop,
scheduling and UI. Each has a concrete owner in the Rust workspace. This chapter maps them to code
so a change lands in the right crate.

## Why this map exists

Jeeves keeps local-first state, human-controlled switches and one engine behind every shell. A
capability here is either a declarative file (SKILL.md, mcp.json, webhooks.json, a schedule) or a
Rust trait injected from `jeeves-tools` and implemented in `jeeves-app-server`. Nothing is loaded
from a chat message as arbitrary code.

## The nine surfaces

| Surface | Owner in Jeeves | Where to change it |
|---|---|---|
| Model | `jeeves-provider` catalog and `jeeves-config` routing | `models.json` for profiles; `<home>/model-routes.json` for routes (a runtime file, not a repository file); `jeeves-config/src/routes.rs` for the routing rules |
| Tools | `ToolRegistry` and its traits | `jeeves-tools/src/lib.rs`; traits `SubagentRunner`, `ApprovalBroker`, `JobRegistry`, `ExecutionPolicy`, `ResourceCoordinator`, `BrowserHost`; implementations in `jeeves-app-server` |
| Skills | `jeeves-memory` skill loader | `<home>/skills/<name>/SKILL.md` for a user skill; `JEEVES_SKILL_DIRS` for bundled packs; `jeeves-memory/src/skills.rs` for the parser |
| Sessions | `jeeves-state` | `chat_log` and `sessions` tables; a session's `cwd` is also the only writable root |
| Sandbox | workspace policy and capability switches | `JEEVES_WRITE`, `JEEVES_EXEC`, worktree isolation and `ExecutionPolicy::authorize`; no sandbox means no execution |
| Storage | `state.db` and checkpoints | `agent_tasks`, `task_runs`, operations, receipts, grants and `checkpoints.rs` for resumable work |
| Loop | `jeeves-loop` | interruption points, budgets and context projection |
| Scheduling | `jeeves-app-server/src/scheduler` | a `schedule:` line in SKILL.md or a row in `schedule_jobs`; a claim interrupted before confirmation stays for human review |
| UI | Shells | `web/` (dashboard), `desktop/` (Mac device app), `integrations/channels/` (Feishu, Telegram) |

Background jobs are the one surface that does **not** survive a Runtime restart. Durable work uses
the task/checkpoint/handoff path instead; see [state](02-state.md) and
[app server](06-app-server.md).

## Where to change what

- Add a tool: [tools](04-tools.md), then [app server](06-app-server.md) for turn assembly.
- Add a skill or connector: the user's own files under `JEEVES_HOME`; bundled packs stay read-only.
- Add a human switch: `Settings` and `BOUNDS` in [protocol and config](01-protocol-and-config.md).
- Add a shell: [shells](07-shells.md); it submits requests and renders committed results only.
- Add an acceptance case: [quality](08-quality.md).

Discovery order is `user > pack > builtin`: the user's file is read first and wins on a same name,
so an operator pack can never silently shadow a person's own choice. The same order is used by the
skill loader, the MCP config reader and the capability center. See
[configuration](../configuration.md#built-in-mcp-servers-jeeves_mcp_config).

## What this chapter does not cover

This is a map, not the contract. Wire types live in [protocol](../protocol.md); a change to them
requires `just write-app-server-schema`. It does not describe the request → goal → delivery product
model either; that is [main entry](../design/main-entry.md). No generic plugin manifest, loader or
event bus exists in this workspace today.
