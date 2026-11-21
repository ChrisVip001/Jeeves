# 技术架构

[English](architecture.md) | 简体中文

Jeeves 通过每个任务库独占的 Runtime 执行复杂长任务。任务状态持久保存在所选宿主的 `JEEVES_HOME` 中；Runtime 调用已配置的模型服务，并把工具操作交给对应的执行器。
本文描述软件结构；运行与配置见 [operations.md](operations.zh.md) 和
[configuration.md](configuration.zh.md)。逐层的实现教程在 `docs/internals/` 下。

## 设计原则

1. **Rust 持有逻辑与状态。** agent 循环、记忆、工具和 SQLite 状态库都在 Rust 里；
   crate 按职责拆分（目标：每个模块 <500 行）。
2. **协议只有一份定义。** `jeeves-protocol` 是跨语言契约的唯一来源：`ts-rs` 生成
   TypeScript，`schemars` 生成 JSON Schema，生成物入库。
3. **一个引擎，多个外壳。** `jeeves-app-server` 持有整个 turn，并且是每个 `state.db` 的唯一执行 Runtime。记忆 MCP 可独立开库读写记忆，home 导出命令可开库制作一致快照。驾驶舱后端（`jeeves-ops`）把它作为子进程拉起，经 stdio 说换行分隔的 JSON-RPC；
   聊天网关通过驾驶舱的 HTTP 端点（`/api/gateway/rpc`）接入；原生 CLI 命令在进程内嵌入
   同一个运行时。运行时在恢复前先对规范化的数据库路径加 OS 建议锁，所以同一数据库上的
   第二个执行运行时会明确失败。

## 多租户

每个租户有一个工作区和一个活跃的 Rust Runtime。云入口将请求交给
[Go 统一网关](../jeeves-ts/packages/gateway/README.zh.md)，由网关处理用户账号、会话和 Runtime 路由。
私网中的 [Go 控制面](../control-plane/README.zh.md)为每个租户协调一个 Pod 和一块持久卷，并计量平台模型用量。
聊天、事项、检查点、记忆和交付留在 Runtime 的 `JEEVES_HOME`。源码个人部署可通过 `just local-up`
在回环地址运行相同的网关和 Runtime。

引擎侧供宿主依赖的几样东西：

* `GET /healthz` 返回 200 及 `status`、`version`、`protocolVersion`（一次有时限的
  `session/capabilities` 往返），否则 503。
* 容量上限：`JEEVES_MAX_TURNS` 在已有这么多个 turn 在跑时拒绝新 turn；`JEEVES_HOME_MAX_MB`
  在 home 目录超限时拒绝新 turn。两者为 0 即关闭；读取、导出和清理仍可用。
* `jeeves home export <dir>` / `jeeves home import <dir>` 搬家（备份、换机器、本地与云端互转）：
  一份一致的 `state.db` 快照加其余文件，附清单。锁文件、`secrets.json`、`mcp-auth/`、
  `connector-auth/` 不导出；导入后需要重新给出密钥并重新登录。
* SIGTERM（容器运行时发的信号）会让运行中的 turn 最多排空 25 秒。

## 模块职责

`jeeves-app-server`：`server/` 放共享服务状态（`runtime.rs` 启动与恢复、`tools.rs` 工具
装配、`controls.rs` 审批/容量/活动 turn 控制、`settings.rs` 配置读取）；`dispatch.rs` 与
`stdio.rs` 路由 JSON-RPC 方法；`turn/` 是一个 turn（入口、执行、续跑、收尾、历史、提示词
装配、操作核对）；`task/` 是独立任务 worker；`interaction/` 是主会话入口；`scheduler/` 触发
定时任务；`memory_worker.rs`、`ledger_writer.rs`、`memory_center.rs` 分别是后台记忆
worker、账本写入器和记忆中心的只读面。HTTP 处理器在 `jeeves-ops/src/api/` 下，按资源分组。

## crate 依赖图

箭头读作「依赖」（取自各 crate 的 `Cargo.toml`）：

