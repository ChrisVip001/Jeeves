# 已知边界，以及要改从哪儿下手

[English](limitations.md) | 简体中文

这里的每一条都是**已知**边界：要么是刻意的设计决定，要么是没人做的工作。每条写明边界是什么、
为什么存在、想改的话先打开哪个文件。

读法约定：**刻意**表示行为符合预期，改动属于设计变更，先讨论；**未做**表示我们想要但没人做；
**边界**表示某项保证到此为止。

## 凭据库只管命令

凭据库按原文精确匹配，从 `run_command` 的输出里抹掉值。命令如果先变换再打印（base64、拆开），脱敏就失效；
发给远程服务器的密钥也不在 Jeeves 手里。它缩小的是模型能读到的范围，不是对付恶意命令的沙箱。
→ `runtime/crates/jeeves-config/src/vault.rs`、`runtime/crates/jeeves-ops/src/api/vault.rs`

## 模型与密钥

* **刻意** — 模型只通过 `model/*` 配置（REPL 的 `/model`、驾驶舱面板）。`JEEVES_API_KEY` / `JEEVES_BASE_URL` /
  `JEEVES_MODEL` / `JEEVES_SMALL_MODEL` 不再读取，`config/write` 也不接受模型字段。`JEEVES_PROVIDER` 只表示
  「什么都没配置时的兜底 provider」。
* **刻意** — 有可用的系统钥匙串时密钥放进去，否则放 `<home>/secrets.json`（明文，Unix 上权限 0600）。
  钥匙串调用失败或超过 5 秒即视为失败并回退到文件，所以一次凭据操作永远不会卡住服务。
  `model/save` 如实报告落点（`keychain` / `file`）。
  → `runtime/crates/jeeves-config/src/models.rs`（`SecretStore`、`KEYCHAIN_TIMEOUT_SECS`）
* **边界** — 平台分配的 profile（`origin: platform`）指向平台模型代理并携带租户 token，请求会在代理处预留月度额度
  （`control-plane/meter.go`）。自带与本地 profile 使用自己的 base URL 和凭据，从不经过代理，也从不消耗平台额度——费用由用户的
  服务商结算。归属是结构性的（端点 + 凭据），不需要额外请求头；代理会拒绝没有有效租户 token 的请求，所以 BYOK profile 无法
  静默消耗平台额度。
  → `control-plane/provisioning.go`、`control-plane/meter.go`、`runtime/crates/jeeves-config/src/models.rs`
* **边界** — 平台 profile 是锁定的：`model/save` 拒绝改写它的 provider、模型、端点或凭据，`model/remove` 拒绝删除它；
  切换到它是允许的。额度耗尽时平台代理返回 429，文案为"额度用完，请充值或配置自己的模型"。可选的 `settings.json`
  开关 `modelAutoSwitchToByok`（默认 false，仅限人工）允许**当轮**回退到用户某个带密钥的 BYOK profile——且只在
  HTTP 429 或网络错误时触发，绝不在 5xx、401/402/403、流中断或已显示文本之后触发。开关关闭时只能手动切换。
  救援 profile 在 fallback 链里带专门标记，因此普通显式 `fallback` 条目仍沿用较宽的切换规则。
  → `runtime/crates/jeeves-provider/src/fallback.rs`、`runtime/crates/jeeves-app-server/src/route.rs`
* **边界** — 平台可以提供多条能力线（语言、多模态、视频理解、视频生成、决策）。带视频附件的回合**仅在平台 profile
  处于活动状态时**才会路由到平台的 `video-understanding` profile；活动 profile 是 BYOK/本地时绝不重定向到消耗额度的平台线。
  视频附件以绝对路径传给模型，绝不当图片解码。活动 BYOK 模型缺少能力时直接明确拒绝（图片为"你的模型看不了图"，
  视频为"你的模型看不了视频"），不静默借用平台。视频理解与生成最终取决于平台模型支持什么——Jeeves 负责路由和命名能力，
  自身不实现视频推理。
  → `runtime/crates/jeeves-app-server/src/turn/{capability,attachments,entry}.rs`、`route.rs`
* **刻意** — 切换模型只重写进程内设置并重新解析 provider；配置存在 `<home>/models.json`，下次启动仍用上次的选择。
  模型不能切换模型：`model/*` 仅限人工，模型看不到密钥。
* **刻意** — 模型目录（`<home>/models-catalog.json`）启动时读一次；新增条目需要重启。坏行各花一行 stderr 并被跳过；
  文件整体损坏则退回内置表。
  → `runtime/crates/jeeves-provider/src/catalog.rs`

