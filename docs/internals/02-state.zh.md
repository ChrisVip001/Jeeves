# ② 存储：`jeeves-state`

[English](02-state.md) | 简体中文

Jeeves 记住的一切都在一个 SQLite 文件里：`<home>/state.db`。本章讲它如何打开、schema 如何组织，以及查询契约。何时写入由服务端与记忆两章决定（[05-memory](05-memory.zh.md)、[06-app-server](06-app-server.zh.md)）。

对客户端而言，数据库访问由 app-server 运行时负责。其他嵌入式运行时（CLI、调度器、MCP 服务）仍可能打开同一个文件，所以必须有 WAL 和连接级的锁等待。执行恢复由 app-server 里另一把操作系统级所有权锁（`lock.rs`）保护，不在本 crate 中。跨进程协调对话轮次不在提供范围内，见 [limitations](../limitations.zh.md)。

## 2.1 打开数据库

`db::open` 创建父目录、把路径解析为绝对路径、按需创建文件、建立连接并运行迁移。

- **绝对路径。** sqlx 把相对路径按进程 cwd 解析，而 cwd 不应决定状态存放在哪；换个目录启动会悄悄新建一个空库。
- **连接选项，而不是 `PRAGMA`。** `busy_timeout`（10 秒）和 `journal_mode = WAL` 设在 `SqliteConnectOptions` 上。`PRAGMA busy_timeout` 是每连接的，只会作用于恰好执行它的那条池内连接。连接池有 8 条连接。`the_pool_waits_for_locks_and_uses_wal` 钉住了这一点。
- **迁移内嵌**（`sqlx::migrate!`），所以二进制自带 schema，运行时不需要任何文件。
- `snapshot` 用 `VACUUM INTO` 写出一致的副本并返回其 schema 版本；`schema_version` 与 `known_schema_version` 用来比较数据库与二进制。

## 2.2 Schema 规则

schema 变更只以 `migrations/` 里带编号的文件存在；当前结构就是这些文件产出的结果，请直接读文件，这里不列清单。规则：

- 追加新的迁移；绝不修改已应用的迁移。
- 数据访问通过 `lib.rs` 导出的类型化句柄（`Facts`、`Chat`、`Ledger`、`Tasks` 等）；其他 crate 不对这些表直接写 SQL。
- 幂等性放在 SQL 里（唯一索引、`INSERT OR IGNORE`、带条件的 `UPDATE … RETURNING`），而不是在 Rust 里先查后写。

表族：

| 表族 | 作用 |
|---|---|
| `facts`、`episodes`（+ `*_fts`）、`memory_*`、`active_facts` / `durable_facts` 视图 | 记忆索引。`<home>/memory` 下的纯文本页是权威，这些行只是索引 |
| `chat_log`、`sessions`、`context_rollups`、`session_*`、`session_queue` | 对话视图、会话元数据、滚动摘要、排队消息、按会话的权限 |
| `ledger`、`ledger_turns` | 每一轮的只追加记录 |
| `agent_tasks`、`task_*`、`task_runs`、`run_*`、`turn_checkpoints`、`jobs`、`job_*` | 长任务、运行、可续跑的轮次状态、后台作业 |
| `tool_operations`、`approval_log`、`interaction_receipts`、`browser_sessions` | 操作、审批与交互记录 |
| `schedule_jobs`、`schedule_state`、`schedule_completions` | 定时任务声明与运行状态 |
| `model_requests`、`historical_turn_usage` | 逐请求计量及旧版逐回合汇总 |

### 事实与情景

- `facts_fts` 和 `episodes_fts` 是外部内容的 FTS5 表（`content='facts'`），由插入/删除/更新触发器保持同步；它们只存索引。
- 分词器是 `trigram`。默认的 `unicode61` 把一串连续汉字当成一个词，无法做子串匹配。代价是索引变大，且短于 3 个字符的词无法命中，这由下面的检索层级兜底。
- `(subject, lower(trim(content)))` 上的唯一索引阻止重复。重复写入时 `Facts::add` 返回已有行和 `false`，因为反复提炼是常态，不是错误。被先前人工编辑抑制的事实，后台写入方（`add_consolidated`）会被拒绝，只有显式记住才能恢复。
- 读取走 `active_facts` 视图，它隐藏超出有效期的事实；`durable_facts` 进一步把有时限的事实从静态镜像里隐藏。
- `facts.embedding` 是 JSON 文本，由记忆层计算；没有向量的行仍参与关键词检索。个人库规模小，在 Rust 里算余弦即可，不需要向量扩展。

