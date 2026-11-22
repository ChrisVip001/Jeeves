# ⑥ 服务端：`jeeves-app-server`

[English](06-app-server.md) | 简体中文

这个 crate 是每个 `state.db` 的唯一执行 Runtime，前面各层都在这里装配成一轮对话。客户端（CLI、dashboard、聊天网关、桌面端）说 [01-protocol-and-config](01-protocol-and-config.zh.md) 里的协议；方法与通知表见 [../protocol.zh.md](../protocol.zh.md)，环境变量见 [../configuration.zh.md](../configuration.zh.md)。

## 目录布局

| 路径（`runtime/crates/jeeves-app-server/src/`） | 负责 |
|---|---|
| `lib.rs`、`server/` | `Server`（共享句柄）、启动（`runtime.rs`）、工具装配（`tools.rs`）、轮表/容量上限/跟进队列（`controls.rs`）、事件出口（`events.rs`） |
| `stdio.rs`、`dispatch.rs` | 传输与方法表 |
| `turn/` | 一轮的生命周期：`entry.rs`（准备）、`execution.rs`（一次 loop）、`continuation.rs`、`driver.rs`（任务回合）、`finalize.rs`、`settlement.rs`、`prompt.rs`、`history.rs` |
| `task/`、`interaction/` | 任务账本开关、派发 worker、等待、工作区；主对话及其 `task` 工具 |
| `approval.rs`、`permissions.rs`、`pending_approvals.rs` | 批准代理、会话策略、实时待批快照 |
| `subagent.rs`、`jobs.rs`、`goal.rs` | 子代理执行器、后台作业宿主、目标裁判 |
| `ledger_writer.rs`、`memory_worker.rs`、`memory_center.rs` | 账本行、后台记忆整理、记忆中心窗口 |
| `resume.rs`、`scheduler/`、`lock.rs`、`resources.rs`、`trace.rs`、`agents_md.rs` | 崩溃续跑、定时任务、运行时所有权、变更闸门、trace 文件、项目约定 |

确定性的一半在 `runtime/crates/jeeves-agent/src/`：`verify.rs`（验收检查）、`observe.rs`（一轮做了什么）、`plan.rs`、`paths.rs`（写工具与验收共用的唯一包含规则），以及纯函数的 cron / schedule。那个 crate 不持有 `Server`、不碰 JSON-RPC，事实判断可以单独测试。

## Server 与启动

### 字段与锁

`Server` 是 `Clone`：每个字段都是句柄（连接池、`Arc`、配置格），克隆不复制状态。需要 `'static` 所有权的闭包拿一份克隆。

| 句柄 | 作用 |
|---|---|
| `settings: Arc<RwLock<Settings>>`、`resolved: Arc<RwLock<Option<Resolved>>>` | `config/write` 与模型切换改它们，下一轮即生效。未配置模型时 `resolved` 为 `None`：记忆与会话方法照常，只有轮次以 `PROVIDER_ERROR` 失败并说明该怎么做 |
| `turns: Mutex<HashMap<turn_id, ActiveTurn>>` | 每个在跑的轮的取消令牌、会话 id、开始时间、插话通道 |
| `approvals` | `turn_id → request_id → Waiting` |
| `session_locks` | 每会话一把异步写段锁，惰性创建、永不删除（删除会让两个并发轮持有不同的锁） |
| `activations` | 每会话的延迟工具激活表，只在内存 |
| `context_usage` | 每会话最近一次的上下文构成，供 `context/usage` 读取 |

std 锁只守一次读改写，绝不跨 await。每会话写段锁覆盖历史追加、账本追加、整理交接与压缩；模型调用从不持有它，所以中断不会排在它后面。

### 启动顺序

`open(settings)`：把已保存的 `config/write` 补丁叠在环境变量之上，对最终值 `validate()`（坏值拒绝启动并点名变量），`ensure_home`，取得运行时所有权（`lock.rs`：`state.db` 旁的内核咨询锁加 `app-server.lock` PID 文件；同一数据库的第二个运行时被拒绝），打开连接池，再 `Server::boot_owned`。`boot`（不加锁）供独占自己存储的嵌入方与测试使用。