```
jeeves-protocol          契约（TS + JSON Schema 生成物）；无内部依赖
jeeves-state             SQLite + FTS5(trigram)、sqlx 迁移；无内部依赖
jeeves-config            JEEVES_* 变量，启动时读一次                       → protocol
jeeves-provider          12 个提供方、三种线格式 + SSE                  → config, protocol
jeeves-memory            检索、固化、压缩、Skills                       → provider, state
jeeves-agent             确定性助手（验收检查、计划、路径包含）         → protocol, provider, state
jeeves-tools             工具注册表 + 内置工具                          → agent, memory, provider, state
jeeves-loop              agent 循环                                     → provider, state, tools
jeeves-mcp               MCP 客户端 + OAuth + 记忆服务                  → state, tools
jeeves-connector         日历/邮件/飞书连接器                            → config, state, tools
jeeves-app-server        turn、任务、恢复、JSON-RPC                     → 以上全部
jeeves-ops               驾驶舱后端（axum + SSE）；拉起 app-server 子进程 → config, connector, mcp, memory, protocol
jeeves-eval              确定性评测、judge、发布门禁                    → app-server, config, memory, protocol, provider, state, tools
jeeves-cli               `jeeves` 二进制                                   → app-server, ops, eval 及其余
```

依赖单向、从不成环。`jeeves-eval` 通过与所有客户端相同的公开 API（`Server`、`run_turn`、
`install_provider`）驱动内核：评测能黑盒驱动的，任何前端都能。

## 一轮 turn 的数据流

```
用户消息
  → [记忆检索] 事实 + 情节（FTS5，top_k）→ 提示词尾部
  → [Skills] 每轮注入目录（名字 + 一行）；命中才载入正文
  → [提示词装配] 稳定前缀（soul + 模型身份 + 对话规则）
      + 每轮尾部（时钟 · 能力说明 · todo · 项目约定 · 记忆命中 · 各目录 · 摘要）
      前缀在一个会话内逐字节不变：这就是缓存边界
  → [循环] 观察 → 推理 → 行动 → 重复
        · 每次模型调用都与取消令牌（turn/interrupt）竞速
        · 每次工具执行同样竞速；错误以文本返回
        · 最后一步允许的迭代是收尾：撤掉工具，让模型总结
  → [账本] 每行追加（user / assistant / tool_use / tool_result）；
      超过 JEEVES_LEDGER_INLINE_MAX 的载荷落盘到 <home>/ledger/
  → [持久化] 对话 + meta（工具/模型/用量）写入 chat_log
  → [固化] 每 N 轮，小模型提炼事实 + 一条情节
  → [知识包] 后台 worker 把它们写进 <home>/memory/ 并重建索引
  → [轨迹] traces/<日期>.jsonl
  → [通知] turnStarted → textDelta → toolStarted → toolCompleted →
      consolidationCompleted → turnCompleted
```