## 通知与同步

* **边界** — 开启 `JEEVES_NOTIFY` 后，作业、定时任务和目标的结果在投递前先提交到 `<home>/outbox/notifications/`。
  失败每 30 秒及重启时重试，每轮最多 32 条回执。投递是至少一次：远端确认后崩溃可能重复。待发回执保持绑定到配置的
  目的地；改了 webhook，旧回执会保留而不是转发到新目标。审批提示尽力而为，从不重放。支持接收
  `{"text": …}` 的 webhook 端点。outbox 记录的是传输确认，不是用户已读。
  → `runtime/crates/jeeves-tools/src/notify_queue.rs`
* **刻意** — 跨设备同步是敲门，不是推流：`/api/events` 的 ping 只说「有事发生」，客户端自己重新拉取所显示的内容。
  任务事件是例外：`task/events` 可以从游标读取。聊天没有消息级增量协议、没有离线聊天队列或通用聊天推送。已配对设备可接收[任务审批通知](protocol.zh.md#任务审批通知)。

## 运行时所有权与并发

* **边界** — 每个受检入口（app-server、REPL、单轮 CLI）在打开并恢复数据库之前，先对 `<database>.runtime.lock`
  加内核级建议锁，锁按规范化后的数据库路径计。竞争的命令会明确报错：用现有运行时、停掉它，或换一个 `JEEVES_HOME`。
  不要删锁文件绕过持有者。这是本地协作式所有权，不是分布式租约，也不是工作区写隔离。
  `Server::boot(pool, settings)` 用于隔离嵌入，调用方必须独占该存储。
  → `runtime/crates/jeeves-app-server/src/lock.rs`
* **刻意** — 每个会话同一时刻只跑一轮：`turn/start` 先占会话，第二个调用方得到指明占用者的 `SESSION_BUSY`。
  这道门是进程内的。`turn/interrupt` 不占会话，总能进去。
  → `runtime/crates/jeeves-app-server/src/server/controls.rs`（`begin_turn`）
* **刻意** — 每会话的写段锁只覆盖一轮里的写段（压缩和尾段）。模型调用段从不持有它，否则会卡住会话并把 interrupt
  挡在门外。锁按需创建，从不移除。
  → `runtime/crates/jeeves-app-server/src/server/controls.rs`（`session_write_lock`）
* **刻意** — SQLite 等锁 10 秒；并发写入者排队而不是失败。不够的话，答案是减少写入者，而不是加长超时。
  → `runtime/crates/jeeves-state/src/db.rs`
* **刻意** — `turn/list`、`turn/steer` 和会话门只看得到本进程内的回合；别的进程里的回合对它们不可见，也不会被转发。

## 执行

`read_file` / `glob` / `grep` 默认开启，绕过执行门：它们的信任边界是进程用户的 OS 权限。模型能读该用户能读的任何文件，
`glob` / `grep` 也能看到这些目录的结构（它们跳过 `.git`、`node_modules`、`target`、`dist`、`build`，因为那是噪音，
不是机密）。`fetch_url` 不经过 `run_command` 沙箱；它的防线是 SSRF 地址检查加外部内容去武器化。它自带的 HTML 转换器
不如 readability 类库；要高质量抽取，请用 `run_command` 配 curl 或 MCP 抓取服务。

* **刻意** — 沙箱内的命令默认离线（`JEEVES_EXEC_NETWORK=1` 放开网络）；写入限于工作目录、临时目录和
  `JEEVES_EXEC_WRITABLE_ROOTS`。Jeeves home 不可写：它装着 `settings.json`（exec 允许表）和 `hooks.json`，
  能写它的 shell 就能给自己加权。等于 home、home 的祖先或 `HOME_CONTROL` 之一的条目，即使列了也会被丢弃。
  → `runtime/crates/jeeves-tools/src/roots.rs`、`exec.rs`（`sandbox_argv`）
* **刻意** — 没有可用沙箱就什么都不跑。Linux 上 `bwrap` 必须存在且能创建命名空间（Ubuntu 24.04 限制了非特权用户命名空间），
  所以可用性通过每进程实际运行一次来探测。
  → `runtime/crates/jeeves-tools/src/exec.rs`（`sandbox_backend`）
* **刻意** — 写工具（`write_file` / `edit`，`JEEVES_WRITE=1`）没有进程沙箱；硬门是可写根，由所选工作目录和适用的额外根确定。
  没选文件夹时什么都不可写。可写根之外的写入在询问审批之前就被拒绝。
  → `runtime/crates/jeeves-tools/src/roots.rs`、`fs_write.rs`
* **刻意** — `JEEVES_WRITE_ALLOW_DIRS` 是「不再询问」清单，不是可写根。
  → `runtime/crates/jeeves-tools/src/fs_write.rs`（`pass_write_gate`）
* **刻意** — 硬拒绝清单是子串匹配加手写的「管道进 shell」检查，所以混淆写法（`bash -c "$(printf …)"`）会漏过。
  它是安全带；真正顶用的是允许表和沙箱。
  → `runtime/crates/jeeves-tools/src/exec.rs`（`HARD_DENY`、`pipes_into_a_shell`）
* **刻意** — `JEEVES_APPROVAL` 取 `never`（默认）或 `on-request`，按模式而不是按工具类别：只有 exec 和写入有门可问。
  → `runtime/crates/jeeves-config/src/lib.rs`
* **刻意** — 「记住这次」有三个范围：`once`、`session`（进程内，重启即失）、`always`（写入 `settings.json`）。
  记下的正是被批准的内容：不带通配符的命令行，或写入的目录。硬拒绝、沙箱和可写边界仍然先于它执行。
* **刻意** — 等待人回答的问题能跨重启保留：它经 `approval/pending` 带 `restored: true` 和更高的 `version` 回来，
  带旧版本的回答会被拒绝。对这类问题回答“是”只是一次性、仅在内存里的授权，由恢复回合再次发问时消耗；它不落库，
  所以在被消耗前进程再次重启就丢了。
  → `runtime/crates/jeeves-app-server/src/approval.rs`、`runtime/crates/jeeves-state/src/grants.rs`
* **部分完成** — 邮件与飞书发送审批会记录范围授权，一次性授权记录为已消费；通用命令和目录的“记住这次”仍写入 `settings.json`，而不是 `action_grants`。
  → `runtime/crates/jeeves-state/src/grants.rs`
  → `runtime/crates/jeeves-app-server/src/approval.rs`
* **部分完成** — `mcp.json`（或 `JEEVES_MCP_CONFIG`）变化后，MCP 服务器在回合之间重新连接，每次重建都会记录新增和移除的工具；进行中的回合沿用旧表。声明 `tools.listChanged` 的服务器在下一回合边界重新查询工具列表（没有空闲通知监听器）。新回合和新子智能体使用刷新过的连接器及 MCP 工具表。`JEEVES_EXEC_ALLOW` 和飞书后台续期配置仍需重启。
  → `runtime/crates/jeeves-app-server/src/server/mcp_live.rs`
* **刻意** — 溢出的命令输出只按时间清理（`spill/` 保留 7 天，启动时清理），不按容量配额。`JEEVES_HOME` 里其他东西都不轮转。
  → `runtime/crates/jeeves-tools/src/exec.rs`（`prune_spill`）
* **刻意** — 后台命令走与前台相同的门。进程内输出环是 64 KiB（溢出丢最旧的字节并说明丢了多少）；输出同时流入
  `<home>/jobs/`，上限 1 MiB。崩溃后，启动对账把仍在运行的作业标为 `interrupted` 并保持其输出可读；绝不自动重跑。
  没有 PTY，所以交互式程序不适合放后台。
  → `runtime/crates/jeeves-app-server/src/jobs.rs`、`jobs_buffer.rs`
* **刻意** — MCP 反向请求（elicitation/sampling）得到明确的「不支持」错误。一个 MCP 连接一次只承载一个在途请求；
  不同服务器并行。熔断器是进程内的：连续 3 次失败后打开 60 秒，不持久化，也没有旋钮。
  → `runtime/crates/jeeves-mcp/src/transport.rs`、`lib.rs`（`Breaker`）
* **刻意** — `jeeves mcp serve` 实现了 `initialize`、`tools/list`、`tools/call`、`resources/list`、`resources/read`（两个只读记忆视图：`jeeves://memory/facts` 与 `jeeves://memory/episodes`）以及`prompts/list` / `prompts/get`（`recall` 与 `digest`）。sampling 和服务端主动通知未实现：这个服务只暴露记忆，没有理由让客户端采样模型或自行推送变更。MCP OAuth 需要手动 `jeeves mcp login <name>`；refresh token 过期后没有后台重新授权。
  → `runtime/crates/jeeves-mcp/src/server.rs`、`oauth.rs`
* **刻意** — 模型调用只对 429/5xx/网络错误重试，最多 `JEEVES_LLM_RETRIES` 次（默认 2），在 30 秒预算内，
  且从不靠切换 provider 来重试。
  → `runtime/crates/jeeves-provider/src/retry.rs`
* **刻意** — 沙箱仅限 Unix（macOS `sandbox-exec`、Linux `bubblewrap`）。原生 Windows 上 `jeeves` 二进制拒绝启动（退出码 2）
  并打印原因；WSL2 是受支持的路径（见 [operations.md](operations.zh.md) 的矩阵）。CI 的 Windows 任务只做构建和单元测试。
  → `runtime/crates/jeeves-cli/src/main.rs`（`windows_blocker`）、`.github/workflows/ci.yml`

## 子智能体、回合与目标

* **刻意** — 子智能体没有审批通道（父级会询问的地方，子级直接被拒绝）、不持久化（`session/list` 里没有 `subagent:*`）、
  不流式输出内部事件、不能再派生子智能体，也拿不到 todo 清单和项目约定。父级只看到一次工具调用和它的结论。
  → `runtime/crates/jeeves-app-server/src/subagent.rs`
* **刻意** — 批量子智能体（`tasks`，最多 4 个）是只读的：它们的工具表里没有 `run_command` / `write_file` / `edit` /
  `apply_patch`，因为「一次一个写」的屏障管不到并发循环。单个委派拿到完整工具表。
  → `runtime/crates/jeeves-tools/src/lib.rs`（`ToolRegistry::readonly`）
* **刻意** — 执行中压缩保留用户的当前指令和最近一批工具往来。过大的单条指令或工具参数仍可能超过窗口，Runtime 会交代已保存的进度并停止。
  [循环退出与收尾](internals/03-model-and-loop.zh.md#314-退出与收尾)
* **刻意** — todo 清单上限 20 项、每项 4000 字符，同一时刻只能有一项 `in_progress`。
  → `runtime/crates/jeeves-tools/src/todo.rs`
* **刻意** — `/goal` 判断器只看对话（便宜模型，无工具，无检索），所以它的结论可能出错；它只能让一轮继续，
  不能让任何东西绕过权限检查。目标只能由人设置（任务行上的 `goalCondition`）。出现终态后循环停止，等人。
  → `runtime/crates/jeeves-app-server/src/goal.rs`
* **刻意** — `steer` 在迭代边界生效；回合结束前一刻补进的话仍会写入 `chat_log`（`meta.steer`）。「做完之后再做这件事」
  用 `turn/follow-up`（持久队列，重启后保留，一次一条）。重启后，不确定的领取保留为 `needs_attention`，从不自动重放。
  排队的 follow-up 只在下一轮开始时才出现在 SSE 流里。
  → `runtime/crates/jeeves-loop/src/lib.rs`（`Steering`）、`runtime/crates/jeeves-state/src/queue.rs`
* **刻意** — `turn/interrupt` 可中断模型调用和运行中的工具。循环开始前的本地记忆查询与历史压缩也会响应取消。
  → `runtime/crates/jeeves-app-server/src/turn/`
* **刻意** — 报告加固（`Human:` / `System:` 前缀和控制标记加反斜杠，外加「这是转录」抬头）是减速带，不是沙箱。
  它不判断是否恶意，也不改变任何权限检查。
  → `runtime/crates/jeeves-tools/src/report.rs`

## 模型路由

只有三档（`fast` / `chat` / `reasoning`）：profile 上没有成本或速度标签，没有视觉档路由（图片跟随该回合的档位，
由 provider 的 `vision` 标志把关）。L1 分类器只看消息文本。同级故障转移需要显式配置，不是自动发生：
`model-routes.json` 里 profile 的 `fallback` 列表按顺序给出备选 profile，仅在主 provider 自身重试后仍遇到
429/5xx/401/402/403、网络错误或流在出现任何文本前断开时启用；已经向用户显示文本后、上下文溢出或其他 4xx 不启用。
若路由解析不到任何可用 profile，回合退回当前 profile 并明说。
→ `runtime/crates/jeeves-provider/src/fallback.rs`、`runtime/crates/jeeves-app-server/src/route.rs`

* **刻意** — 声明的 `tier` 优先于 `fixed` 模式。`model-routes.json` 中没有该档位条目时，退到当前 profile，
  并由 `TurnMeta.routeReason` 明说，而不是藏起来。
  → `runtime/crates/jeeves-app-server/src/route.rs`（`resolved_for_turn`）

## 文件与上传

驾驶舱上传走两条路。非图片文件落到 `<home>/uploads/<date>/`，名字由服务端生成（扩展名经白名单检查），其绝对路径被注入消息；
模型自己决定是否用 `read_file` 读。图片作为真正的图片块发送，事先按 provider 的 `vision` 能力把关，并检查：位于 `<home>`
之内、存在、媒体类型、单张 20 MiB、以及直接从文件头读出的每边 4096 像素上限。

* **未做** — 附件在会话恢复时不会重放：`chat_log` 存的是消息文本，所以恢复后的会话是描述图片而不是重新展示它。
  → `runtime/crates/jeeves-app-server/src/turn/attachments.rs`、`history.rs`
* **刻意** — 任一边超过 4096 像素的图片被拒绝而不是缩小；分片发送才是正确做法，自动切片需要引入图像库。
  → `runtime/crates/jeeves-app-server/src/image_meta.rs`（`MAX_EDGE`）
* **刻意** — `vision` 默认跟随厂商目录行，宁可偏向 `false`。profile 上的声明（`/model add` 的 `--vision`，或
  `models.json` 里的 `"vision": true`）优先。
  → `runtime/crates/jeeves-config/src/models.rs`、`runtime/crates/jeeves-provider/src/lib.rs`

## 记忆

* **边界** — 人工删除和更正会原子地存下抑制摘要。后台固化跳过相同的规范化主题与内容；忘掉整个主题会阻止对该主题的自动提取，
  包括改写。显式的用户记忆写入仍然允许。单条声明的抑制认不出任意改写；这不是对原始对话、备份或人工编辑的 Markdown 页面的擦除，
  摘要也不是匿名保证。抑制表出现之前被删除的事实没有标记。
* **刻意** — 完全重复的内容进不了表：`(subject, lower(trim(content)))` 上的唯一索引让 `add` 返回已有行。近似重复
  （同一事实换种说法）会成为第二行；合并它们没做。
  → `runtime/crates/jeeves-state/src/facts.rs`、`migrations/0008_memory_dedup.sql`
* **刻意** — 记忆类别是固定的五种（`fact` 兜底）；自创的类别落入 `fact`。
  → `runtime/crates/jeeves-state/src/facts.rs`（`KINDS`）
* **刻意** — 临时性表述的过滤是标记词表，不是分类器，遇到不寻常的措辞会漏。
  → `runtime/crates/jeeves-memory/src/consolidation.rs`（`TEMPORARY_MARKERS`）
* **刻意** — Skill 发现是词法的：ASCII 单词加相邻汉字对，至少两处不同的重合。它不懂同义词；skill 目录仍然留给模型做语义选择。
  → `runtime/crates/jeeves-memory/src/skills.rs`
* **刻意** — 数据库来源的记忆页面在召回时与当前持久事实核对；来源缺失、被更正或带时间边界，整页退出，原始文件仍留在磁盘上。
  有效期针对事实，不针对情景或检查点（[约定](protocol.zh.md#记忆有效期与提炼来源)）。
* **边界** — 经 `jeeves memory backfill` 产生的行，其历史工具输出无法恢复：它们标记为 `fidelity: folded`，记忆中心显示折叠标签，
  而不是假装字节还在。

## 上下文

* **刻意** — 滚动摘要存在 `context_rollups`；`chat_log` 保留原始轮次，所以「重读历史」看到的是真实对话。每次滚动把上一份摘要加上新被
  挤出的轮次重新总结，输出预算 700 token；模型失败时退回 600 字符的摘录（提示词里会说明）。反复压缩会丢细节，摘要也不是授权或
  任务承诺的持久记录。
  → `runtime/crates/jeeves-memory/src/compaction.rs`
* **刻意** — 压缩不保留文件操作状态。被淘汰后留下的是滚动摘要加 `[tools used: …]` 折叠；超过 2000 字符的输出，折叠行带有
  `spill/…` 地址，更短的输出或已变化的文件需要重新读。
  → `runtime/crates/jeeves-app-server/src/turn/history.rs`
* **刻意** — 六段仪表只覆盖随会话增长的部分。时钟、todo 面板、能力说明和项目约定（`AGENTS.md` 等，每轮上限 32 KB）
  不计入而放进提示词，所以在 `AGENTS.md` 很胖的仓库里，仪表会低估提示词那么多。
  → `runtime/crates/jeeves-app-server/src/memory_center.rs`、`agents_md.rs`
* **刻意** — token 数用一种 tiktoken 编码跨 provider 估算。估算决定何时压缩，从不决定计费；以 provider 报告的 `usage` 为准。
  校准是一个比值，限制在 0.5–2.0。
  → `runtime/crates/jeeves-provider/src/tokens.rs`、`runtime/crates/jeeves-state/src/chat.rs`（`context_factor`）
* **刻意** — 每回合工具结果预算只处理单个很大的结果；许多中等结果合起来超出上限则不动它们（只打一行 stderr）。
  → `runtime/crates/jeeves-loop/src/budget.rs`
* **刻意** — 运行时对模型和对人都说英文。数据保持写入时的语言（模型输出、会话标题、任务文本、记忆页面、`AGENTS.md`、手改的
  `SOUL.md`）。唯一的切换器是驾驶舱的中/英按钮；服务端不对错误消息做本地化。**例外**（产品决策 7）：模型缺少本轮所需能力时的
  能力拒绝消息（如"你的模型看不了图"）直接用中文，保证意思不被翻译冲淡；这类消息不是通用错误本地化。

## 回合与任务

* **边界** — 会话可命名并绑定工作目录。会话元数据不记录所有者、可见性和状态。在 `sessions` 表出现之前创建的会话工作目录为空。[会话存储](../runtime/crates/jeeves-state/src/chat.rs)
* **边界** — 创建本身不执行任务。仅限人工的 `task/start` 在独立工作会话里排入一次首次执行。本地 worker（1–8 个，默认 1 个）需要进程存活且已配置模型；
  它不是全天在线或分布式的调度器（[独立任务执行](protocol.zh.md#独立任务执行)）。
* **未做** — 只有 `feishu_send` 与 `email_send` 写回执：飞书真实发送后记 `claimed` 的 `feishu_message` 回执，
  再读回消息（`GET /im/v1/messages/{id}`），飞书返回则为 `confirmed`，否则为 `failed`。邮件记
  `email_message`：SMTP 接受则 `confirmed`，永久拒绝则 `failed`，DATA 之后结果未知则保留 `claimed` 且不重试。
  不在任务运行里时两者都发送但不写回执，并在结果里说明。其他工具不写回执。
  → `runtime/crates/jeeves-state/src/receipts.rs`、`runtime/crates/jeeves-connector/src/{feishu,email}.rs`
* **边界** — 「完成」由事实而不是模型决定：`fileExists`、`fileContains`、`jsonEquals`、`toolsOk`、`receiptConfirmed`、`commandSucceeds`、`sourceSupports` 每轮之后检查，
  条件不满足就跳过判断器。没有运行的命令检查（exec 关闭、没有沙箱、被拒绝）报 `skipped:`，算未满足。其他（「报告写得好」）不被接受为条件。`fileContains` 是纯子串，不是正则。
  → `runtime/crates/jeeves-agent/src/verify.rs`
* **边界** — 预算每轮检查一次，在下一次模型调用之前，所以单轮可能超出上限；账本如实报告超出，而不是假装在调用中途切断。
  预算不是 provider 计费的预留。
  → `runtime/crates/jeeves-app-server/src/turn/accounting.rs`
* **边界** — 崩溃后的恢复发生在进程启动时，且只针对比 `JEEVES_RESUME_MAX_AGE`（默认 60 分钟）更新的检查点——有预算且预算未用完的未结束任务的检查点例外，不论多久都恢复；更老的聊天检查点标为 `abandoned`
  （一种状态，不是删除）。通知不重放；错过的人去读聊天记录。人工打断，或回合在进程内出错，同样把检查点标为 `abandoned`，
  所以进程结束后仍为 `open` 的检查点只可能意味着崩溃。
  → `runtime/crates/jeeves-app-server/src/resume.rs`、`turn/finalize.rs`
* **边界** — 重启时 `running` 任务先被标为 `suspended`；有 `open` 检查点的随后恢复。崩溃恢复是自动的，人工暂停不是：
  只有 `task/resume` 能让暂停的任务继续。
  → `runtime/crates/jeeves-app-server/src/resume.rs`
* **边界** — 后台作业默认不挂在任务的账本上，除非作业在任务内启动，此时它携带该任务的 run 身份，任务也可以等待自己的作业。
  → `runtime/crates/jeeves-state/src/jobs.rs`、`runtime/crates/jeeves-app-server/src/task/`

## 连接器与网关

* **边界** — 飞书 token 请求（幂等）会对网络错误、429 与 5xx 做有界指数退避重试（最多 2 次，500ms→8s）；其他 4xx 立即上报。发消息刻意不重试——重发会重复人只批准过一次的动作——且仍没有通用限流。大批量第三方调用不在范围内。
  → `runtime/crates/jeeves-connector/src/retry.rs`、`feishu.rs`
* **边界** — 飞书默认以应用身份（`tenant_access_token`）发送；以个人身份行事需要一次浏览器授权
  （`jeeves connector feishu login`）。该流程没有 PKCE，回调固定为 `127.0.0.1:41766/callback`，必须在控制台登记。
  续期会轮换 refresh token，所以长驻的 app-server 在到期前十分钟续期；所有进程都停了就没人续期，refresh token 过期后需要重新登录。
  → `runtime/crates/jeeves-connector/src/feishu_oauth.rs`
* **刻意** — 连接器允许名单都是精确匹配：没有通配符、正则或域名后缀条目。只有人能改，改动在下一回合生效。
  → `runtime/crates/jeeves-connector/src/config.rs`
* **边界** — 日历是只读 CalDAV/ICS。TZID 必须能由 IANA 数据库解析，自定义 `VTIMEZONE` 定义不被解释。
  重复展开覆盖 `RRULE` 与 `EXDATE`；`RECURRENCE-ID` 覆盖实例和 `EXRULE` 被忽略。邮件只读取 UNSEEN
  邮件的纯文本，不保存附件，每封正文最多保留 16,000 字符。发送是纯文本，不支持附件。邮件发送没有自动重试
  （不同于幂等的飞书 token 请求），所以 SMTP 结果未知时只报告，不重放。
  → `runtime/crates/jeeves-connector/src/{calendar,email}.rs`
* **未做** — Lark 网关从未对接过真实平台，只对假服务测试过。语音网关不存在。
  → `integrations/channels/`

## 运维

* **刻意** — 定时任务按分钟粒度触发，一次认领一个触发。`catch_up` 只补错过且未被认领的触发；未结算的已认领触发可能已经产生了副作用，
  所以从不重放，并阻塞同一任务的后续触发（不影响其他任务），直到复核后用 `jeeves schedule acknowledge <name>` 释放
  （[恢复](operations.zh.md#定时任务恢复与投递)）。
  → `runtime/crates/jeeves-app-server/src/scheduler/`

## 业务形态（任务 API、webhook）

* **刻意** — `/api/run` 和同步 webhook 等回合跑完；「先接受、后投递」是 webhook 的 `callback` 模式。两者都没有作业队列：
  投递失败（或进程先死）就丢了。答案在会话历史里，但不会重新推送。
  → `runtime/crates/jeeves-ops/src/api/stream.rs`（`run_headless`）、`webhooks.rs`
* **刻意** — 重放保护只在 `signature: true` 时存在，新鲜度按本地时钟判断，窗口 ±300 秒。普通 `X-Jeeves-Secret` 模式不拒绝重放。
  → `runtime/crates/jeeves-ops/src/webhooks.rs`（`SIGNATURE_WINDOW`）
* **刻意** — webhook 限流是进程内的每分钟计数器，没有按调用方分桶；重启会清零。
  → `runtime/crates/jeeves-ops/src/webhooks.rs`

## 配置

* **刻意** — `config/write` 持久化到 `<home>/settings.json`，叠加在环境变量之上，所以保存过的补丁在该字段上优先于 `JEEVES_*`，
  直到清除（空字符串清除模型覆盖；空补丁删除该文件）。非法值整体被拒绝。
  → `runtime/crates/jeeves-config/src/lib.rs`（`apply_patch`、`save_patch`）

## 质量体系

* **刻意** — 确定性评测对着脚本化 provider 跑（无密钥、无网络）。真模型评测是两个单独的命令，分数从不阻塞发布：
  `jeeves judge` 用 rubric 给单轮打 0-10 分；`jeeves research` 在未见过的材料上评测研究简报，把引文、审查解码与综合质量
  分开计分。两者都需要密钥，无密钥时诚实跳过。
  → `runtime/crates/jeeves-eval/src/`、`evals/`
* **刻意** — 需要机器上可能没有的东西（python3、shell、可用沙箱）的场景声明 `prereq`，被计为跳过，绝不计为通过。
  → `evals/deterministic/exec.jsonl`

## 崩溃恢复与记账

* **边界** — 工具批次在执行前先持久化意图，检查点写失败就停止执行。恢复从不自动重放结果未知的批次：它停下来等核对。唯一例外：
  崩溃后，若一个批次里所有未应答的调用都是已证实的读取，则把它们结算为失败并给出「interrupted」结果，回合继续。MCP 传输错误和超时
  报告结果未知，重连从不重复失败的调用。前台命令超时、输出读取失败或无退出码终止，同样报告效果未知并停止后续工具；杀掉进程不会撤销它此前的写入。
  非零退出是已知失败，不能满足 `toolsOk`。这是保守的批次级保护，不是端到端的恰好一次执行。
  → `runtime/crates/jeeves-loop/src/lib.rs`、`jeeves-mcp/src/lib.rs`、`jeeves-app-server/src/resume.rs`
* **边界** — 轮次账目有稳定的检查点身份；任务和 run 的总数与累计水位线原子提交，恢复只计入新增部分，记账失败会让任务停下等人复核。
  崩溃前丢失的墙钟时间是近似的，provider 从未报告的用量不会被估成已知花费，已耗尽的预算不能靠新回合或恢复绕过。
* **边界** — 恢复的任务与全新任务用同一个驱动做预算、验收、目标判断和后续轮次。检查失败、预算耗尽、记账失败和判断器失败都变成
  `needs_attention`。任务身份来自检查点所属的 run，而不是会话里最新的任务。存下来的验收条件或预算若格式错误，任务会停下，而不是变成空清单。
* **边界** — 计划以完整、不可变的版本提交，只是建议性的：不改变验收或授权。模型回复被截断时，任务会报告未完成。结构化交接每轮从
  持久记录重建，与滚动摘要无关；损坏、执行上下文不匹配或超过 64,000 字节的交接会停止执行，而不是截断约束。
* **边界** — 任务的作业等待持久化其确切的依赖集合。有未完成后台工作的任务变为 `suspended`（`waiting for background jobs`），
  作业结束后恢复，重启后也一样；未知、被中断或被杀的作业需要复核。人工暂停或取消优先，取消任务时若作业宿主可用则停掉其存活作业。
  等待不授予权限或目标轮次。其他种类的外部事件等待仅限下一节所述。
* **边界** — 任务与目标的停止通知、作业结果和定时任务结算，与其完成回执在同一个事务里提交；回执失败会回滚状态。待发通知在重启时和每 30 秒重试；
  在入队与确认之间崩溃可能重复通知。
* **边界** — 定时触发原子认领；未结算的认领不会被之后的分钟覆盖，已记录的分钟在时钟回拨后也不会重放。报告正文先持久化再落到 outbox，
  已存在的不同内容被保留待复核而不是覆盖，`produced` 表示生成了输出，不代表领域验收。

## 外部输入等待

外部等待每个任务支持一个不可变条件和一个定向事件，载荷为 16 KiB 的 JSON。来源与事件键的匹配是关联，不是发送者认证。没有公开的回调监听、
事件广播、循环条件或自动附件获取。需要宿主授权；声明条件和继续任务都不是模型工具。输入到达不等于已验证完成。持久的人工暂停围栏阻止
迟到的检查点写入和已暂停工作的静默重启；它们不是跨运行时的执行租约。

## 操作核对

操作日志把工具批次与意图和结果检查点原子地记录下来。被中断或不确定的批次保守地保持未决。它不能证明网络传输已发生、不能回滚副作用、不能阻止用新调用 ID
重复的语义相同的动作，也不能补上外部 provider 缺少的幂等性。早于日志的已停止任务检查点，可以在精确快照核对之后重建带标记的待核对操作元数据，
这既不证明有执行前记录，也不证明已传输。人工回执是带出处的证据，不是对远端服务的独立验证。普通聊天的核对是 `turn/operations` / `turn/reconcile`；
跨会话或跨运行时没有全局的外部副作用去重。

## 产物证据

验证指纹描述测量时观察到的字节；它们不是归档副本，也不是锁。读取期间检测到的本地修改会让检查失败，但文件可能在验证之后变化。
子串匹配并不证明研究质量、来源真实、计算正确或远端事务已结算；这些需要领域专用的验收适配器。人工核对的远端效果保持其独立出处，
绝不变成观察到的工具成功。墙钟预算基于观察，不是对文件系统 I/O 的硬中断。

## 无工具回复

能力说明会检查当前工具表是否为空（委派回合用其过滤后的表）；工具表为空时要求纯文本，也不承诺文件搜索。这并不保证遵从：一次真实样本把 DSML 调用标记
当成了回复，一次关系边界样本承诺了持续在线。评估器能检测到观察到的 DSML 标记，但它不是生产输出过滤器，也不覆盖所有格式或虚假承诺。