`boot_owned` 按依赖顺序：应用活动模型档案，解析 provider（失败只记日志，不致命），写入默认 skill，构建工具表，加载会话策略，挂上变更闸门，构建各存储，注册 `todo_write` 与 `delegate_task`，执行 `restore_tasks`，启动完成通知投递。记忆 worker 由进程挂接（CLI 调 `attach_memory_worker`），不由 `open` 挂接，使 eval 保持确定；没有 worker 时 `finalize_turn` 内联整理。

### 工具装配

`builtin_tools` 把所有可开关的能力各建一次（`exec`、`write`、`delegate` 强制打开），每轮再按当前设置隐藏被关掉的（`enabled_tools`：`JEEVES_EXEC` 对应 `run_command` + `job`，`JEEVES_WRITE` 对应 `write_file` + `edit` + `apply_patch`，`JEEVES_DELEGATE` 对应 `delegate_task`）。因此开关在轮开始时读取，翻转无需重启。这里还装配：网页搜索、浏览器工具（仅当浏览器运行时存在）、飞书连接器（仅当白名单非空才注册）、`mcp.json` 的 MCP 工具（按独占注册，因为远端 schema 不能证明只读；连不上的服务只留一行 stderr）。`ToolCtx` 只有一个构造点（`tool_ctx`），轮与子代理共用，新增字段不会漏掉某条路径。`delegate_task` 拿到的是它注册之前的工具表副本：递归是不存在，而不是被拒绝。

`resources.rs` 是变更闸门：所有会话共用一把互斥锁；有副作用的工具或运行中的后台作业持有它，第二个申请者等 2 秒，之后得到一条文本错误，提示先检查进度。

### 启动对账

`restore_tasks` 处理死去进程留下的现场，且从不重放副作用：停在 `running` 的步骤变 `waiting`；停在 `running` 的 run 变 `waiting`；`running` 的任务变 `suspended`；被打断的派发行需要复核；被打断的排队指令在记录里留一条说明；持久的作业等待与外部输入等待重新武装；仍有排队跟进的会话开始排空。仍在运行的作业由作业宿主标记为中断。随后每次传输启动时 `resume::reconcile` 续跑所有 `open` 的检查点，但被中断的、失败的、以及超过 `JEEVES_RESUME_MAX_AGE`（默认 60 分钟）的除外，它们标记为 `abandoned` 并输出一行 stderr。续跑重新经过同样的批准关口，不放宽任何东西。接着 `task::dispatch::kick` 启动任务 worker。

## 传输

`stdio.rs`：逐行 JSON-RPC。通知与响应共用一个无界通道、恰好一个写任务；通知被包成方法 `turn/notification`。解析失败回 `PARSE_ERROR`，id 为 null。`turn/start` 与 `interaction/start` 会 spawn，使读循环继续读（轮在等批准时 `turn/interrupt` 与 `approval/respond` 必须进得来）；其余方法按序处理，脚本先前的写入对后面的查询可见。日志只写 stderr，因为 stdout 是协议通道。stdin 关闭后循环结束，spawn 出去的轮继续跑完（写任务在所有 sink 克隆释放前一直存活），记忆 worker 得到停止宽限，MCP 会话被关闭。

## 分发

`dispatch.rs::handle` 一定产出响应：`Ok(Value)` 成为响应，`Err` 成为错误，`Outcome::SelfSent`（轮与交互方法）表示轮已自己发过响应。match 覆盖会话、记忆、配置、模型、定时、用量、批准、权限、任务、浏览器与轮方法；未知方法先交给 `memory_center::handle`，否则回 `METHOD_NOT_FOUND`。权威方法清单是 `jeeves_protocol::methods`，这里不抄数量。

改之前值得知道的约定：存储的 `i64` 通过 `narrow` 夹取成协议的 `i32`；列表方法按不透明游标分页，游标在原始行上计算；`session/main` 打开唯一公开会话；`turn/interrupt` 对未知轮返回 `interrupted: false`；`approval/respond` 来迟时返回 `accepted: false`。

## 一轮

`run_turn`（[`turn/entry.rs`](../../runtime/crates/jeeves-app-server/src/turn/entry.rs)）的顺序如下；每一步放在那个位置，是为了让被拒绝的轮不留下任何东西。