完整通知集见 [protocol.md](protocol.zh.md#turnstart-的通知序列)。

验收补做与目标续跑会把用户原始委托和本轮反馈一起交给模型。检查点分别保存原始委托和当前轮的模型对话，恢复时可以接续对应轮次，历史中保留用户实际交办的内容。

## 关键机制

### 状态层（jeeves-state）

一个 `state.db`：`facts` / `episodes`（各带 FTS5 trigram 索引和同步触发器）、`chat_log` 与
`sessions`（会话一行记录名字、工作目录和分叉来源；正文仍在 `chat_log`）、只追加的
`ledger`、任务 / 运行 / 检查点 / 操作等表。迁移通过 `sqlx::migrate!` 在编译期嵌入，
二进制自带 schema。无空格文字（中日韩）的查询被切成相邻重叠的双字词，按每行命中个数
排序；其他查询走 FTS5 trigram，并带 LIKE 兜底。记忆后端契约在 `store.rs`：
`SemanticStore` / `EpisodicStore` trait 加 `conformance` 验证；加第二个后端就是实现 trait
并跑同一套测试。

### 记忆层（jeeves-memory）

* **记忆检索**会在新回合查询本地事实和情节。一个索引不可用时仍使用另一个索引的结果。两者均不可用时，回合可以不带记忆继续。
* **固化**：每 N 轮（`JEEVES_CONSOLIDATE_EVERY`）把未固化的对话提炼成事实（标注来源）和
  一条情节；失败从不丢原始日志。
* **压缩**：被挤出工作记忆窗口的轮次折叠进每会话一份的滚动摘要（存于
  `context_rollups`，向前累积，不会从头重算）。摘要模型挂了就退回确定性摘录，淘汰
  从不等于失忆。
* **Skills（程序性记忆）**：`SKILL.md`（Agent Skills 格式）渐进披露：frontmatter 每轮
  扫描，正文只在消息命中时进入提示词。
* **知识包（派生知识）**：后台 worker 把事实提炼成 `<home>/memory/` 下的页面（OKF 形态，
  一个概念一个文件），并对其建 FTS5 索引。提示词带的是目录而不是页面；模型用
  `read_page` 取一页。人可以手改页面，下次 `jeeves memory rebuild` 以文件为准。
* **账本优先**：每个 turn 逐行追加进 `ledger` 表（每行带摘要，超大载荷落盘到
  `<home>/ledger/`），记忆中心从这里读回 turn。

### 上下文预算（模型每轮到底看到什么）

模型每轮看到的一切都在 `build_system` 里装配一次，再由记忆中心的仪表测量：

| 段 | 来源 | 旋钮 |
|---|---|---|
| soul | `SOUL.md`（精简或完整） | `JEEVES_SOUL_MODE` |
| memory | 检索命中 + 知识包目录 + skill 目录 | `JEEVES_RETRIEVAL_TOP_K` |
| skills | 命中本条消息的正文 | 无 |
| tools | 已注册工具的 JSON schema | `JEEVES_TOOL_POLICY=deferred` 可缩小 |
| history | 最近 N 轮，从账本读回 | `JEEVES_HISTORY_TURNS` |
| summary | 滚动压缩摘要 | `JEEVES_COMPACT_THRESHOLD` |

尾部有四样东西刻意不计入六段仪表：时钟、todo 面板和能力说明（各几十个恒定 token），以及
**项目约定**（`agents_md.rs`）。从本轮工作目录向上到外围 `.git`，每个目录按
`AGENTS.override.md`、`AGENTS.md`、`CLAUDE.md` 的顺序取一个文件，从根到内拼接。总量上限
32 KB，由内向外填充：离工作区最近的文件保持完整，外层的被丢弃（只有最内层文件自己就超预算
时才截断，并注明截断）。它放在尾部而不是稳定前缀，因为它随目录变化。子智能体拿不到它。
仪表显示的是随会话增长的部分，所以在 `AGENTS.md` 很胖的仓库里它会低估提示词（见
[limitations](limitations.zh.md#上下文)）。

三样东西防止预算悄悄膨胀：压缩（见上）；工具结果裁剪，每次请求前把最旧的超大结果换成占位
（`JEEVES_TOOL_RESULT_TOTAL_CHARS`，最新 `JEEVES_TOOL_RESULT_KEEP_RECENT` 条保持完整）；以及迭代
预算，约束 turn 本身（`JEEVES_MAX_ITERATIONS`；最后一步是收尾）。

### 工具层（jeeves-tools）

内置工具和 MCP 工具合并进同一个 `ToolRegistry`（确切清单见
[internals/04-tools.md](internals/04-tools.zh.md)，并与代码核对）。契约：`execute` 永不返回 Err。
错误以 `Error: …` 文本回给模型，循环继续；工具失败是模型能纠正的信息，不是崩溃。

### 打断（turn/interrupt）

每个 turn 注册一个取消令牌；模型调用和工具执行通过 `tokio::select!` 与它竞速，落败的
future 被丢弃（HTTP 连接随之关闭）。部分回复如实存下，带 `meta.interrupted = true`。

### 修改操作准入

生产环境的工具注册表跨会话共用一个由宿主注入的修改协调器。文件修改、命令、声明式命令
和保守归类的 MCP 调用共持一个运行时许可；后台命令把它一直持有到进程、输出和账本落定。
读取以及任务/作业的查看与取消保持可用。准入最多等待两秒，忙时返回可见的工具错误。

这是单个运行时内的单写者协调，不是文件系统隔离，也不是分布式租约：独立的 `JEEVES_HOME`
运行时、人工编辑和外部进程仍可能冲突。

### 主会话与任务

Web 和桌面使用常驻的 `jeeves-main` 会话和 `interaction/start`。运行时把人的消息路由为普通
讨论、澄清，或最多四个独立任务，并负责准入、任务目录、分发、重试和继承的权限；客户端渲染
共享的任务控制。默认一个 worker 执行任务，可配置至八个，主会话 turn 保持可用。主提示词带的是有界的
任务进度，而不是 worker 的对话记录。协议细节见
[protocol.md](protocol.zh.md#主交互入口)。

### 评测（jeeves-eval）

确定性评测在进程内用脚本化模型驱动同一个 `run_turn`：离线、0/1、可复现，场景在
`evals/deterministic/*.jsonl`。**除已声明的已知缺口，所有适用用例都必须通过**（`jeeves eval` 的退出码）。即使用例标记了已知缺口，准备或执行出错仍会导致门禁失败。报告写盘失败会输出提示。judge 让真模型
作答、由更便宜的模型按评分细则打 0-10 分；分数只报告，不阻塞发布。报告落在
`eval_report.json`，历史追加到 `eval_runs.jsonl`。

## 安全边界

模型 profile 显式选择凭据来源，见[凭证处理](../SECURITY.zh.md#凭证处理)。响应和日志里只带密钥所在位置的指针。MCP OAuth
令牌落在 `mcp-auth/`（0600，临时文件再原子重命名）。`send_message` 只起草；skill 名强制为
slug 以防路径穿越。完整清单见 [SECURITY.md](../SECURITY.md)。

## 设计决策

### 为什么中文检索需要专门的迁移

FTS5 默认的 `unicode61` 分词器把一串中文当作一个 token，所以「早上」永远匹配不到
「Alex likes morning meetings」。`0002_cjk_trigram` 迁移改用 `trigram`（滑动三字切片），
中文自然就能用；trigram 切不了的最短查询（两字中文词很常见）退回 LIKE 子串扫描。个人记忆
库只有几千行，一次全表 LIKE 是微秒级。

### 为什么生成物要 check-in

「改了 Rust 协议、忘了重新生成」从不报错，只会让前端悄悄读到过期类型。把生成物提交进库，
就让这种漂移出现在 PR diff 里；CI 用 `--check` 把关。

### 为什么生成逻辑只在 test 构建下编译

`jeeves-protocol` 的运行时依赖是 `serde`、`serde_json` 和 `jeeves-protocol-noop-macros`；
`ts-rs` 与 `schemars` 是 dev-dependency：

```rust
#[cfg(test)]      pub(crate) use ts_rs::TS;
#[cfg(not(test))] pub(crate) use jeeves_protocol_noop_macros::TS;   // 空宏
```

所以 `cargo build` 从不碰生成器，二进制里也没有它们。

## 记忆的三层

* **账本就是事实。** `ledger` 表，只追加：每个 turn 的 user / assistant / tool_use /
  tool_result 行，每行都有对规范化载荷的 `digest`。超过 `JEEVES_LEDGER_INLINE_MAX` 的载荷挪到
  `<home>/ledger/…`，行里保留指针、摘要和字节数。任何 turn 都能由它重建。经由
  `jeeves memory backfill` 进来的行带 `fidelity: folded`，这是「此工具输出只剩摘要」的诚实标签。
* **知识包是派生知识。** `<home>/memory/` 下 OKF 形态：每个概念一页 Markdown，frontmatter
  带 `title` / `kind` / `sources`，`index.md` 是目录，`changelog.md` 是历史。人改了某页后，该页
  摘要与索引不再一致，下次 `jeeves memory rebuild` 以**文件**为准。
* **索引是缓存。** 记忆检索使用的 SQLite FTS5。丢了只是重建索引，不会丢事实。

提示词带的是前两层的目录，而不是百科全书：模型看到目录并按需取页（`read_page`），这与
skill 目录、延迟工具表是同一种渐进披露。