### 账本

`chat_log` 是视图（每个说话者一行，工具活动折叠为文本）。`ledger` 是记录：每条用户消息、助手块、工具调用、工具结果和图片，均为结构化。`ledger.rs` 里的规则：

1. 内容没有 UPDATE 路径；更正是新增一行。（`ledger_turns` 里有提炼用的队列状态列，它们不是内容。）
2. 保真度是声明出来的：实时轮次为 `exact`，从 `chat_log` 回填的行为 `folded`，其工具输出当时已被截断。
3. 大载荷放在行外（`<home>/ledger/`，由调用方写入）；行里保留指针、摘要和字节数，因此 `verify` 仍然可用。

条目种类固定在 `KINDS` 中，其他一律在写入时被拒。

## 2.3 并发门

每个先读后做的位置在 SQL 里都有原子门：

| 位置 | 门 | 防止什么 |
|---|---|---|
| 提炼 | `claim_unconsolidated`：一条 `UPDATE … RETURNING` 先标记、后处理 | 两个轮次提炼同一批行（浪费一次模型调用） |
| 提炼失败 | `mark_consolidation_failed` 归还认领并按指数退避安排重试（起始 1 分钟，翻倍，上限 1 小时） | 标记为已完成却什么都没写；或每轮都重试坏批次 |
| `save_rollup` | `WHERE excluded.covered_turns > context_rollups.covered_turns` | 并发压缩时旧摘要覆盖新摘要 |
| `session_queue` | 部分唯一索引，每个会话只允许一行 `claimed` | 两个 worker 认领同一会话 |
| `Facts::add` | 唯一索引（见上） | 重复事实 |

`context_rollups.covered_turns` 是只会前进的水位线：每次压缩把新被挤出窗口的轮次并入已存的摘要，而不是重算历史。

## 2.4 检索

所有方法共用的契约：未命中绝不是错误。没找到返回空 `Vec`，id 不存在返回 `false`，没删到返回 `0`。调用方是模型，它需要的是一个平淡的「没有」。

`Facts::search` 与 `Episodes::search` 按查询选择层级：

1. **CJK 查询**（`fts::cjk_terms`）：把每段 CJK 切成相互重叠的两字词项（其他词整体保留，最多 24 个），用 `LIKE` 取出含任一词项的行，按命中的不同词项数排序，并列时新的在前。一个中文问句到 FTS 里是一个没有任何已存句子包含的长词，它的 trigram 也很少对得上，所以跳过 FTS。
2. **FTS5**（`fts::to_match_expr`）：按非字母数字字符切分，给每个词项加引号，用 `OR` 连接。加引号使表达式始终合法，切分去掉了像 `:` 这类会破坏 FTS5 语法的字符。按 `bm25` 排序。
3. **LIKE 兜底**（`fts::like_pattern`）：FTS 无结果时使用，用于过短而无法做 trigram 的词。去掉 `%` 和 `_`，输入就不能变成通配扫描；不含任何字母数字字符的查询返回空。

个人库只有几千行，第 1、3 层的扫描只需微秒级。

## 2.5 后端契约

`SemanticStore` 与 `EpisodicStore`（`store.rs`）是替换记忆后端的接缝。`conformance.rs` 是每个后端都必须通过的验收套件，内置 SQLite 实现同样要跑（`conformance_tests.rs`）。它钉住了上面的未命中语义，以及 `Facts` 上 trait 调用与固有方法调用的结果一致。

## 2.6 测试

- `state_tests.rs` 与各领域的 `*_tests.rs` 覆盖各个句柄。
- 创建临时数据库的测试调用 `dir.keep()`，而不是让 `TempDir` 被 drop：SQLite 还会写 `-wal` / `-shm` 文件，目录消失会让后续写入失败。

## 2.7 不变量

- schema 只由迁移产生。
- 表只通过类型化句柄访问。
- 未命中不是错误。
- 账本的内容只追加。