### 准备

1. 空会话 id 变为 `default`；上一轮以来后台整理出的记忆以 `consolidationCompleted` 告知。
2. 认领任务：worker 轮用派发行的任务，否则用会话的活动任务。要在解析模型之前，因为任务可以声明档位。
3. 档位：显式参数优先于任务声明，拼错被拒。解析模型（没有则 `PROVIDER_ERROR` 并给出指引）；`route.rs` 按档位或自动模式选模型，原因写入 `meta.routeReason`。
4. 读取附件并按视觉能力校验。
5. 对任务：取 `task-control:<id>`，复核准入（任务未关闭、派发状态一致），拒绝结果未知的检查点。
6. 容量：先 `turn_capacity`（`JEEVES_MAX_TURNS`），再 `home_capacity`（`JEEVES_HOME_MAX_MB`），见下文“容量、健康检查与关停”一节。
7. 会话闸门 `begin_turn`：每会话一个驾驶者；第二个轮得到 `SESSION_BUSY` 并带上持有者的 turn id（“等它，或用 `turn/steer`”）。`TurnGuard` 在任何出口移除轮表项，然后排空该会话的跟进队列。
8. 任务准入（`task/contract.rs`：验收与预算在任何模型调用前校验）、心跳 `set_running` + `taskUpdated`、可选计划、开 run（run id 等于 turn id；无任务的轮没有 run）、构建检查点出口。
9. 工作目录：本轮显式给的，否则任务的，否则会话的。目录不存在表示没有可写范围；从不使用进程 cwd。
10. 发送 `TurnStarted`；把计划摘要附到消息末尾；安装重试出口（每次重试发 `retry` 并计入 `meta.retries`）。

### 执行

`full_turn` 是一次 loop，也是模型所见一切的汇合点：任务交接与进度投影、记忆检索（受 60 秒预循环超时约束）、skill（索引加命中正文）、在会话写段锁下的历史+滚动摘要+压缩（同样有界、可中断）、记忆目录、项目约定、系统提示、上下文计量、工具表（`enabled_tools`、会话激活、仅主对话有的 `task` 工具、早先聊天操作未决时只留已证明的读工具、`JEEVES_TOOL_POLICY=deferred` 下的延迟 schema），然后 `jeeves_loop::run`。上下文溢出错误会压缩到最近两轮并重试一次；其他模型错误变成 `PROVIDER_ERROR`。loop 事件立即变成 `toolStarted` 与 `toolDelta`；`toolCompleted` 在收尾时发。loop 见 [03-model-and-loop](03-model-and-loop.zh.md)。

任务交接包含近期任务事件、上次记录的编辑检查摘要、最后一次成功记录的待办替换中的未完成项，以及编辑操作提及的路径。这些路径和历史检查只是证据，并非当前文件状态的证明；其中可能包含中断或失败的操作。交接还包含已保存的计划和最新验收回执。编辑检查摘要作为任务事件记录，不保存命令输出。

失败会如实关闭检查点、run 与任务（`fail_turn`），因为留着 open 的检查点会在下次启动被当作什么都没发生而续跑。

### 自续

`continuation.rs`：一次 loop 之后，轮可以自己续跑，最多 `JEEVES_AUTO_CONTINUE_MAX` 次（默认 2）。闸门只在这次被步数上限截断（`iterations >= cap`）且会话 todo 里仍有未完项时才开；自己停下的轮从不被劫持，带目标的轮归目标循环管。续跑带着上一段的消息、步数与用量恢复，并额外买 `JEEVES_TURN_STEP_SLICE`（默认 12）步。更早各段的工具调用被接在最后一段之前，收尾因此只报一个请求顺序。预算用完时回复会说明差一步，trace 记录 `unfinished`。

### 任务回合

`driver.rs::drive_task` 在每次 loop 后运行。顺序固定：观察 → 记账 → 度量 → 决定。没有任务的轮也经过它，并以 `Done` 结束。

