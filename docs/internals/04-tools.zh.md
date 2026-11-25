# ④ 工具：`jeeves-tools`

[English](04-tools.md) | 简体中文

模型能用来行动的一切都在 `runtime/crates/jeeves-tools/`。工具层只认识 trait
（`SubagentRunner`、`ApprovalBroker`、`JobRegistry`、`ExecutionPolicy`、`ResourceCoordinator`、`BrowserHost`），
所有实现都在 `jeeves-app-server`（[06](06-app-server.zh.md)）。线上类型见
[01](01-protocol-and-config.zh.md)，调用工具的循环见 [03](03-model-and-loop.zh.md)，`memory` / `read_page`
背后的记忆见 [05](05-memory.zh.md)。

## 清单：15 个常驻 + 9 个门控

“常驻”指没有任何能力开关控制该工具。`handlers::build_default_with_web()` 安装其中 14 个，
`app-server` 再加 `todo_write`。清单由测试 `default_registry_has_the_expected_tools` 固定，
`scripts/check_tool_docs.py` 把本节与代码对账。

| 分组 | 工具 | 一句话 |
|---|---|---|
| 记忆 | `memory`、`read_page` | `memory` 用 `action` 选操作；`read_page` 按 `kind` 读记忆页或技能正文 |
| 文件（读） | `read_file`、`glob`、`grep`、`find_symbol`、`repo_map` | 只读，不需要 shell |
| 网络 | `fetch_url`、`search_web` | 仅当有可用的搜索后端时才注册 `search_web` |
| 发现 | `find_tools` | 打开延迟加载的工具（[延迟加载](#延迟加载)） |
| 消息 | `send_message` | 写一份草稿文件，从不发送 |
| 待办 | `todo_write` | 会话待办清单 |
| 调度 | `schedule_job`、`list_schedules`、`cancel_schedule` | 存在状态库里的任务 |

门控工具：

| 工具 | 门控 | 缺席意味着 |
|---|---|---|
| `run_command` | `JEEVES_EXEC` | 不能运行任何命令 |
| `job` | `JEEVES_EXEC` | 无法查看后台任务（`action` = output / list / kill） |
| `write_file`、`edit`、`apply_patch` | `JEEVES_WRITE` | 不能修改任何文件 |
| `delegate_task` | `JEEVES_DELEGATE` | 没有子代理 |
| `browser_observe`、`browser_act`、`browser_batch` | 已安装浏览器运行时（`JEEVES_BROWSER_RUNTIME`）且可用 | 没有任务浏览器 |

不计入：连接器工具（`feishu_send`；`calendar_list`；`email_read`；邮件白名单非空时的 `email_send`）、
`task` 工具（每轮在主对话上注册，且只对允许派发的发送者）以及 MCP 工具。连接器读取工具声明 `read`；
未声明 effect 的工具一律是 `write`。

## 注册表契约

`ToolRegistry::execute(ctx, name, args) -> String` 返回文本，从不返回 `Err`。失败是一句给模型读的话：

| 情形 | 结果 |
|---|---|
| 未知名称 | `Error: no tool named '{name}'. Available: …`（若名称与恰好一个已注册工具只差大小写、`_`/`-`/空格分隔、末尾的 `tool` 或泄漏的 `"`/`<` 片段，则执行该工具并在 stderr 记下修复） |
| 参数不符合 schema | `Error: the arguments do not match the {name} schema — …`（最多三条违规，各带字段路径） |
| handler 返回 `Err` | `Error: {name} failed: {e}` |
| 拒绝或业务失败 | handler 返回失败原因，由注册表生成可读文本 |

执行循环使用 `execute_result`，把授权、参数校验和处理函数给出的状态与正文一起读取。成功正文可以以 `Error:` 开头。

`register` 用 `jsonschema` 预编译 `input_schema`；`execute` 先校验，校验失败绝不进入 handler。
无法编译的 schema 跳过校验，并在 stderr 打一行日志。若校验失败仅因顶层的 integer、number、boolean、array 或 object 参数以 JSON 字符串传来（`"5"`、`"true"`、`"[…]"`），`execute` 会解析它，解析后通过校验就执行，并在 stderr 记下修复的键；其他情况仍返回上述错误。`require_str` 会去空白，并拒绝缺失、非字符串和空值；`opt_u32` 拒绝负数。

`execute` 内部顺序：schema 校验，然后宿主的 `ExecutionPolicy::authorize`（见[授权](#授权)），
再对独占工具从 `ResourceCoordinator` 取变更许可（`delegate_task` 除外，它的子循环各自取），
再检查等待期间策略版本是否变化，最后才是 handler。

**独占工具。** `register_exclusive` 标记必须单独运行的工具（循环把它变成屏障）。写入、执行、子代理、浏览器和 MCP 工具都是独占的。
安装了执行策略时，凡不是已证明只读（`permissions::proven_read`）的工具也都视为独占。

**命名。** `register` 遇到已占用的名字会拒绝并在 stderr 说明，绝不替换已有工具。

**只读视图。** `ToolRegistry::readonly()` 返回去掉所有独占工具的副本；`without(name)` 去掉单个工具。

## 每轮装配

启动时 `builtin_tools()` 在 `exec_enabled`、`write_enabled`、`delegate_enabled` 强制为开的前提下注册全部工具；
`delegate_task` 与 `todo_write` 在 `Server` 构造时注册。之后每轮调用 `enabled_tools(&tools, &settings)`，
按**当前**设置移除开关为关的工具：

| 开关 | 移除 |
|---|---|
| `exec_enabled` | `run_command`、`job` |
| `write_enabled` | `write_file`、`edit`、`apply_patch` |
| `delegate_enabled` | `delegate_task` |

开关来自 `JEEVES_EXEC` / `JEEVES_WRITE` / `JEEVES_DELEGATE`，或来自 `settings.json`（通过 `config/write` 修改）；
它们在每轮开始时读取，所以改动对下一轮立即生效，无需重启。关掉的工具根本不在表里，而不是“注册了再拒绝”。
提示词会告诉模型缺了什么（`capability_note`，[06](06-app-server.zh.md)），避免它把缺失的工具当成自己还没找到。

开关之后，本轮的表再套上按会话的激活集（[延迟加载](#延迟加载)）、主对话的 `task` 工具；
若之前的聊天操作尚未处理，则只放行已证明只读的工具。

执行策略、写入策略和浏览器宿主在启动装配表时构建。执行白名单与本会话已批准的命令是共享单元，所以一次批准或一次 `config/write` 修改会作用于下一次调用。
`prune_spill` 在启动时运行一次，删除文件全部超过 7 天的 `<home>/spill/<日期>/` 目录。

## 授权

`ExecutionPolicy` 由 `jeeves-app-server/src/permissions.rs` 实现；每个会话有一个 `PermissionMode`
（`readOnly` / `standard` / `fullAccess`），按会话连同版本号存储。

- 私人主对话按当前会话策略调用已启用的工具。飞书交互仍使用来源限定的工具表。
- **`readOnly`**：拒绝所有不是已证明只读的调用。
- **`standard`**：每个非只读调用经审批代理询问一次（120 秒；预览会对 `token`、`password`、`authorization` 等键脱敏，并限制在 8192 字节内）。
  批准后，该次调用中 handler 自己发起的审批请求都按“是”回答（`ApprovedInvocation`）；硬门仍然生效。
- **`fullAccess`**：策略本身不逐次询问；工具自己的门和 `JEEVES_APPROVAL` 仍然生效。
- **`delegate_task`** 在 `fullAccess` 之外一律拒绝。

已证明只读（`proven_read`）：`browser_observe`、`read_file`、`glob`、`grep`、`search_web`、`fetch_url`、`read_page`、
`list_schedules`、`find_tools`；另有 `action` 为 search / list 的 `memory`，以及 `action` 为 output / list 的 `job`。
MCP 工具和未知工具永远不算已证明只读。

`ApprovalBroker::request` 返回 `ApprovalOutcome`，只有 `Approved` 放行。`Denied`（有人说不）、`TimedOut`（到期没人回答）和 `Withdrawn`（轮已结束、权限已变或这里没人能回答）各自给模型一句不同的拒绝说明，明确的“不”不会被当成审批通道坏了。请求会写明“记住这次”要记录什么（`RememberTarget`）：
`run_command` 记录精确的命令行，写入工具记录目录。

## `memory` 与 `read_page`

| `action` | 参数 | 行为 |
|---|---|---|
| `save` | `subject`、`content`，可选 `kind` | 存一条事实；说明落在哪里（`state.db` 的 facts 表），重复时返回 “Already recorded” |
| `forget` | `subject` | 删除该主题下的所有事实 |
| `search` | `query`、`top_k`（默认 4） | 事实检索 |
| `list` | `limit`（默认 10） | 最近的事实 |
| `update` / `delete` | `id`（update 另需 `content`） | 按 id 修改或删除一条事实 |
| `delete_episode` | `id` | 删除一条经历记录 |
| `create_skill` | `name`、`description`、`body` | 写入 `<home>/skills/<name>/SKILL.md` |

未知 `action` 会返回列出可选项的文本。`create_skill` 把名字转小写并把空格换成 `-`，要求是 slug（`is_slug`，与 `jeeves skill install` 共用），
`SKILL.md` 已存在则拒绝，写入前用 `parse_skill_text` 校验文本。写入前通过 `ApprovalBroker` 请人确认并展示完整草稿；拒绝、超时或无人可答都会返回拒绝文本且不落盘。

`read_page(kind, name)`：`kind=memory` 返回记忆库页面及其 `sources`（已过期或有时效的页面会被报告为已排除）；`kind=skill` 返回技能正文。
记忆包和技能的细节见 [05](05-memory.zh.md)。

## 文件：读、找、写、补丁

**`read_file`**（`path`、`offset` 从 1 起、`limit`）：每页最多 2000 行 / 50 000 字节，按行边界截断，
并带 `[Use offset=N to continue.]`。同一会话里重读同一范围返回“未变化”占位。
`~` 会展开；相对路径相对本轮工作目录解析。非 UTF-8 文件会被拒绝并给出指引。

**`glob`**（`pattern`、`path`）：有 pattern 时返回匹配的路径（`**` 跨目录，`*` 与 `?` 只在一层内，不含 `/` 的 pattern 匹配任意深度的文件名）；
没有时列出 `path`（目录在前，文件带大小，500 项）。**`grep`**（`pattern`、`path`、`glob`、`ignore_case`）：纯文本匹配，输出 `path:line: text`，
行截断到 240 字符，跳过二进制文件和超过 2 000 000 字节的文件。两者都自己遍历目录树，跳过
`.git .hg .svn node_modules target dist build .venv venv vendor .next .turbo .cache .jeeves`，命中 200 条或访问 20 000 个条目就停止，并附 `[Stopped at …]` 说明。

**`find_symbol`**（`symbol`、`path`）按声明名精确查找；**`repo_map`**（`path`）列出声明。两者对常见源码扩展名建立临时 SQLite FTS5 索引，沿用 `grep` 的遍历和匹配上限，另有 32 MiB 源码读取及 30,000 条声明上限。每次调用重建可反映未提交编辑，不写仓库文件。这不是语法解析器：可能漏报或误报声明。

**可写范围**（`roots.rs`，写入工具与 `run_command` 沙箱共用的同一定义）：本轮工作目录加 `JEEVES_EXEC_WRITABLE_ROOTS`。`<home>` 不是可写根。
额外根如果是磁盘根、`<home>`、`<home>` 的祖先，或位于某个 `HOME_CONTROL` 条目内（`settings.json`、`hooks.json`、`mcp.json`、`models.json`、`secrets.json`、
`connectors.json`、`state.db`、`skills`、`memory` 等），就会被丢弃；人选定的工作目录即使是 `<home>` 也予以认可，唯独磁盘根除外。

**写入门**（`fs_write.rs`）：解析符号链接后的路径必须落在某个根内，否则在任何审批之前就拒绝；没选工作目录时什么都写不了。
通过后，在 `JEEVES_APPROVAL=on-request` 下每次写入都问一次，除非其目录已被批准（`write_approved_dirs`，持久化为 `JEEVES_WRITE_ALLOW_DIRS`）；批准不能解锁范围之外的路径。

- **`write_file`**（`path`、`content`）：覆盖文件，创建父目录，最多 1 000 000 字节，不含 NUL 字节；内容相同则报告“nothing was written”。
- **`edit`**（`path`、`old_string`、`new_string`）：`old_string` 必须恰好出现一次，零次或多次都拒绝。匹配分层进行，在第一个有结果的层停下：先按字节精确匹配；再把 CRLF、弯引号、各种破折号和特殊空格折成 ASCII 后匹配；最后按整行匹配并忽略行尾空白。缩进从不忽略。宽松层命中时结果会写明，写进 CRLF 区域的 LF 替换文本会改用 CRLF。未命中时，错误信息要么指出 `new_string` 已在文件中，要么列出 `old_string` 多数行对得上的那段文件，最多 12 行并带行号（`edit_match.rs`）。结果是变更片段（3 行上下文，最多 40 行）。
- **`apply_patch`**（`patch`，最多 512 KB）：`*** Begin Patch` 格式，含 `Update File` / `Add File` / `Delete File`；上下文行和删除行先精确匹配，再按 `edit` 的逐行宽松规则匹配；所有片段先在内存中解析，只有全部匹配才写盘；`Add File` 遇到已存在的文件会拒绝；未知的段标题会拒绝。
- **过期文件检查**（`file_seen.rs`）：`read_file` 和三个写工具按会话记下每个文件的修改时间、大小和内容摘要（8 MiB 以内）。`edit`、`write_file` 或 `apply_patch` 修改已有文件之前，若与记录不符（此后编辑器、用户或格式化工具写过它），就在请求审批之前拒绝写入，并请模型重新读取。只改了时间戳、字节完全相同不算改动。本会话从未读写过的文件没有记录，照常可写。

## `run_command`

参数：`command`、`cwd`、`background`。`vet()` 依次返回 `Allow`、`Deny(reason)` 或 `NeedsApproval`：

1. **硬拒绝列表**，不可配置，也不受审批影响：（小写后的）子串 `sudo `、`doas `、`mkfs`、`dd if=`、`dd of=`、`shutdown`、`reboot`、
   `halt `、`:(){`、`chmod -r 777 /`、`chmod 777 /`、`chown -r /`、`> /dev/disk`、`> /dev/sd`、`diskutil erase`、
   `launchctl unload`、`systemctl disable`、`rm -rf /*`、`rm -rf ~`、`rm -rf $home`、`eval $(`、`history -c`；
   另有 `pipes_into_a_shell`：`curl`/`wget` 段通过管道接到 `sh bash zsh dash python python3 perl ruby`。
2. **白名单**（`JEEVES_EXEC_ALLOW`，最多 64 条规则）：`*` 放行一切，结尾的 `*` 为前缀匹配，其余为整条命令相等。本会话批准过的命令按精确匹配。空表什么都不放行。
   不匹配时，`JEEVES_APPROVAL=never`（默认）拒绝并在消息中列出当前规则，`on-request` 则询问（`JEEVES_APPROVAL_TIMEOUT`，默认 120 秒）。没有代理时拒绝信息会说明没人可问；回答“否”则说明是用户拒绝的，除非用户再次要求，否则不要再申请。
3. **沙箱**：实测而非假定。macOS 需要 `/usr/bin/sandbox-exec`；Linux 需要 `bwrap` 且探测运行 `bwrap … /bin/true` 成功（装了但被 AppArmor 挡住的 `bwrap` 不算）。没有可用沙箱就拒绝运行。

`pass_gate`（第 1–3 道门加审批）与 `spawn_sandboxed` 由前台和后台运行共用。
沙箱只允许写入可写根（本次调用的 `cwd`——绑定了工作目录时必须位于其中——加 `JEEVES_EXEC_WRITABLE_ROOTS`）、
`/tmp`、`/private/tmp` 与临时目录。除非 `JEEVES_EXEC_NETWORK=1`，网络被切断（seatbelt `(deny network*)`，bwrap `--unshare-net`）。
根在写入规则前会先规范化。

执行：stdin 为空，stdout 与 stderr 并发读取，受 `JEEVES_EXEC_TIMEOUT`（默认 30 秒，1–3600）约束；有观察者时 stdout 以增量形式推送。
结果包含退出码与标准输出。非零退出会将调用记为失败。有标准错误输出时，正文在 `--- stderr ---` 下列出。
超时、管道读不出或没有退出码时返回 `Error: … outcome is unknown. Reconcile any committed effects …` 并通知宿主（`on_uncertain`）。
超过 8000 字符的输出被替换为由完整首尾行组成的摘要；完整文本先写入
`<home>/spill/<日期>/command-<时间>-<pid>.txt`，摘要中指明路径。落盘失败时摘要不带路径。
同一套摘要逻辑也用于本轮工具结果预算（[configuration.zh.md](../configuration.zh.md#轮内工具结果预算)）。

## 后台任务

`run_command` 带 `background=true` 时过同样的门，然后 `JobRegistry::spawn_guarded` 启动任务并返回 id。
注册表（`jeeves-app-server/src/jobs.rs`，以状态库为底）为每个任务保留 64 KiB 的输出环。

`job` 工具：`action=output`（`id`、`cursor`、`wait`）、`action=list`、`action=kill`（`id`）。读取从绝对 `cursor` 继续；环溢出时回复会说明丢了多少字节；`wait=true` 最多等 20 秒，然后报告任务仍在运行。
只有启动任务的会话能读取、列出或停止它；其他会话看到的是没有这个任务。输出会擦除保险库里的值。
任务结束时，环中内容写入 `<home>/outbox/jobs/<pid>-<id>.txt`，退出信息记到 stderr，配置了通知器时还会发送通知。

## 凭据保险库

机密按名称保存（大写字母、数字、`_`，最长 40 字符，最多 50 个名称）；值存放在密钥存储（优先系统钥匙串），`<home>/vault.json` 只存名称。
每次启动进程时，`spawn_sandboxed` 把每个值以 `JEEVES_SECRET_<NAME>` 加到那一个进程，前台后台一样，`scrub` 把输出中每个值（4 字符及以上）的原文替换为 `«NAME»`。
值在启动和擦除时读取，所以在设置里新增的机密下一条命令就能用。启用 `run_command` 时，提示词只列名称（`$JEEVES_SECRET_<NAME>`），从不包含值。

## 网络

**`fetch_url`**（`url`）：只允许 http/https；每个解析出的地址都必须是公网地址（回环、私有、链路本地和保留段一律拒绝）；
重定向手动跟随，最多 5 次，每一跳重新检查；超时 20 秒；下载上限 2 MiB，文本上限 50 000 字符，超出会注明截断。
HTML 由本模块自己的转换器转成文本。正文经 `report::as_external` 包装为数据而非指令。

**`search_web`**（`query`、`max_results` 默认 5）：每轮最多调用 `JEEVES_SEARCH_MAX_PER_TURN` 次（默认 20，限制在 1–200）。
生产中 app-server 先让 Parallel 搜索 MCP 处理器接管 `search_web` 与 `fetch_url`（首次使用时连接；失败则冷却 60 秒，由内置路径应答）。
内置路径在有密钥时使用 Tavily（`TAVILY_API_KEY` 或 `JEEVES_SEARCH_API_KEY`）和 Brave（`BRAVE_API_KEY`）；没有密钥时依次用 DuckDuckGo 与 Bing（`JEEVES_SEARCH_DDG=0` / `JEEVES_SEARCH_BING=0` 可关闭）。
没有可用后端时该工具不注册，stderr 会说明原因。结果经 `report::as_search_results` 包装。
配置见 [configuration.zh.md](../configuration.zh.md#搜索与评测)。

## 消息与调度

**`send_message`**（`to`、`body`）写入 `<home>/outbox/<UTC %Y%m%dT%H%M%S>-<to>.txt`，内容为 `To: …` 加正文；`to` 只保留字母数字（其他换成 `-`）并截到 40 字符。
它不发起任何网络调用；回复会说明没有发送。

**`schedule_job`**（`name`、`cron`、`prompt`、`delivery` = outbox / notify / quiet、`catch_up`）、**`list_schedules`**、**`cancel_schedule`**（`name`）与触发任务的运行时共用
`jeeves_agent::schedule` / `cron` 的校验与 cron 语义。名称为小写字母、数字和连字符（最长 64），且不得与技能的 `schedule:` 行重名；回复会给出下一次运行，并说明任务只在 Runtime 运行期间触发。
技能声明的任务不能用该工具取消。
`run_at`（本地 `YYYY-MM-DD HH:MM`，代替 `cron`）表示一次性任务：已过去或超过一年后的时间会被拒绝，存下的 cron 固定在同一分钟仅用于展示，由调度器（`scheduler/once.rs`）触发一次后删除那一行；cron 路径会跳过它。

## 待办清单

`todo_write` 接收 `todos`，每项 `{content, status}`，status 为 `pending` / `in_progress` / `completed` / `cancelled`；省略 `todos` 即读回当前清单。
每次调用替换整张清单。最多 20 项，每项最多 4000 字符，最多一个 `in_progress`；违规则什么都不改。
清单由服务器按会话保存，并在每轮注入系统提示词，而不是存进历史。

## 子代理

`delegate_task` 接收 `task` 或 `tasks`（最多 4 个，两者同时给出时以 `tasks` 为准）和 `result_schema`。
执行器（`jeeves-app-server/src/subagent.rs`）给子代理的是父轮当前已启用的表，去掉 `delegate_task` 与 `todo_write`；没有审批通道（需要审批的一律拒绝），
没有记忆，也没有项目指令。带 schema 的结果会校验并重试一次，仍不符则退回文字。输出经 `report::as_report` 包装，其中会转义形似指令的片段。
批量（`tasks`）并发运行子代理，且各自应用 `readonly()`，因为父轮的屏障无法给四个独立循环排序；单个任务保留完整的表。
结果按请求顺序返回；某个子代理失败不会让整批失败。

## 延迟加载

`JEEVES_TOOL_POLICY=eager`（默认）发送全部 schema；`deferred` 隐藏所有不在 `ALWAYS_ON`（`lib.rs`：`run_command`、`write_file`、`edit`、`apply_patch`、`delegate_task`、`todo_write`、
浏览器工具、`find_tools`、`memory`、`read_page`、`read_file`、`glob`、`grep` 与调度工具）里的工具，直到被激活。`find_tools(query)` 激活名称或描述包含查询中任一词的隐藏工具（一个都不匹配时放出整个隐藏货架），
它们从下一轮起出现。激活集按会话保存在内存中。隐藏 schema 只改变发送的内容，不授予任何权限，开关与审批照常生效。没有按名字调用其他工具的代理工具。

## MCP 工具

`jeeves-mcp` 把已配置服务器的工具变成注册表里名为 `{server}_{tool}` 的工具，名字净化为 `[a-zA-Z0-9_-]` 并限制 64 字符；重名追加 `_2`、`_3` 并在 stderr 打一行。
它们都以独占方式注册。每个连接有一个熔断器：连续失败 3 次后，60 秒内调用直接返回说明，之后放过一次探测。
传输失败会为后续调用重连，绝不重放失败的那次调用。配置见 [configuration.zh.md](../configuration.zh.md)。
