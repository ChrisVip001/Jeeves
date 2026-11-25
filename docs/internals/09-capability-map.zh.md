# ⑨ 能力地图

[English](09-capability-map.md) | 简体中文

Jeeves 的运行时由九个面组成：模型、工具、技能、会话、沙箱、存储、循环、调度和界面。每个面在 Rust 工作区里都有明确归属；本章把它们与代码逐一对应，让改动落到正确的 crate。

## 为什么需要这张地图

Jeeves 坚持本地优先、能力由人类开关、所有外壳共用一个引擎。这里的每一项能力要么是声明式文件（SKILL.md、mcp.json、webhooks.json、schedule），要么是从 `jeeves-tools` 注入、在 `jeeves-app-server` 实现的 Rust trait。聊天消息里的任意代码永远不会被加载执行。

## 九个面

| 面 | Jeeves 归属 | 改动位置 |
|---|---|---|
| 模型 | `jeeves-provider` 目录与 `jeeves-config` 路由 | 档案在 `models.json`；路由在 `<home>/model-routes.json`（运行时文件，不在仓库里）；规则在 `jeeves-config/src/routes.rs` |
| 工具 | `ToolRegistry` 及其 trait | `jeeves-tools/src/lib.rs`；trait 有 `SubagentRunner`、`ApprovalBroker`、`JobRegistry`、`ExecutionPolicy`、`ResourceCoordinator`、`BrowserHost`；实现在 `jeeves-app-server` |
| 技能 | `jeeves-memory` 技能加载器 | 用户技能在 `<home>/skills/<name>/SKILL.md`；随包技能在 `JEEVES_SKILL_DIRS`；解析器在 `jeeves-memory/src/skills.rs` |
| 会话 | `jeeves-state` | `chat_log` 与 `sessions` 表；会话的 `cwd` 同时是唯一可写根 |
| 沙箱 | 工作区策略与能力开关 | `JEEVES_WRITE`、`JEEVES_EXEC`、worktree 隔离和 `ExecutionPolicy::authorize`；没有沙箱就不执行 |
| 存储 | `state.db` 与检查点 | `agent_tasks`、`task_runs`、操作记录、收据、grants，以及 `checkpoints.rs` 里的可恢复工作 |
| 循环 | `jeeves-loop` | 中断点、预算与上下文投影 |
| 调度 | `jeeves-app-server/src/scheduler` | SKILL.md 的 `schedule:` 行或 `schedule_jobs` 表的一行；claim 后未确认就中断的触发留在人工复核 |
| 界面 | 各外壳 | `web/`（驾驶舱）、`desktop/`（Mac 设备端）、`integrations/channels/`（飞书、Telegram） |

后台作业是唯一**不跨 Runtime 重启**的面。持久工作走任务／检查点／移交路径，见[状态](02-state.zh.md)与[应用服务端](06-app-server.zh.md)。

## 改动该落哪里

- 加工具：[工具](04-tools.zh.md)，再读[应用服务端](06-app-server.zh.md)看回合装配。
- 加技能或连接器：`JEEVES_HOME` 下用户自己的文件；随包保持只读。
- 加人工开关：[协议与配置](01-protocol-and-config.zh.md)里的 `Settings` 与 `BOUNDS`。
- 加外壳：[外壳](07-shells.zh.md)；外壳只提交请求、呈现已提交的结果。
- 加验收用例：[质量](08-quality.zh.md)。

发现顺序是 `user > pack > builtin`：用户文件先读、同名时胜出，所以随包配置永远不会悄悄盖掉人的选择。技能加载器、MCP 配置读取器与能力中心用同一顺序。见[配置](../configuration.zh.md#内置-mcp-服务器jeeves_mcp_config)。

## 本章不覆盖什么

这是地图，不是契约。线类型在[协议](../protocol.zh.md)；改它必须跑 `just write-app-server-schema`。本章也不描述「请求 → 目标 → 交付」的产品模型，那在[主入口](../design/main-entry.zh.md)。当前工作区没有通用插件 manifest、加载器或事件总线。