- `account_round` 写步骤（幂等键 `<turn_id>:<seq>`）、真实存在的产物、花费，并运行验收（`fileExists`、`fileContains`、`jsonEquals`、`toolsOk`、`receiptConfirmed`、`commandSucceeds`：一次 `stat`、一次读、一次 SQL 计数，或经 exec 门运行一条命令；路径经 `paths.rs` 解析）。
- 决定顺序，先命中者生效：被中断；预算（时长、token、工具调用；`budget` 事件点明哪一项）；工具结果未知；仍有后台作业（`WaitingForJobs`）；触达输出或步数上限；声明的外部输入仍待或已过期；验收失败；矛盾（工具说写了、文件不在）；最后才是裁判。
- 验收失败且还有回合（`max_rounds`：任务自己的，否则 `JEEVES_GOAL_MAX_ROUNDS`，默认 5）时，把度量结果作为下一条消息再跑一次；回合用尽则任务进入需要关注。有事实失败时不调用裁判。
- 裁判（`goal.rs`）只在任务有目标条件时运行：小模型、无工具、最近 8 轮且截断到 8000 字符、JSON `{ok, impossible, reason}`；自相矛盾的回答按未达成算。`ok` → 完成，`impossible` → 失败，回合用尽 → 失败且状态为 `round-limit`，裁判坏了 → 需要关注且状态为 `blocked`，否则理由成为同一会话的下一条用户消息。每个结论都发 `goalRound`。只有人能设目标（`set_task_goal`，任务行上的一列），模型没有通往它的路径。
- `needs_attention` 非终态：人可以继续或取消。
- 计划（`JEEVES_PLAN`，默认关）：第一次 loop 前一次模型调用产出 3 到 6 步的计划，存为 `task_steps`（`kind = "plan"`）并以文本附到消息上。验收失败可修订一次（`MAX_VERSIONS = 2`）。计划从不执行任何东西，也从不携带验收条件；规划失败只打一行，轮照常运行。

### 收尾

`finalize_turn`，按顺序：

1. `settle_task`（`settlement.rs`）关闭检查点（`done`；`turn/interrupt` 为 `abandoned`；`task/suspend` 或结果未知为 `suspended`；已提交等待时为 `waiting_jobs` 或 `waiting_input`），并按决定关闭任务，发 `taskUpdated`。
2. 按请求顺序为每个调用发 `toolCompleted`。
3. `fold_tool_activity` 在存储的回复后为每个调用追加一行：`[tools used: name(N chars[, full output at spill/…]; first line…)]`。超过 `FOLD_SPILL_CHARS`（2000）且自身没有地址的输出，在折叠时溢出到 `<home>/spill/`。这是阻止下一轮重复动作的证据；`turnCompleted.reply` 带的是未折叠文本。
4. 写一行 trace 与一行用量（失败只记日志）。
5. 在会话写段锁下：追加 `chat_log`（浏览器任务的来源为 `browser-task`，记忆提取会跳过）、追加账本（`ledger_writer.rs`）、保存（实测、估算）输入 token 对、武装作业或输入等待。
6. 唤醒记忆 worker 并 spawn 预压缩；没有 worker 则内联整理并发 `consolidationCompleted`。
7. 发 `turnCompleted`，然后才发 JSON-RPC 响应，使客户端在答复前收齐所有通知。续跑路径使用同一个 `finalize_turn`。

## 提示装配

`build_system(PromptParts) -> SystemPrompt { stable, tail }`；切点就是缓存断点。

```
stable = SOUL.md + “你的模型 / provider”一行 + 对话规则      （会话内逐字节不变）
tail   = 时钟（按天）· 能力缺口 · 能力使用说明 · todo · 记忆命中 ·
         记忆目录 · skill 目录 · 命中的 skill 正文 · 项目约定 · 滚动摘要
         （之后由 full_turn 追加：未决操作说明、会话权限说明）
```

