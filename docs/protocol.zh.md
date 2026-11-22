# 协议参考（app-server v2）

[English](protocol.md) | 简体中文

`jeeves app-server` 在 stdin/stdout 上说**换行分隔的 JSON-RPC 2.0**：一行一个
请求，应答与通知混序写出（通知先于应答）。所有类型定义在
`protocol/src/protocol/v2.rs` 和 `protocol/src/rpc.rs`，并生成 TypeScript
（`protocol/schema/typescript/v2/`）。
**改协议必须跑 `just write-app-server-schema`。**

## 方法一览

| 方法 | 请求 → 应答 | 说明 |
|---|---|---|
| `turn/operations` | `TurnOperationsParams` → `TaskOperationsResponse` | 按会话查询普通聊天的未知操作，可限定回合 |
| `turn/reconcile` | `TurnReconcileParams` → `TaskOperationsResponse` | 人工记录确切操作的证据，不执行或继续 |
| `turn/start` | `TurnStartParams` → `TurnStartResponse` | 跑一轮对话；应答在全部通知之后发出。会话忙时再发一轮会被 `SESSION_BUSY` 拒绝 |
| `turn/interrupt` | `TurnInterruptParams` → `TurnInterruptResponse` | 打断在跑的回合；`interrupted:false` = 已经跑完 |
| `turn/steer` | `TurnSteerParams` → `TurnSteerResponse` | 往**正在跑**的回合补一句：下一个轮次边界生效，已跑完的工具不重跑 |
| `interaction/start` | `InteractionStartParams` → `TurnStartResponse` | 主会话的人工入口：路由为普通交流、澄清或独立任务（见[主交互入口](#主交互入口)） |
| `turn/follow-up` | `TurnFollowUpParams` → `TurnFollowUpResponse` | 回合结束后由服务端起下一轮（会话级订阅） |
| `turn/list` | `TurnListParams` → `TurnListResponse` | 正在跑的回合，可按 `sessionId` 过滤 |
| `approval/respond` | `ApprovalRespondParams` → `ApprovalRespondResponse` | 回答一次「要不要执行」；太晚送达时 `accepted: false`。可选的 `actor`（如 `lark:<open_id>`）记入审批记录，不传记为 `owner`。可选的 `version` 必须等于该问题当前的版本，否则返回文本错误（见[重启后的待处理审批](#重启后的待处理审批)） |
| `approval/pending` | `ApprovalPendingParams` → `ApprovalPendingResponse` | 一个会话中实时待确认的审批请求 |
| `approval/log` | `ApprovalLogParams` → `ApprovalLogResponse` | 审批审计：最近 100 条，四态 `approved` / `denied` / `timeout` / `voided` |
| `todo/view` | `SessionViewParams` → `TodoListView` | 当前的待办清单；模型没写过则为 null |
| `task/create` | `TaskCreateParams` → `TaskEnvelope` | 建任务：会话可空，带验收条件、预算、工作目录 |
| `task/start` | `TaskStartParams` → `TaskEnvelope` | 人工授权入队，在独立工作会话中执行 |
| `task/wait` | `TaskWaitParams` → `TaskEnvelope` | 开始前声明一个不可修改的外部输入条件 |
| `task/wake` | `TaskWakeParams` → `TaskWakeResponse` | 渠道入站消息成为正在等待该来源和事件键的任务的输入，每个来源键只投递一次 |
| `task/signal` | `TaskSignalParams` → `TaskSignalResponse` | 经获授权宿主提交一个关联的输入事件 |
| `task/operations` | `TaskGetParams` → `TaskOperationsResponse` | 任务的操作日志（见[操作日志与人工核对](#操作日志与人工核对)） |
| `task/reconcile` | `TaskReconcileParams` → `TaskOperationsResponse` | 人工为一个未决操作提交证据 |
| `task/list` | `{cursor?}` → `TaskListResponse` | 新的在前，每页最多 100 项。用返回的 `nextCursor` 读取较早的任务 |
| `task/view` | `SessionViewParams` → `TaskViewResponse` | 会话当前的任务卡 |
| `task/events` | `TaskEventsParams` → `TaskEventsResponse` | 游标之后的任务事件，从旧到新（见[任务事件游标](#任务事件游标)） |
| `capability/list` | — → `CapabilityListResponse` | 每个模型 profile、MCP 服务器和 connector 一张卡（见[能力卡](#能力卡)） |
| `capability/probe` | `CapabilityProbeParams` → `CapabilityProbeResponse` | 按 id 重新检查一张卡 |
| `connector/tools` / `connector/tools/set` | `CapabilityProbeParams` / `ConnectorToolsSetParams` → `ConnectorToolsResponse` | 查看工具或人工修改用户 MCP 连接的工具开关；HTTP 对应 `GET /api/connector/tools?id=…` 和 `POST /api/connector/tools/set` |
| `connector/catalog` | — → `ConnectorCatalogResponse` | 精选连接器、配置方式、文档和清单，亦见已鉴权的 `GET /api/connector/catalog` |
| `connector/settings` | — → `ConnectorSettingsResponse` | 人工查看邮件/日历配置，仅提供凭据引用 |
| `connector/settings/save` | `ConnectorSettingsSaveParams` → `ConnectorSettingsResponse` | 完整校验并原子保存且仅保存一个段落 |
| `connector/oauth/start` | `ConnectorOAuthStartParams` → `ConnectorOAuthStartResponse` | 准备真实回环 OAuth 授权链接 |
| `task/integrate` | `TaskIntegrateParams` → `TaskGitCheckpointResponse` | 人工确认子任务提交，合入待启动父任务的工作目录 |
| `task/gitCheckpoint` | `TaskGitCheckpointParams` → `TaskGitCheckpointResponse` | 人工确认后将指定文件提交到非运行中的任务 worktree |
| `task/gitCheckpointPolicy` | `TaskGetParams` → `TaskGitCheckpointPolicyResponse` | 查看任务自动检查点授权状态 |
| `task/gitCheckpointPolicy/set` | `TaskGitCheckpointPolicyParams` → `TaskGitCheckpointPolicyResponse` | 人工确认自动检查点的具体文件或关闭策略 |
| `task/get` | `TaskGetParams` → `TaskEnvelope` | 单个任务：验收、预算、花费、执行次数 |
| `task/runs` | `TaskGetParams` → `TaskRunsResponse` | 执行记录：每次的开始、结束与停下的原因 |
| `task/artifacts` | `TaskGetParams` → `TaskArtifactsResponse` | 交付物清单（真实写出来的文件） |
| `task/reviewCapture` | `TaskReviewCaptureParams` → `TaskReviewView` | 为已完成的旧交付绑定两个明确的 Git 提交 |
| `task/suspend` | `TaskActionParams` → `TaskEnvelope` | 暂停：保留断点，不丢已完成的工具 |
| `task/resume` | `TaskActionParams` → `TaskEnvelope` | 继续：从断点接着跑 |
| `task/cancel` | `TaskActionParams` → `TaskEnvelope` | 取消（终态，不再自动续） |
| `session/permission/get` | `SessionPermissionGetParams` → `SessionPermissionResponse` | 当前会话策略、版本与能力开关；`mode:null` 保留旧运行时规则 |
| `session/capabilities` | `SessionCapabilitiesParams` → `SessionCapabilitiesResponse` | 外壳上报平台、传输方式与能力名；引擎返回 `accepted` / `unsupported` |
| `session/permission/set` | `SessionPermissionSetParams` → `SessionPermissionResponse` | 人工修改会话策略；`expectedRevision` 必须匹配，提升完全访问须显式确认 |
| `session/main` | 无 → `SessionNewResponse` | 幂等打开常驻的 `jeeves-main` 会话 |
| `session/list` | `SessionListParams` → `SessionListResponse` | 唯一公开会话 |
| `session/get` | `SessionGetParams` → `SessionGetResponse` | 单个会话的元数据，与 `session/list` 共用 [SessionSummary](../protocol/schema/typescript/v2/SessionSummary.ts) |
| `session/update` | `SessionUpdateParams` → `SessionUpdateResponse` | 改名 / 改工作目录（`null` = 不动，`""` = 清空） |
| `session/messages` | `SessionMessagesParams` → `SessionMessagesResponse` | 会话消息按从新到旧分页。任务最新页的可选字段 `activeInputs` 列出尚未归入历史的本轮指令与已接收的补充 |
| `memory/search` | `MemorySearchParams` → `MemorySearchResponse` | facts + episodes 关键词检索 |
| `memory/list` | `MemoryListParams` → `MemoryListResponse` | 最近事实（游标分页） |
| `memory/list-episodes` | `MemoryListEpisodesParams` → `MemoryListEpisodesResponse` | 最近情景 |
| `memory/setValidity` | `MemoryValidityParams` → `MemoryValidityResponse` | 人工替换事实有效区间 |
| `memory/provenance` | `MemoryProvenanceParams` → `MemoryProvenanceResponse` | 查看有效期与提炼上下文 |
| `memory/remember` | `MemoryRememberParams` → `MemoryRememberResponse` | 写一条事实 |
| `memory/forget` | `MemoryForgetParams` → `MemoryForgetResponse` | 按主题删除 |
| `config/read` | `ConfigReadParams` → `ConfigReadResponse` | 完整配置视图 |
| `config/write` | `ConfigWriteParams` → `ConfigWriteResponse` | 应用补丁并热生效（落 settings.json） |
| `model/list` | `ModelListParams` → `ModelListResponse` | 模型目录 |
| `model/profiles` | `ModelProfilesParams` → `ModelProfilesResponse` | 已配置的多套模型 + 当前那套（**不含密钥**，只有「key 在哪」的指针） |
| `model/save` | `ModelSaveParams` → `ModelSaveResponse` | 新增/改一套并切成当前；只带 `id` + `makeActive` 就是切换 |
| `model/remove` | `ModelRemoveParams` → `ModelRemoveResponse` | 删一套，连它的凭据一起 |
| `model/routes` | — → `ModelRoutesResponse` | 路由表：`fixed` / `auto` + 三行「档位 → 配置 id」（没配的档位是 `null`，表示跟随缺省链） |
| `model/routes/save` | `ModelRoutesSaveParams` → `ModelRoutesResponse` | 整表替换；档位名与 profile id 都在写入前校验，拼错直接报错 |
| `usage/summary` | `UsageSummaryParams` → `UsageSummaryResponse` | 用量台账：per-model 聚合 + 总数。**只记账不强制** —— 没有配额、没有限速 |
| `responsibility/reports` | `ResponsibilityReportsParams` → `ResponsibilityReportsResponse` | 最近 50 次周报、投递状态和待答审批 |
| `responsibility/prepare` | `ResponsibilityContract` → `ResponsibilityContract` | 校验并规范化待核对的委托定义 |
| `responsibility/sample` | `ResponsibilityContract` → string | 按当前当地日期预览带来源的私人摘要 |
| `responsibility/amend` | `ResponsibilityAmendParams` → `ResponsibilityView` | 按期望版本修改委托，未结唤醒转入核对 |
| `responsibility/register` | `ResponsibilityContract` → `ResponsibilityView` | 由人登记周期性责任 |
| `responsibility/list` | `{}` → `ResponsibilityListResponse` | 列出契约、状态与上次认领的分钟 |
| `responsibility/setPaused` | `ResponsibilitySetPausedParams` → `ResponsibilityView` | 暂停或恢复尚未到期的责任 |
| `schedule/list` | `ScheduleListParams` → `ScheduleListResponse` | 全部已声明的任务（存储的与 skill 自带的），含运行状态与下次运行时间 |
| `schedule/save` | `ScheduleSaveParams` → `ScheduleChangeResponse` | 新增或替换一个存储的任务（会校验；skill 自带的名字会被拒绝） |
| `schedule/remove` | `ScheduleRemoveParams` → `ScheduleChangeResponse` | 删除一个存储的任务（skill 自带的会被拒绝） |
| `browser/list` | `BrowserListParams` → `BrowserListResponse` | 浏览器可用性及所属浏览器会话（仅人工） |
| `browser/start` | `BrowserStartParams` → `BrowserSessionView` | 启动任务浏览器（仅人工） |
| `browser/control` | `BrowserControlParams` → `BrowserSessionView` | 携带预期代际控制浏览器（仅人工） |
| `browser/frame` | `TaskGetParams` → `BrowserFrameResponse` | 一帧限长、按需的画面（仅人工） |
| `memory/ledger` | `MemoryLedgerParams` → `MemoryLedgerResponse` | 一个会话的账本，最新在前；带 `turnId` 时连同那一轮的条目一起给。只读 |
| `memory/index` | — → `MemoryIndexResponse` | bundle 的目录（`index.md`）以及每页一句话摘要。只读 |
| `memory/page` | `MemoryPageParams` → `MemoryPageResponse` | 一页的正文与 `sources`；没有这一页时是 `page: null` |
| `context/usage` | `ContextUsageParams` → `ContextUsageResponse` | 各段上下文占用明细；`usage: null` = 还没测量过 |
| `skill/stats` | — → `SkillStatsResponse` | 汇总 `<home>/skill-hits.jsonl`。只读 |
| `session/compact` | `SessionCompactParams` → `SessionCompactResponse` | **现在**把较早轮次折进滚动摘要。只有人能发起，没有任何工具能走到它 |

约定：方法名 `<resource>/<method>`（resource 单数）；载荷命名
`*Params` / `*Response` / `*Notification`；wire 字段 camelCase；整数一律
`i32`（`i64` 会被 ts-rs 映射成 `bigint`，与 `JSON.parse` 的 `number`
运行时不符）；分页列表方法（`memory/list`、`session/list`、`session/messages`）用 `cursor`/`limit`，返回 `data` + `nextCursor`。

### 目标循环的用法

目标是澄清后的结果，不一定是持久对象。在对话内完成的目标不会进入本协议；只有需要持久任务线程的目标，才存为任务行的 `goalCondition`，所以协议里没有
`goal/set` / `goal/view` —— 人要设目标就走 `/goal` 或 `task/create {goalCondition}`，
循环读同一行。唯一的协议面是通知 `GoalRound`（每判一次发一条）：

```
you>  /goal 让测试全绿
       → task/create  {sessionId: "…", title: "让测试全绿", goalCondition: "让测试全绿"}
… 一轮跑完，判断器说不算达成 …
       ← GoalRound {turnId, round: 1, maxRounds: 5, status: "continuing", reason: "还有两条红"}
… 又跑一轮 …
       ← GoalRound {turnId, round: 2, maxRounds: 5, status: "satisfied", reason: "全绿了"}
```

`TurnMeta.goalStatus` / `goalRounds` 把出口与轮次也带进这一轮的元数据。
`TurnMeta.routeReason`（可选）说明这一轮的模型是怎么选的：`auto:reasoning → ds-r1`
或 `fallback: …`；没发生选择时为空。

`TurnStartParams.requestId`（可选，1-120 个 `[A-Za-z0-9_-]` 字符）标明这一轮回答的是哪条消息。同一会话里用同一个 id 再发 `turn/start`，会原样重放第一次那一轮的通知和应答，不再调用模型；同一个 id 配不同的消息或参数会被拒绝。回执存在 `interaction_receipts` 表里（键带 `turn:` 前缀），重启后依然有效。不带 `requestId` 时行为不变。

`task/artifacts` 的 `latestDelivery` 返回最近一次结束的执行、结果状态、记录时间与摘要。新一轮运行中仍可查看上次回执。`latestDelivery` 与对应的 `session/messages` 完成通知通过 `approvalDecisions` 按决定顺序列出该次执行的请求编号、工具和审批结果。空列表表示本轮没有已记录的决定，字段缺失表示无法关联到具体执行。任务原回复可单独查看。文件的可选 `writtenText` 是写入时记录的完整文本，局部编辑会清除旧文本。

Git 隔离工作树任务在创建时记录基线提交，每次交付结束时保存相对基线的文件快照。旧交付可通过 `task/reviewCapture` 指定任务仓库中的两个完整提交 ID。受文本与快照体积上限影响的文件可能只有路径和变更类型。

## turn/start 的通知序列

```
turnStarted → retry* → textDelta* → toolStarted → toolDelta* → toolCompleted*
            → consolidationCompleted? → turnCompleted →（应答）
```

重试可能出现在任何一次模型调用之前（loop、压缩摘要都算），所以它在序列里
是**可插入**的，位置不固定。`ServerNotification` 的全部变体（判别式 `type`）：`permissionChanged`、`turnStarted`、`textDelta`、`retry`、`approvalRequested`、`goalRound`、`toolStarted`、`toolDelta`、`toolCompleted`、`consolidationCompleted`、`turnCheckpointed`、`turnSteered`、`turnResumed`、`turnSuspended`、`taskUpdated`、`turnCompleted`、`error`。

* `retry`：一次重试（第几次、为什么、等多久）。**每次必发** —— 重试绝不
  静默；`meta.retries` 记着总数。
* `approvalRequested`：有一条命令在等人批准（工具、命令原文、拒因、时限）。
  收到之后要调 `approval/respond`，那一轮才继续；**不回答就是拒绝**。
* `textDelta`：流式文本增量；仅 `stream:true` 时产生。
* `toolDelta`：工具的实时输出片段；是进度，不是结果。
* `cwd`（`TurnStartParams`，可选）：本轮工作目录。必须是已存在的绝对路径（fail-closed）；`run_command` 的回退链是 `args.cwd > cwd > 进程 cwd`，`read_file`/`glob`/`grep 的相对路径也按它解析（会话级 working-directory）。子代理继承父轮的 cwd。
* `toolStarted` / `toolCompleted`：工具开始与完成（含耗时与状态）。
* `turnCompleted`：`reply`、`iterations`、`usage`、`meta`（`TurnMeta`：tools、model、latency、interrupted、retries、目标状态、路由原因等）。
  meta 随对话落库。

### `Fact.kind`

记忆的类别：`user` / `feedback` / `project` / `reference` / `fact`（兜底）。
`memory/search`、`memory/list` 的应答与 `turn/*` 的检索上下文都带着它；
`memory/remember` 可以传，写歪了一律收敛成 `fact`。

## 错误码

| 码 | 含义 |
|---|---|
| -32700 / -32600 / -32601 / -32602 / -32603 | 标准 JSON-RPC：解析 / 请求 / 方法不存在 / 参数 / 内部错误 |
| `-32000` PROVIDER_ERROR | 模型侧失败（缺 key、401、限流……），附「怎么办」 |
| `-32001` TOOL_ERROR | 工具执行失败（错误同时作为文本回给模型，不中断 turn） |
| `-32002` NOT_IMPLEMENTED | 协议定义了但未实现；与方法不存在区分开 |
| `-32003` SESSION_BUSY | 每个会话同一时刻只跑一轮；消息里写明占着会话的那一轮 |

## 客户端

* TypeScript：`integrations/channels/runtime-client`（`StdioTransport` + `JeevesClient`，
  通知按判别式 `type` 分发）。
* 终端 REPL 在进程内走同一套方法；驾驶舱通过 HTTP 接入（路由见 `jeeves-ops/src/lib.rs`）。

## 独立任务执行

`task/create` 只记录任务。人可以调用 `task/start {taskId, message}` 或点击驾驶舱的「开始任务」，授权首次执行。message 是实际指令，驾驶舱使用任务标题与目标。SQLite 持久队列由一个 worker 消费。创建本身不启动工作，模型没有开始任务的工具。

`AgentTaskView.executionSessionId` 指向独立执行记录；`sessionId` 仍是原交流会话。`dispatchState` 为 pending / claimed / finished / interrupted，只描述首次派发，不代表任务最终结果。暂停与等待作业仍使用原任务状态。原聊天可独立继续，并携带有上限、限定本会话的任务进度；不复制工作对话。没有验收条件的产出在进度中标为尚未核验。

每个任务只接受一次首次派发，重复开始会拒绝。待执行记录跨重启保留；已领取工作由 checkpoint 恢复负责，没有 checkpoint 的领取进入待核对，不重新提交。排队期间暂停后必须人工恢复，取消则不执行。`POST /api/task` 支持 `action: "start"` 与同样参数；TypeScript 客户端提供 `TASK_START`。

这是本地 worker，不是全天在线服务或分布式租约系统；app-server 运行且模型已配置时才工作。不自动接收外部回调，不授予额外能力。其他手动回合沿用原并发规则；多个操作修改同一外部资源仍需协调。

## 任务事件游标

离线过的客户端用 `task/events { cursor?, taskId?, limit? }` 回放任务时间线。事件从旧到新返回，`limit` 最大 500
（默认 100）。响应带 `nextCursor`、`hasMore` 和 `latestCursor`；把 `nextCursor` 传回去即可续读。游标为 0（或不传）
从头开始；游标越过末尾时返回空页并保持原游标。任务内部的顺序看每条事件的 `seq`。

`/api/events` 仍然只是敲门，但与任务相关的 ping 现在带 `cursor`，即最新的事件游标。客户端与自己的游标比较，
落后时才调用 `task/events`。

驾驶舱通过 `GET /api/task/events?cursor=&limit=` 读取同一页。每个标签页在内存里记住上次游标，重连、收到敲门和 5 秒轮询时从游标拉取并追加；只有服务端最新游标落后于标签页的游标（日志被替换）时才从头开始并重载快照。`@jeeves/client` 以 `TaskEventFollower` 提供同样的逻辑。

## 能力卡

`capability/list` 为每个已配置的模型 profile（`model:<id>`）、MCP 服务器（`mcp:<name>`）和飞书 connector
（`connector:feishu`）返回一张 `CapabilityCard`：`kind`、`title`、`identity?`、`toolSource`、`state`、`detail?`、
`secretPointer?`、`lastCheckedAt?`。`state` 取 `ready`、`unauthorized`、`expired`、`missingScope`、
`blockedByPolicy`、`offline`、`unknown`。卡片从不携带密钥，只给 `secretPointer`（`env:NAME`、`keychain`、`file`、`memory` 或未解析的 `vault`；
MCP OAuth token 报 `file`）。飞书收件人白名单为空时报 `blockedByPolicy`；MCP 连接阶段明确返回 OAuth 权限不足才报 `missingScope`。探测按需进行，每张卡在内存里缓存 60 秒；`capability/probe { id }` 从该缓存回答，
未知 id 会被拒绝。

驾驶舱通过已鉴权的 `GET /api/capability` 和请求体为 `{id}` 的 `POST /api/capability/probe` 转发相同的响应。

已鉴权的 `GET /api/connector/settings`、`POST /api/connector/settings/save`、`POST /api/connector/oauth/start` 转发仅供人工使用的连接器方法。OAuth 仅在绑定本地回调后返回 `authorizationUrl`；用户须在 Runtime 设备打开链接。凭据存入现有凭据库，绝不返回 token。只有已配置且启用 OAuth 的 MCP 服务器及飞书用户身份支持该流程。

拆分编码任务时，先创建带 `repo` 和最终命令检查的父任务。父任务待启动期间，逐个调用 `task/create`，传入 `parentTaskId` 和子任务标题。每个子任务都从父任务已提交的 HEAD 建立自己的分支，拥有独立任务账本。通过 `task/start` 启动子任务后，不同工作目录可以并行写入。已获授权的编码 CLI 可通过 `run_command` 的后台选项运行。任务产物包含日志，以及有大小上限的跟踪文件差异和未跟踪文件名。执行器失败或中断时需要审阅。

子任务完成后，为选定文件创建检查点，通过 `POST /api/task/integrate` 或 `task/integrate {parentTaskId,childTaskId,commit,confirmed:true}` 逐个确认子任务的准确 SHA。遇到冲突，由人在父任务工作目录中合入该提交，再确认同一 SHA。全部合入后启动父任务，用它自己的验收条件检查汇总结果。

`POST /api/task/git-checkpoint` 转发 `task/gitCheckpoint {taskId, paths, confirmed}`。需 `confirmed: true`、显式相对文件路径且符合预期的非运行中任务独立 worktree 和分支。路径按字面匹配，包括非 ASCII 文件名和已选跟踪文件的删除。将提交 SHA 记为 `git_checkpoint` 事件供交接重建。已有暂存改动时拒绝，不推送、不暂存未选择的文件、不改写历史。自动 Git 检查点默认关闭。`GET /api/task/git-checkpoint-policy?taskId=…` 查询策略；停止任务后，`POST /api/task/git-checkpoint-policy {taskId,paths,enabled,confirmed}` 人工启用或关闭。启用后每个持久执行断点仅将已选文件映射到提交（内容未变则沿用 HEAD），检查点失败时停止执行待审。不要选择含密钥的文件。Git 历史在任务本地分支。

## 持久化外部输入

两组保留的来源/事件键允许已有任务启用只读轮询：`connector:email` / `new-unread` 和 `connector:calendar` / `upcoming-15m`。仅轮询人工声明且已暂停的输入等待（每分钟一次，最多两个并发轮询）。邮件首次成功轮询捕获 UIDVALIDITY 及 UIDNEXT 并跨重启保留基线，只有不小于基线的未读 UID 可唤醒。日历选择 15 分钟内开始的非全天日程。账号身份或邮箱世代变化时保留等待待核对。输入信号仅包含条目身份和时间元数据，不包含邮件正文，也不会创建新任务。

任务开始前，用户或获授权的宿主可调用 `task/wait {taskId, source, eventKey, description, expiresAt?}` 声明一个不可修改的输入条件。每个任务限一个条件；`expiresAt` 是 RFC 3339 格式的输入到达期限，按时接收的数据可以在期限后处理。任务可保留草稿并暂停等待，不消耗额外模型轮次，陪伴聊天保持独立。

通过 `task/signal {conditionId, source, eventKey, eventId, data}` 提交不超过 16 KiB 的 JSON 数据。来源与事件键必须匹配；相同来源、事件 ID、目标和数据的重复提交返回原事件确认，修改目标或内容会被拒绝。这些字段仅用于关联，不提供发送者身份认证；必须使用现有的鉴权宿主传输，不可直接暴露为公开 webhook 或模型工具。

飞书网关先将每片入站事件提交到按账号隔离的本地 SQLite 收件箱，再向飞书确认接收。写入失败时保留未确认状态。重启后，完整事件使用原消息 ID 继续处理。处理完毕后只保留去重键，删除内容。

`task/wake {sourceKey, source, eventKey, data}` 是面向渠道的形式。`sourceKey` 为 `<渠道>:<消息ID>`。若有任务正好在等待这个 `source` 和 `eventKey`（飞书为 `lark` 与群聊 ID），消息经与 `task/signal` 相同的路径投递，响应 `woken: true`。重复的 `sourceKey` 返回 `duplicate: true`，不重复投递，并记录 `duplicate wake skipped`。没有任务在等待时，`woken` 与 `duplicate` 都为 false，调用方按普通消息处理。飞书网关先把每条通过检查的文本消息交给 `task/wake`，调用失败时退回普通回合。

事件提交后仅唤醒准确绑定的等待执行一次，重启后也可恢复。检查点保留完整会话和草稿；输入明确作为不可信数据，不增加权限。接收输入不代表完成，恢复后仍须满足原预算和验收条件。人工暂停和取消优先；人工暂停期间到达的数据会保存，直到明确调用 `task/resume`。继续请求等待旧执行收尾后，将人工授权、任务激活和检查点重新打开作为同一事务提交。

## 操作日志与人工核对

`task/operations {taskId}` 返回持久化操作身份、参数和结果摘要、执行意图或已观察结果、人工证据及当前检查点摘要。本地操作键不是服务端幂等键。自动重放策略为 `never`；存在意图只说明操作可能执行过，也可能尚未执行。

人工停止任务并检查真实外部状态后，可调用 `task/reconcile {taskId, operationId, checkpointDigest, resolution, evidence}`。结果限 `reconciled_success`、`reconciled_failure` 或 `not_executed`，证据为 1–16384 字节。必须匹配最新的已暂停执行，以及确切的未决调用和参数。证据与明确标记的工具结果回执和检查点原子提交。相同证据可重复确认，冲突结算会被拒绝。

核对不会执行工具、清除人工暂停或恢复任务。全部未决调用核对后仍须明确调用 `task/resume`。人工确认不是实际观察到的工具成功返回，不满足 `toolsOk`；原产物验收和预算仍适用。近期操作证据会独立于滚动摘要重建到持久化交接中。这些方法是鉴权的人工或宿主控制入口，不是模型工具。

`ToolOperationView.status` 区分准备分发、已执行、经人工核对、未执行和历史证据不足。旧记录只有成功或失败标签、没有执行器输出预览时，返回 `historical_unverified`。界面可展示原摘要；验收和自动恢复不以该标签为成功证据。人工核对只接受 `reconciled_success`、`reconciled_failure` 或 `not_executed`。

`ToolOperationView.origin` 区分与执行检查点共同提交的意图（`checkpoint`）和从未变化、已停止的旧检查点重建的未决元数据（`recovered_checkpoint`）。两者都不证明已经发送。人工暂停会将任务围栏、持久等待取消及可恢复检查点暂停作为同一事务提交；提交失败保留原等待。

## 版本化验收证据

`CheckView.artifactSha256`（如有）标识文件验收实际读取的确切字节。文件存在检查流式读取普通文件；子串检查在同一次稳定文本读取上匹配并计算摘要。读取期间变化或无法读取的文件不能通过检查。历史摘要不表示该路径现在仍包含相同内容。

非空验收结果与不可修改的 task/run/round 验收回执、文件清单及当前检查投影共同提交。证据提交失败会阻止完成并等待人工确认。内容变化生成关联的新回执，包括 A→B→A 的回退；紧邻的相同测量复用最新回执。回执完整性覆盖测量、观察时间及前序 ID。成功任务的完成回执绑定对应确切 run 的验收 ID。不生成归档文件副本。

仅由模型判断目标达成可以停止目标循环，但不构成独立验收。没有验收条件时，即使存在目标条件，完成回执和聊天进度仍使用 `produced_unverified`，驾驶舱也相应标明尚未核验。

## 记忆有效期与提炼来源

人工客户端通过 `memory/setValidity`（`MemoryValidityParams` → `MemoryValidityResponse`）
替换事实的完整有效区间。传入 `factId`、`validFrom` 和 `validUntil`；时间使用 RFC3339，
null 清除对应边界，结束必须晚于开始。开始包含、结束不包含。这些控制不作为模型工具。
`memory/provenance`（`MemoryProvenanceParams` → `MemoryProvenanceResponse`）读取有效区间和
已记录的提炼上下文；保留事实 ID 时也可查看已失效事实的这些元数据。

数据库保存实际领取的聊天行 ID，以及角色/内容的 SHA-256，作为提炼输入上下文。
这标识模型看到的整批输入，不证明哪句话支持该事实。同一上下文重试不会增加确认次数。
更正清除旧声明的上下文；删除级联删除上下文。旧事实可能没有这些记录。

召回从关键词、向量和最近事实召回中排除失效事实。有时间边界的事实即便当前有效，
也不写入无法自行过期的静态镜像。提示词目录与 `read_page` 在读取时检查数据库来源；
任一来源已删除、更正或存在时间边界，整份快照退出自动召回。没有事实引用的人工页面保持独立。
现有文件、聊天历史、事件摘要、断点与备份不会被擦除或追溯脱敏。有效期过滤针对事实，不覆盖事件摘要。
后台提炼还会记录绑定原始消息的临时观察。提炼合同将长期 `facts` 与可选的
`states` 分开。每个状态带有确切 `sourceRef`、4–400 字符的用户原文片段、[0,1] 范围内的
模型自报置信度和 1–24 小时有效期。引用必须属于本次领取的输入，且与现存用户消息的
SHA-256 一致；助手文本或不匹配的引文被拒绝。到期时间从原消息计算，不从提炼或重试
时刻计算。来源已过期则不产生临时事实。事实、有效期与评估记录原子提交。

`memory/provenance` 另返回可选的 `validityOrigin`（`human` 或 `consolidation`）以及每次观察
可选的 `assessmentJson`，包含原文、来源、置信度与按来源时间计算的有效期。置信度是
未经校准的模型判断；引文匹配不证明其解释正确，也不证明某人的当前情绪。相同观察
重试不会续期。人工内容/有效期修改和删除优先；历史无期限声明不会自动重新分类。
这些元数据不增加模型控制方法。

召回将有期限声明标为临时，将事件摘要标为历史总结。事实有效期不擦除原始历史，也不让
事件摘要过期。模型分类错误、语义改写、显式笔记工具及历史无期限情绪不在自动 TTL 保证内。
提炼结构不合法时保留来源记录并退避重试，不静默标为已处理。提炼指令与 JSON 交流数据
使用不同的消息角色。

提炼区分已领取批次与已完成记录。独占运行时启动只释放有持久领取标记的未完成批次。
即使新聊天到来或新任务阈值改变，重试仍保留原始来源集合；新批次最多 4,096 行。退避更新
在同一事务中提交。按来源上下文提交的回执防止相同批次重复插入或替换事件摘要；人工删除
摘要后回执作为墓碑保留。已完成记录与没有领取元数据的旧记录不会重新打开。

记忆 worker 存活时保留数据库执行所有权。正常关闭先短暂等待，再取消并等待停滞的提炼调用
退出；丢弃 worker 会中止其循环。恢复提炼可能再次调用只读模型并产生用量，不会重放任务工具。
未收到的供应商用量仍不能精确计入任务账目。

## 结构化文档验收

`jsonEquals` 将解析后的 JSON 值与人工固定的交付合同比较，不再靠正文里是否出现数字判断。
验收条件示例：

```json
{"kind":"jsonEquals","path":"report.json","pointer":"/totals/approved","expectedJson":"42"}
```

`pointer` 遵循 RFC6901（`~1` 表示斜杠，`~0` 表示波浪号）；空指针选择整个文档。
`expectedJson` 是编码后的 JSON 值，最多 16,384 个 UTF-8 字节，因此 `"null"` 与未声明期望值
可以区分。缺失字段不等于 null，字符串不等于数字。按解析后 JSON 值比较，不提供财务十进制运算。
文档必须是任务工作区内、不超过 16 MiB 的常规 UTF-8 JSON 文件。无效指针、期望值、文档、
路径或不可用的测量均不通过验收。

## 回执与命令验收

“工具说发了”和“读回看到了”是两件事。`receipts`（迁移 0004）每个效果一行，状态为 `claimed`（工具报告成功）、
`confirmed`（读回确认）或 `failed`。两种验收用到它们：

```json
{"kind":"receiptConfirmed","receiptKind":"feishu_send","refPrefix":"om_"}
{"kind":"commandSucceeds","argv":["cargo","test"],"cwd":"."}
{"kind":"sourceSupports","path":"claims.json"}
```

`sourceSupports` 读取 `path` 指向的数组，元素为 `{claimId, text, sourceRefs, quote}`。在任务里，`fetch_url` 把返回的内容存到 `sources/`，并在 `sources/index.jsonl` 登记（`url`、`fetchedAt`、`sha256`、`path`）。搜索线索与查询单独登记在 `sources/search/index.jsonl`，不能通过此验收。只有每条主张都引用了已保存的快照、其 `quote` 在被引快照里逐字出现、且 `text` 里每个数字都原样出现在 `quote` 里，才通过。模型写的摘要永远不算证据。

`receiptConfirmed` 只有在存在该类型、ref 以 `refPrefix` 开头且状态为 `confirmed` 的回执时才通过；只有 `claimed`
则不通过，详情会写明。`commandSucceeds` 在任务目录或其子目录里运行 `argv`（参数列表，逐项加引号，绝不是 shell 字符串），
沿用 `run_command` 的会话权限、命令放行规则、沙箱和网络设置。当前策略要求审批时，会向用户展示本次命令。退出码 0 通过，其他退出码
不通过并附输出末尾。exec 关闭、没有沙箱、命令被拒绝或结果未知时，检查不通过，详情以 `skipped:` 开头（如
`skipped: exec off`）。跳过从不算通过。输出保存在任务目录的 `checks/` 下。任一条件缺字段会报错并带该条件的序号。

比较与 SHA-256 测量使用同一份捕获字节。既有验证回执把该版本绑定到任务、run 与合同；
多个检查观察到互相矛盾的文件版本时，不能提交成功证据。它证明文档符合给定值，
不证明外部来源真实或远端事务已成功。

## 普通聊天操作核对

使用会话顶栏的「核对聊天操作」，或人工专用 `turn/operations` / `turn/reconcile`。
查询限定在 `sessionId`，可选 `turnId` 指定已停止的普通聊天检查点。属于任务的操作继续使用
`task/operations` / `task/reconcile`。跨多个回合查询时，每个操作携带自己的检查点摘要；
客户端必须提交选中操作对应的摘要。

同一会话的新循环不能绕过此前未知的外部结果：仅保留已知读取工具，并带有限的宿主台账说明。
覆盖没有操作元数据的旧检查点，以及启动期间尚未被领取的 open 边界。人工查询会先停止无人
执行且结果不明的边界，再重建有来源标记的元数据；这不证明请求已经发出。旧消息损坏时仍拒绝修改。

证据不可变、限定确切身份，并与检查点原子提交。同一结果和证据可重复确认；过期摘要或冲突结果
被拒绝。旧回合保持停止。核对后请在当前工作目录下发送新指令；此入口不会恢复旧工作目录或权限。
人工观察不计为实际工具成功。不同会话和非协作外部进程仍没有全局外部副作用去重保证。

### 停止后的明确继续

`turn/interrupt` 停止执行，检查点不会自动恢复。人工可对结果已知、属于任务最新执行的停止检查点明确调用 `task/resume`；授权、任务激活与检查点重新打开原子提交。已取消任务、过期执行身份或未核对的未知工具结果仍不能因此恢复。

## 会话权限

`readOnly` 放行已审查的内置读取工具和所有者在[连接器清单](configuration.zh.md#连接器清单)中声明为读取的工具。`standard` 的副作用工具须经当前回合真实审批，批准仅用于该次调用。两者禁用委派与会话生命周期/工具 hooks，避免通过子执行绕过审批。`fullAccess` 允许在已启用能力范围内自动执行，仍保留硬性拒绝、允许目录和沙箱等执行层规则，不会开启 `JEEVES_EXEC` / `JEEVES_WRITE` 等开关。

`session/permission/set` 先持久化再生效，成功后通知 `permissionChanged`。旧版本、无效会话和未确认的提升被拒绝；权限存储不可读时工具执行失败关闭。审批之后、资源等待之后和实际调用之前复核版本；切换权限使待审批动作失效，已经开始的操作不做回滚保证。此接口仅供可信宿主调用，没有模型工具。主会话以 `standard` 模式起步；未设置预设的会话继续旧运行时规则（`mode:null`）。

权限预设约束模型工具与会话 hooks；对话账本、使用量记录和既有自动记忆整理仍属于 Runtime 的内部持久化，不意味着应用以只读方式打开数据目录。协议入口为 stdio；ops 驾驶舱通过鉴权 HTTP 转发同一组权限/配置/模型路由（`/api/session/permission`、`/api/config`、`/api/model/catalog`）。

## 能力协商

新的外壳在连接时调用一次 `session/capabilities`。`protocolVersion` 只在破坏性协议变更时递增；不支持的能力名会明确返回，而不是被静默忽略，这样旧客户端连到新引擎时不必猜测。`platform` 与 `transport` 只是诊断标签，绝不是权限边界。

## 主交互入口

可持续监测的报价来自私人对话中已成功执行的只读工具。其 JSON 结果提供 `url`、`model`、`spec`、`currency`、`price` 和 `shipping`。金额为最多两位小数的非负数，运费为 null 时，到手价仍待确认。

`session/main`（无参数）幂等打开 `jeeves-main`，返回 `SessionNewResponse`；仅首次创建初始化标准权限。`interaction/start` 使用生成的 `InteractionStartParams`：`sessionId`、稳定 `messageId`、人的 `message`，可选 `taskId`、`cwd`、图片 `attachments`。返回 `TurnStartResponse` 与既有通知。Web 对应鉴权的 `GET /api/session/main` 与 `POST /api/interaction`（SSE）。驾驶舱和网关 HTTP 客户端支持跨网络分块的多行 SSE 事件，兼容 LF、CRLF 和 CR 换行。

查询会话失败时，客户端可用 `session/main` 重新取得主会话 ID。

主模型在主聊天里应答和澄清，可通过本次交互的 `task` 工具一次创建最多四项独立委托，或续办属于这条会话的任务。两种操作都使用当前用户消息中的原文片段。同一交互中，已经用于续办的片段不能再次建任务。新任务附带文档引用和有界主会话背景。显式 `taskId` 经归属检查后选定事项。选中盯价事项时，本次交互只能查询该事项、修改阈值或取消监测。排队任务追加要求，执行中任务在下次模型边界接收补充；已结束且无验收条件的单回合任务在原目录/上下文中以新运行续做，其他状态使用既有恢复/核对入口。排队/执行中的新增图片明确拒绝。

任务在执行边界核对累计 token、工具调用次数和耗时。旧检查点所属的回合能关联到任务，但缺少执行标识时，恢复流程会将其挂起待核对。工具结果已记录的普通对话检查点仍可续跑。

任务列表、执行记录、产物和详情查询失败时返回 RPC 错误。任务的检查结果、验收条件或预算无法解析时也会报错。查询不存在任务的执行记录和产物仍返回空列表。

持久 `(sessionId,messageId)` 回执回放相同的已完成提交；修改输入、失败和未完成提交均拒绝重复派发。部分创建/启动失败逐项说明。连接中断导致结果不明时，先检查任务进展。`task/create` 未指定 `cwd` 时分配独立托管目录，主动绑定已有项目仍受支持。给了 `repo`（人选定的 git 仓库绝对路径）时，任务改为在自己的 worktree `<home>/worktrees/<task-id>`、分支 `jeeves/<task-id>` 里运行，那是它唯一的工作目录；`task/view` 和 `task/get` 报告 `worktreePath` 和 `branch`。`task/create` 还接受 `codeChecks: [{name, argv, after}]`：`after: "final"` 变成 `commandSucceeds` 验收条件；`after: "edit"` 在每次 `edit` / `apply_patch` 之后于任务目录运行（走 exec 门和沙箱，每条 60 秒），最多 4 KB 输出追加到该工具的结果；exec 关闭时结果写明 `[check <name>] not run: exec off`。worktree 在 `task/create` 时建立，已存在则复用，从不自动删除；`repo` 不是 git 仓库、或 `repo` 与 `cwd` 同时给出，都以文本错误拒绝。`task/start` 接受可选图片。

`GET /api/notifications` 在单次提交之外推送鉴权的 `ServerNotification`，用于工作者进度与审批。`POST /api/task` 的 `{action:"artifacts",taskId}` 转接 `task/artifacts`。`session/list` 只返回 `jeeves-main`，事项检查可读取内部执行记录。私聊改稿按用户绑定的任务限定候选动作。回指可使用紧邻的上一条已完成私聊交互中的任务绑定、主题或动作 ID；多个候选时请用户选择。任务工具按创建时间列出当前会话最近的 20 项任务。渠道只可查看同一聊天和发送者交办的事项。私人主对话通过当前会话的工具和审批策略执行当场交办。允许派发的交互在两种工具加载模式下都从首个请求起展示本轮专属的任务委派工具。此前聊天有待核对的操作时，工具表收窄到无条件只读工具，直至人工核对。任务会话中，只读模式拒绝记忆修改，标准模式要求审批，完全访问模式使用已启用的工具。外部记忆 MCP 客户端通过单独启用的记忆服务读写。`turn/start` 进入同一个主交互。同一核心仍运行时可以恢复实时待确认问题；缺失/过期确认不会放行操作。详见 [internals/06-app-server.md](internals/06-app-server.zh.md)。

### 实时待确认快照

`approval/pending {sessionId}` 返回 `ApprovalPendingResponse.approvals`：实时确认请求、执行上下文、归属会话及可选的当前任务视图。Web 通过经鉴权的 `GET /api/approval/pending?sessionId=...` 转接；Web/Mac 在打开/重连时修复任务面板。超出最近任务列表窗口、但仍在等待确认的任务会随快照补入。审批审计记录不充当待处理队列。

### 重启后的待处理审批

等待人回答的问题同时存进 `state.db`（迁移 0003，`pending_approvals`）。进程重启后，尚未过期的问题会经
`approval/pending` 回来，带 `restored: true`，`request.version` 加一；已过期的被丢弃。
`ApprovalRequestedNotification.version` 从 1 开始，客户端在 `approval/respond` 里原样回传 `version`。版本过期的
回答以文本错误拒绝（`Error: this approval is now at version N, not M ...`），问题保持原样。
第一个回答生效。之后的回答得到 `accepted: false`，若已有人回答，还会带 `answeredBy`（第一个回答者的 `actor`，如 `telegram:7`、`lark:ou_…`），各渠道据此显示“已由 … 回答”。网关用 `<渠道>:<id>` 作为 `actor`。

恢复出来的问题没有活着的工具调用在等它。回答“是”会为同一个工具、同一个动作预览记下一次性授权，恢复后的回合再次发问时
消耗它；回答“否”不记任何东西，恢复后的回合照常重新询问。授权本身（`action_grants`）只在 account、resource、action、
destination 完全一致时匹配；授权带内容哈希时，内容也必须一致。过期或已撤销的授权永远不匹配。

`request.expiresInMs` 是采集快照时的剩余时间，不会延长原截止时间。截止时间从问题登记时开始，即使等待 Future 尚未运行，回答时也会检查；接收方已经关闭的请求不再有效。请求 id 包含进程/时间身份和计数器，客户端只能原样回传，不能自行生成或复用旧进程的 id。首个成功送达的回答消费请求；过期、取消、撤权及重复回答返回 `accepted:false`。归属会话改权限会让子任务的旧问题失效。客户端使用事件/操作版本防止迟到的旧快照重新显示刚处理的请求。核心重启不会恢复确认授权或旧问题；执行结果不明仍走检查点与操作核对。

### 设备配对与撤销

在操作者的驾驶舱连接上，先通过可信渠道核对设备上显示的 Ed25519 公钥。将 32 字节公钥用标准带填充的 base64 编码，作为 `deviceId` 传给 `device/pair`，同时提供 `expectedGeneration`（首次为 0），并从 `browserControl`、`desktopControl`、`approvalReview` 中至少选择一项能力。私钥保存在设备上。操作者可用 `device/list` 查询授权，用 `device/revoke` 加设备 ID 和当前代际撤销授权。对应 HTTP 接口为驾驶舱认证下的 `POST /api/device/pair`、`GET /api/device` 和 `POST /api/device/revoke`。远程管理驾驶舱时使用可信的加密连接。

将返回的 `grantId` 和 `generation` 交给设备。设备的第一项操作是向 `POST /api/device/report`（RPC 为 `device/report`）上报已授权能力在线。用 Ed25519 给紧凑 JSON 数组 `["jeeves.device.report.v1",deviceId,grantId,generation,capability,sequence]` 的 UTF-8 字节签名，请求体传入这五个字段和标准带填充的 base64 签名 `signature`。同一授权下的序号必须为严格递增的正整数。上报成功会记录并返回序号。每次配对都会产生新的授权 ID，序号从头开始。撤销及重新配对均增加代际。当前授权和序号在 Runtime 重启后保留。

设备端点仅接受能力在线上报。控制输入、桌面操作和审批决定需要针对对应操作另行授权。

### 任务审批通知

在 iPhone 上，将可信 HTTPS 地址的驾驶舱添加到主屏幕并从图标打开。在 Mac 上使用 Safari。先在 Runtime 设置 [Web Push 联系地址](configuration.zh.md)。在「设置 → 审批通知」显示本机公钥，在可信驾驶舱核对这串公钥并授予 `approvalReview`。回到接收设备，先核对配对状态，再点击启用按钮申请系统通知权限。本机的 Ed25519 签名私钥保存在 IndexedDB 中且不可导出。Runtime 在显式设置时生成 VAPID 签名密钥，连同订阅凭据存入凭据库。接收地址须为 Apple 推送服务的 HTTPS 端点。[Apple 平台要求](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/)

锁屏通知打开原任务的审批。在已配对的设备上核对动作、目标和影响范围，再选择允许一次或拒绝。离线、审批过期、已处理或设备授权已撤销时无法提交。撤销时已经发出的通知仍可能出现；打开通知和提交决定时，驾驶舱都会重新核验设备代际和待处理请求。推送服务回执丢失时保留尝试记录，不自动重发。仍可在驾驶舱查看待处理审批。

本地可运行 `just smoke-approval-notice` 验证离线冷启动、审批界面及配对流程。测试使用临时浏览器、假订阅和回环服务。实体 iPhone 的锁屏投递需另行验收。

以下四个接口均需驾驶舱认证。`POST /api/approval/push-key` 返回 VAPID 公钥，未配置时返回 null。`POST /api/approval/push-subscribe` 校验紧凑 JSON 数组 `["jeeves.approval.push.v1",deviceId,grantId,generation,sequence,endpoint,p256dh,auth]` 的 Ed25519 签名，采用设备授权的递增序号。加密通知仅含任务、回合、请求、版本及接收设备授权的标识。`GET /api/approval/notice` 读取当前请求，`POST /api/approval/notice` 提交一次决定。两者都校验 `["jeeves.approval.notice.v1",deviceId,grantId,generation,taskId,turnId,approvalRequestId,approvalVersion,approved]` 的签名，读取时 approved 为 null。签名使用带填充的标准 base64。对应 RPC 为 `approval/pushKey`、`approval/pushSubscribe`、`approval/notice` 和 `approval/noticeDecision`。

### 全局急停

点击驾驶舱顶栏的**全局急停**，当前 Runtime 会断开所有受管任务浏览器，阻止新启动，并在重启后继续挂起受影响的任务。中断前页面可能已收到输入。继续前请在任务操作记录里核对结果未知的操作。

只有驾驶舱操作者可以解除急停。解除后可启动新浏览器。为每个受影响的任务重新打开浏览器，待引擎取得新观察后继续。任务浏览器上的**暂停浏览器**仍只影响当前任务。

操作者可调用 `executor/control`、`executor/stop` 和 `executor/release`。对应 HTTP 调用为 `GET /api/executor/control`、请求体为 `{}` 的 `POST /api/executor/stop`，以及请求体为 `{"expectedGeneration":N}` 的 `POST /api/executor/release`。解除时使用最近读取的代际。冻结期间重复急停会返回同一代际，每次成功解除都会推进代际。设备在线上报和无关的对话仍可使用。急停同时撤销当前原生窗口授权。解除后须重新选择窗口。

### macOS 单窗口操作

打开任务卡的桌面窗口面板，选择应用，再在 Mac 的原生对话框核对窗口名称。只有这个任务能使用所选窗口。助手需要 macOS 14 或更高版本。选择窗口需要“辅助功能”权限，截图预览另需“屏幕录制”权限。权限由操作者在“系统设置 → 隐私与安全性”中授予。开发环境运行 `just desktop-window-helper`，将 `JEEVES_DESKTOP_HELPER` 指向编译命令输出的绝对路径，重启 Runtime。

观察返回可操作控件。支持按按钮、垂直滚动条的步进操作，以及向普通单行字段写入纯文本。在选定应用内切换窗口、关闭窗口或接手后，需重新选择。任务暂停只撤销对应任务的窗口。每条操作消耗一次观察，任务面板保留最近 20 条回执。操作中断且效果未获确认时，回执显示结果不明；请检查目标窗口再决定下一步。

可为 Runtime 选择独立的视觉模型。点击预览只将所选窗口的图像返回驾驶舱。如果台前调度已将窗口移出屏幕，助手可将其应用带回前台。只有在屏幕上唯一匹配已授权窗口时才会生成截图。先核对图片、模型和目的地址，再确认发送这一张。模型只返回文字建议，像素和字段输入不进入回执。刷新观察、修改屏幕模型、接手或急停都会使预览失效。

已授予辅助功能和屏幕录制权限的 Mac 可运行 `just smoke-mac-window` 做原生回归。测试打开两个临时 AppKit 窗口，检查滚动位置，按颜色标记核对各自的预览，并验证切窗撤销授权、同名同位置时拒绝截图。

人工验收可运行 `just desktop-window-fixture` 打开两个临时窗口和独立的本地驾驶舱。测试任务和本机图像接收端均已配置。在浏览器打开终端显示的新驾驶舱地址，点击左侧「主会话 → 任务进展」。保持验收卡待开始，直接在「Mac 单窗口验收 / Window acceptance」卡片展开「任务窗口」，点击「在 Mac 上选择窗口」。选中 JeevesWindowFixture 的一个窗口后，勾选复选框、修改普通字段、滚动列表，核对安全字段没有输入操作。把测试应用切到另一个窗口后尝试再次操作，应提示重新选择。重新选中后验证预览只包含目标窗口，再测试接手和全局急停。关闭测试进程即可丢弃所有输入。

驾驶舱使用 `GET /api/desktop/status?taskId=…` 查询状态，其他操作使用 `POST /api/desktop/{select,observe,act,takeover,model,frame,share}`。对应的操作者 RPC 使用 `desktop/` 前缀。窗口与图像授权仅在当前 Runtime 进程内有效。

### 远程浏览器操作与下载

在已认证的驾驶舱打开任务浏览器，选择**接管浏览器**，再打开远程操作。桌面和手机操作的是同一任务的 Chromium 浏览器。可在画面中点击、输入文字、滚动，或使用 Tab、Enter、Escape、Backspace。每次人工输入消耗一帧并返回新帧。页面变化或经过 30 秒后须刷新画面。交还控制后，引擎重新观察。远程访问驾驶舱应使用可信的加密连接。已配对设备的在线上报权限不包含驾驶舱权限。

在人工接手期间，已允许网站发起的下载会显示在任务浏览器中。文件接收完成后选择**保存**，即可登记到该任务产物；也可**取消**下载。每个浏览器实例最多接收 16 项，每项上限 768 KiB。关闭浏览器会结束临时列表。保存的文件保留在 `<home>/artifacts/<taskId>/`，可跨重启查看。文件名带有唯一下载 ID，同名报表可以分别保留。如果登记已接受的文件时中断，引擎会在启动时从本地副本重试。遇到内容已经被修改的同名文件时，保留现有文件供核对。

驾驶舱认证下的 HTTP 接口为 `POST /api/browser/input`、`GET /api/browser/downloads?taskId=…` 和 `POST /api/browser/download`，对应 RPC 为 `browser/input`、`browser/downloads`、`browser/download`。输入需提供当前代际和画面 ID；保存或取消需提供任务、代际、下载 ID 和决定。旧版可选浏览器组件可用 `just browser-runtime` 更新后启用上述操作。

### 任务浏览器控制

仅用户可调用 `browser/list`，输入 `BrowserListParams`，返回安装可用性和所属浏览器元数据。`browser/start` 输入 `BrowserStartParams`；`browser/control` 输入携带预期代际的 `BrowserControlParams`；二者返回 `BrowserSessionView`。`browser/frame` 输入 `TaskGetParams`，返回限长、按需的 `BrowserFrameResponse`。这些是已认证的客户端 RPC，不是模型工具。Web 对应 `GET /api/browser`、`POST /api/browser/start`、`POST /api/browser/control`、`GET /api/browser/frame`。工作进程传输类型也由同一 Rust 协议定义，但其命令不是公开 app-server RPC。

## 周期性责任

所有者在主聊天提出创建、修改、暂停或恢复。模型通过 `recurring_work` 提议，工作台从 `GET /api/responsibilities/reviews` 读取卡片，向 `POST /api/responsibilities/decide` 提交所选 ID 和布尔值 `accept`。确认在同一事务检查版本并消费卡片。

已认证的工作台可向 `/api/responsibilities/prepare` 提交待核对契约，查看规范化后的定义。核对只校验字段。正式登记时会重新校验契约、检查标识是否可用，并启用委托。

人可以通过 `POST /api/responsibilities` 登记，通过 `GET /api/responsibilities` 查询，向 `POST /api/responsibilities/pause` 发送 `{"id":"weekly","paused":true}` 暂停。接口沿用控制台的访问控制。模型和聊天网关没有登记或暂停权限。

登记需要唯一的调度标识、目的、具名证据来源、按运行时主机本地时间解释的五段 cron、未来的 RFC 3339 截止时间，以及至少一项正数任务预算（`tokens`、`tools` 或 `wallMs`）。登记后契约不可覆盖。来源名称只标识证据，连接器操作仍需独立授权。

调度器在有效时段匹配到 cron 时，用同一数据库事务记录待处理唤醒及其 UTC 分钟。重复轮询和重启都只能认领该分钟一次。停机期间错过的分钟不会补跑。暂停或到期会把待处理唤醒转入对账，并在每次状态转变时报告一次停止原因。恢复后，截止时间前的后续轮询可以继续认领。普通责任只记录到点工作。

私人摘要可组合 `calendar`、`openTasks`、`unreadMail`，每种来源生成一段或一个列表。填写 `digest.timeZone`、`calendarWindow`（today、tomorrow、thisWeek、nextWeek）和 `layout`（paragraphs、list）。所有者通过 `POST /api/responsibilities/sample` 读取样例。正式执行在同一事务写入主聊天消息和送达记录。

### 飞书周报

登记责任时可设置 `weeklyReport: {"taskId":"…","recipient":"oc_…"}`，在 `sources` 中写入研究任务允许使用的完整 HTTP(S) URL。例如 `0 9 * * 1` 在主机本地时间每周一 09:00 触发。任务工作区需要有 `claims.json` 和 `fetch_url` 保存的来源快照。每次周报只引用触发前七天内经 URL、摘要和原文核对的摘录。来源缺失、过期或不符会留下待检查记录。

通过 `GET /api/responsibilities/reports?id=<责任标识>` 查看正文、审批和投递结果。每次触发独立计算预算，研究核对计一次工具操作，批准后的发送尝试再计一次，模型 token 消耗为零。墙钟预算包含等待审批。默认生成本地草稿；实际发送需开启飞书 live、收件人位于允许名单且 `JEEVES_APPROVAL=on-request`。用已有的 `approval/respond` 回答绑定账号、收件人和正文摘要的问题。只有回读确认相同消息 ID 才算送达。中断、回执未知或范围变化都会停止该次投递，保留记录供人工对账。