- 每轮会变的东西不进 `stable`；时钟只到天，前缀不会每小时失效。
- 能力说明只在有东西关着时出现（`JEEVES_EXEC` / `JEEVES_WRITE`，或根本没有工具）：没有它，模型分不清“工具不存在”和“还没找到”。开关全开时它不出现。exec 打开时会列出已存凭据的名字（从不列值）。
- `SOUL.md` 归用户。`load_soul` 仅当文件与某个内置种子（当前或旧版）逐字节相同才换成当前种子；一旦被编辑就再不碰。`JEEVES_SOUL_MODE`（`auto`/`thin`/`full`）选种子；`auto` 对本地 provider（`ollama`、`lmstudio`）给完整版。
- 灵魂种子、尾部各段、规划提示与引擎错误措辞用同一种语言写；用户内容（回复、标题、记忆页、`AGENTS.md`、改过的 `SOUL.md`）保持原样。

### 项目约定

`agents_md.rs`：从本轮工作目录向上走到含 `.git` 的目录（或磁盘根），每个目录取一个文件（`AGENTS.override.md`，否则 `AGENTS.md`，否则 `CLAUDE.md`），按最外层在先拼接，并以来源路径作前缀。预算 32 KB，从最内层文件向外填充：外层文件整个丢弃，超限的最内层文件被截断并标注。只进尾部，因为它随目录变化。没有文件就没有这一段、不占 token。子代理不会拿到它（`instructions: None`）。没有工作目录时什么也不读。

### 上下文计量与校准

`usage_of` 计量六段：soul（`stable`）、memory（命中加目录）、skills（目录加正文）、tools（schema）、history、summary。时钟、todo 与能力行不计入：它们很小且不增长。预算 = 上下文窗口（`JEEVES_CONTEXT_WINDOW` 或 provider 表）× `JEEVES_COMPACT_THRESHOLD`，下限 1024，再除以校准系数；预算为零时比例记 0、不告警。达到或超过 `JEEVES_CONTEXT_WARN_RATIO`（默认 85）时面板给出提示。

校准：收尾时把最后一次请求的 provider 实测输入 token 与本地估算存入 `session_context`；下一轮把预算除以 `实测 / 估算`，夹在 0.5 到 2.0。任一数字非正则不记录，只保留最新一对，写失败只记日志。

## 批准与权限

`approval.rs` 实现 `ApprovalBroker`（工具层只认这个 trait）。一次请求在 `approvals[turn_id][request_id]` 登记一个 `oneshot`（id 为 `ap-<pid>-<微秒>-<计数>`，跨进程生命周期唯一），发送 `approvalRequested`（及可选的 notifier），然后带超时等待（`JEEVES_APPROVAL_TIMEOUT`，默认 120 秒），超时由这个代理而不是工具层负责。无论结果（批准、拒绝、因轮结束而作废、超时），都向批准审计账本写一行并移除登记；不是明确的批准一律视为拒绝。`finish_turn` 丢弃一个轮的整个桶。

`answer_approval` 对来迟或未知返回 `false`（协议里的 `accepted`）。`remember` 在答复送达之前应用，使下一次调用已经看到：`once` 什么都不记，`session` 只写进程内的活格，`always` 还通过 `apply_config_patch` 持久化（命令原样存储，从不存成通配；写入批准存目录而不是文件）。被记住的规则绕不过硬拒绝与沙箱。`approval/pending` 返回某会话名下未过期问题的实时快照，来自等待表而不是审计日志；重启后什么都不留。

`permissions.rs` 持有人控的会话策略（`readOnly`、`standard`、`fullAccess`），每会话带版本号，持久化并在每个工具边界检查（`ExecutionPolicy`）：

- 只读拒绝一切未被证明为读的操作；standard 每次操作问一次（参数脱敏，预览上限 8 KB）；完全访问仍受已启用能力、根目录与沙箱约束。委派除完全访问外一律拒绝。
- 升到完全访问需要明确确认；写入需带期望版本，过期客户端被拒。策略变更会暂停所属者的浏览器、作废所属者的待批请求，并广播 `permissionChanged`；已在途的副作用不回滚。
- 任务的 worker 会话绑定到所属对话：适用所属者的策略，只能通过所属者修改，启动时恢复绑定。
- 主对话（`jeeves-main`）只执行已证明的读与 `task` 交接。
- 存储的策略读不出来时阻止工具执行，而不是默认放开。

## 子代理

`subagent.rs` 为 `delegate_task` 实现 `SubagentRunner`（依赖方向不允许工具层持有它）。不变量：

- 设置与解析后的模型在调用时通过共享的 `Arc<RwLock<_>>` 读取，从不在构造时快照，所以 provider 切换会生效。
- 独立上下文：空历史、简短 system、无检索、skill、摘要与项目约定；不写入 `chat_log`；没有批准通道；没有通知与中断。
- 工具表：父轮已启用的表去掉 `delegate_task` 与 `todo_write`；只读的批量成员还会失去独占工具，因为四个并行 loop 无法遵守单写者屏障。
- 上限：迭代默认 5、硬上限 10；输出 2048 token；独立于 `JEEVES_TURN_TOKEN_BUDGET` 的 120 000 token 预算。
- `result_schema`：回复必须通过校验；重试一次，携带错误与之前的消息；仍不合格则返回原文并附原因。
- 结论由 `jeeves-tools/src/report.rs` 包成转述（见 [04-tools](04-tools.zh.md)）。

## 后台作业

`jobs.rs` 为带 `background=true` 的 `run_command` 与 `job` 工具（output、list、kill）实现 `JobRegistry`。它与前台共用 `ExecPolicy`，所以后台不是绕开批准、沙箱或白名单的后门。实时输出在 64 KiB 环形缓冲里，并记录丢弃了多少字节；每个字节同时追加到 `<home>/jobs/<pid>-<id>.log`（上限 1 MiB，之后留一条标记）。`wait=true` 最多等 20 秒，并如实说作业仍在运行。开始与结束是 `state.db` 里的行；启动时仍在运行的作业被标记为中断，从不重跑。作业按所有者隔离：未知 id 与他人的 id 给出同样的答复。作业运行期间持有变更闸门。

一个任务的回合结束时若作业仍在运行，任务会以 `waiting_jobs` 挂起；轮询器（`task/waits.rs`，1 秒）在它们结束后通过同一个恢复驱动让其继续，外部输入（`task/signal`）对 `waiting_input` 同理（`input_wake.rs`）。两种等待都是持久的，启动时重新武装。完成通知从已提交的结果行投递（`job_delivery.rs`、`completion_delivery.rs`），重试到 notifier 接受为止。

## 主对话与任务

`session/main` 打开 `jeeves-main`（只创建一次，默认策略 standard，无目录）。`interaction/start` 是它上面的一次普通轮。没有路由模型：当消息要求做事，这一轮调用 `task` 工具（`create` 最多四个独立项、`continue`、`cancel`、`list`）。工具只在该交互运行期间存在，且只给允许派发的发送者（本地视图即所有者；聊天发送者必须列在 `JEEVES_DISPATCHERS` 中，空列表谁也不放行；检查在 Runtime 内）。每条 `instruction` 必须是当前人类消息中逐字、互不重叠的片段，模型因此无法凭空造活。显式的 `taskId` 在所有权检查后直接续该任务。

幂等：`(sessionId, messageId)` 是持久化的回执（消息 id 为 1 到 120 个字母、数字、`-`、`_`；消息最多 64 000 字节）。相同且已完成的输入重放其存储的事件与结果、不做新工作；同一 id 下不同输入、或未完成或失败的回执，一律拒绝。带 `requestId` 的 `turn/start` 用同一张表，键前缀为 `turn:`。确认只说已排队，从不说已完成。

新任务获得托管工作区 `<home>/workspaces/YYYY-MM-DD-title-uniqueId`，规范化后位于 home 内（标题只留字母数字与 `-`，最多 32 字符；身份是 id 而不是标签），除非绑定了项目目录。它还会收到其确切指令、开头的文档引用，以及主对话最近两次交换的有界快照（人类至多 2000、助手至多 3000 字符，各自），标注为引用数据。主对话的提示只看到有界的任务进度投影，从不看到任务的工具输出。

任务执行是一个串行 worker（`task/dispatch.rs`），排空持久的派发队列：为任务的会话命名、绑定其权限所有者，并带着派发行运行 `run_turn_inner`。待执行的跟进会追加，运行中的在下一个模型边界被插话（`turn/steer`），`turn/follow-up` 排队一条在当前轮之后运行的消息（空闲时被拒）。人的开关（`task/suspend`、`resume`、`cancel`、`signal`、`wait`）从不是模型工具。`task/list` 返回最近 100 个。

## 容量、健康检查与关停

- `JEEVES_MAX_TURNS`（0 = 不限，最大 64）限制跨会话的并发轮数；超出时轮以 `SESSION_BUSY` 被拒，绝不悄悄排队。
- `JEEVES_HOME_MAX_MB`（0 = 不限）在 home 目录（普通文件，不跟随符号链接，在异步线程之外测量）达到或超过上限时拒绝新轮；读取、导出与清理照常，主人始终能腾出空间。两种拒绝都会打印一行 stderr。
- `GET /healthz`（`jeeves-ops/src/api.rs`）：以 3 秒为界调用 `session/capabilities`，成功返回 200 与版本、协议版本，否则 503。
- 关停（`jeeves-ops`）：收到 Ctrl-C 或 SIGTERM 时 HTTP 服务停止接收，然后 `shutdown(25 秒)` 关闭子进程 stdin 并等待；子进程按“传输”一节所述跑完在途的轮。超过宽限则子进程被杀，未完成的轮在下次启动时从检查点续跑。所有权锁由操作系统在退出时释放。

## 记忆中心与后台维护

| 文件 | 负责 | 边界 |
|---|---|---|
| `ledger_writer.rs` | 每轮结构化的 user / assistant / tool_use / tool_result / image 条目 | 只追加；超过 `JEEVES_LEDGER_INLINE_MAX` 的载荷移到 `<home>/ledger/`，行里留指针与摘要；失败只降级为一行 stderr |
| `memory_worker.rs` | 离开轮的时钟做提炼与 bundle 镜像 | 通过数据库认领保证单飞；由收尾唤醒，60 秒心跳兜底；`stop` 最多等 5 秒，未完成的工作在下次启动继续 |
| `memory_center.rs` | 只读窗口（账本、索引、页面、上下文用量、skill 统计）与 `session/compact` | 只读、限定在 home 内、页面路径由 `Bundle::page_path` 校验；压缩只有人能触发，并与 REPL 共用 |

不变量：`entries_for_turn` 与 `payload_text` 解码同一批行，所以被移出的行既能查看也能重建；摘要只向前走（`save_rollup` 拒绝覆盖覆盖更多轮的摘要，错误信息会说明）；手动压缩保留最新 2 轮。检索、整理与 bundle 见 [05-memory](05-memory.zh.md)，表见 [02-state](02-state.zh.md)。

## 观测文件

`trace.rs` 每轮向 `<home>/traces/<本地日期>.jsonl` 追加一行 JSON：`meta` 加供 `jq` 使用的平铺字段（`tools[]`、`retries`、含估算与实测输入及轮预算的 `tokens`、`history` 的总轮数/保留轮数/是否压缩、`steps`、`continues`、`unfinished`）。用量账本在 `state.db`。写失败只记 stderr，从不 panic、也不让轮失败。

## 修改入口

| 想改 | 改哪里 |
|---|---|
| 一个方法 | `jeeves-protocol` 的 v2 类型与 `methods`，再 `dispatch.rs`，然后 `just write-app-server-schema` |
| 一轮的顺序 | `turn/entry.rs`、`driver.rs`、`finalize.rs`；保持检查先于裁判、响应在通知之后 |
| 某工具的能力开关 | `server/tools.rs` 的 `enabled_tools` 与 `builtin_tools` |
| 提示各段 | `turn/prompt.rs`（`stable` 必须在会话内字节稳定） |
| 会话策略或批准 | `permissions.rs`、`approval.rs`、`server/controls.rs` |
| 一个上限 | `jeeves-config` 的边界，再 `server/controls.rs` |

测试：单元测试紧挨各模块（`turn/tests.rs`、`dispatch_tests.rs`、`approval_tests.rs`、`permissions_tests.rs`、`jobs_tests.rs`、`resume_tests.rs`、`interaction/tests.rs` 等）；跨模块的装配行为在 [08-quality](08-quality.zh.md) 的 eval 里。架构总览：[../architecture.zh.md](../architecture.zh.md)；已知缺口：[../limitations.zh.md](../limitations.zh.md)。
