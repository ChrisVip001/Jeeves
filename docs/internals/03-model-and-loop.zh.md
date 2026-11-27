# ③ 模型与循环：`jeeves-provider` 与 `jeeves-loop`

[English](03-model-and-loop.md) | 简体中文

`jeeves-provider` 在 Jeeves 唯一的方言与各厂商 API 之间做翻译。`jeeves-loop` 持有工作记忆，反复「问模型、跑工具、把结果喂回去」，直到该停为止。循环不知道自己在和哪家厂商说话：所有厂商差异都收在 wire 适配器里。

目录：`runtime/crates/jeeves-provider/src/` 与 `runtime/crates/jeeves-loop/src/`。循环由 `jeeves-app-server` 驱动（见 [06-app-server](06-app-server.zh.md)），工具来自 [04-tools](04-tools.zh.md)。

## 3.1 中间方言

`lib.rs` 定义了循环和工具层唯一认识的类型：`Message { role, content: Vec<ContentBlock> }`、`CreateRequest`、`CreateResponse { stop_reason, usage, content }`、`ToolSchema`、`Usage`。

- `ContentBlock` 是内部打标签的枚举（`type`，snake_case），序列化后与 Anthropic 的 content block 逐字节一致，所以 `anthropic.rs` 几乎不用翻译。
- 除 `Text`、`ToolUse`、`ToolResult` 外，还有 `Image`（base64，不带路径），以及必须原样回放的延续块：`Thinking` / `RedactedThinking`（Anthropic wire）、`OpenAiReasoning`（Chat Completions 的 `reasoning_content`）、`ResponseItem`（Responses 的加密 reasoning）。一种 wire 绝不发送另一种 wire 的延续块：Anthropic 与 Chat Completions 遇到 `ResponseItem` 或带 Responses 标记的 `ToolUse.extra` 会拒绝，Anthropic 遇到 `OpenAiReasoning` 也拒绝，而不是悄悄丢掉。
- `ToolUse.extra` 是一个不透明槽位，为 `None` 时序列化跳过。Gemini 思考模型会给 tool call 附 `thought_signature`，下一轮必须原样带回；在 Chat Completions wire 上它以 `extra_content` 往返。
- `StopReason` 带 `#[serde(other)]` 兜底，厂商新增取值不会让一轮崩溃。`UncertainEffect` 由循环设置，从不来自 wire。
- `Usage.input_tokens` 记录完整输入。Anthropic 适配器将新输入、缓存读取和缓存写入相加。OpenAI 的输入总量已经包含缓存。缓存明细单列。每次模型调用保留用途、执行归属、终止原因及实际用量。任务预算按已知的完整输入加输出计入一次，缺少供应商计量的中断请求保留未知用量。

## 3.2 厂商目录

`lib.rs` 里的 `PROVIDERS` 是编译期表，每行一个 `ProviderInfo`：id、`Wire`、`key_env`、`base_url`、默认 `model` 与 `small_model`、近似的 `context_window`、`vision`。当前有哪些厂商及默认值以这张表为准，不要抄进正文。

`Wire` 有三个取值，唯一作用是选 HTTP 客户端：

| Wire | 客户端 | 用于 |
|---|---|---|
| `Anthropic` | `anthropic.rs` | Anthropic，以及提供 Anthropic 兼容端点的厂商 |
| `Responses` | `responses.rs` | OpenAI |
| `OpenAi` | `openai.rs` | Chat Completions 兼容网关，含本地 Ollama |

`key_env` 为空表示本地无需密钥的端点（`needs_key()` 为 false）；`resolve_with` 与 Chat Completions 客户端都据此分支。

`catalog.rs` 在启动时用 `<home>/models-catalog.json` 扩展这张表：已知 id 的行按字段打补丁（`deny_unknown_fields`，拼错会报错而不是悄悄无效）；未知 id 则追加新厂商，至少要有 `model` 和 `wire`。文件或某行损坏只会输出一行 stderr 并保留内置表。合并后的 `Catalog` 启动后不可变。

上下文窗口刻意取保守值：低估只会更早压缩，高估会溢出。`vision` 用来在本地对纯文本厂商给出可读的图片拒绝，而不是远端 400。

## 3.3 解析出客户端

`resolve_with(settings, catalog)` 返回 `Resolved { provider_id, client, model, small_model, vision, context_window }`：

1. 在目录里查 `settings.provider`；未知 id 会列出合法值。
2. **密钥只有一个来源**：`settings.api_key`，这是运行时快照，由 `jeeves-config::models` 按当前 `models.json` 条目的凭据指针（环境变量、钥匙串或回退文件，见 [01-protocol-and-config](01-protocol-and-config.zh.md)）填入。不会隐式回退到 `info.key_env`：一个只因 shell 恰好导出了变量才能跑起来的回合，是最难排查的那类 bug。无需密钥的厂商得到空字符串。缺密钥时返回 `no_key_message`，告诉用户去哪拿 key、该执行哪条 `/model add`。
3. `settings.model` / `small_model` / `base_url` 覆盖目录行；`small_model` 回退到目录行的值，再回退到主模型。`vision` 与 `context_window` 先看 profile 的声明，再看目录行。
4. `capabilities::validate` 在构造任何客户端之前检查声明的模型能力（窗口为正、`reasoningEffort` 合法且要求 Responses wire）。
5. 按 wire 构造客户端，使用 `settings.llm_timeout_secs`（一个客户端一个超时，没有逐请求覆盖）与 `settings.llm_retries`。Anthropic 加 `with_thinking`；Responses 加 `with_reasoning`；DeepSeek 模型的 Chat Completions 客户端会回填 `reasoning_content`。
6. 包一层 `capabilities::Checked`：带工具的请求发给声明了 `tools=false` 的模型，或发给必须走 Responses wire 的模型时，在任何付费调用之前就拒绝。

`Resolved` 手写了 `Debug` 并省略客户端，密钥因此不会进日志。`resolve()` 只按内置表解析，给测试和没有 home 的调用方用。

## 3.4 `Provider` trait

```rust
trait Provider: Send + Sync {
    fn create(&self, req) -> Pin<Box<dyn Future<Output = Result<CreateResponse, ProviderError>> + Send + '_>>;
    fn stream(&self, req, on_text: TextSink) -> Pin<Box<…>> { Box::pin(self.create(req)) }
}
```

只有两个方法；接口越小，接入越容易。Future 手工装箱，是因为 `Resolved` 存的是 `Arc<dyn Provider>`，而 trait 里的 `async fn` 不是对象安全的。`stream` 在文本到达时调用 `on_text`，最后仍返回完整组装好的 `CreateResponse`，所以循环在两条路径上逻辑一致；默认实现退回 `create`。

## 3.5 三种 wire

### Chat Completions（`openai.rs`）

`to_openai` 与 `from_openai` 就是全部翻译，都是纯函数。

- `system_text()` 成为 `messages[0]`。assistant 的文本与其全部 tool call 合并为一条 assistant 消息（无文本时 `content` 为 null）；`extra` 写入该调用的 `extra_content`；`OpenAiReasoning` 写入 `reasoning_content`。
- 带 `ToolResult` 的 user 消息，每个结果变成一条 `role: "tool"` 消息；该消息里的纯文本被丢弃，因为这种 wire 没有同时容纳两者的形状。带图片的普通 user 消息变成数组 content（data URI 部分）。
- token 上限以 `max_completion_tokens` 发送；`tools` 仅在非空时发送。不发 `cache_control`：未知字段在某些网关上是 400。
- `from_openai` 把带 `error` 键的响应体（OpenRouter 用 200 + error 表示限流）或没有 `choices` 的响应体变成 `ProviderError::Api`。`stop_reason` 是推断的：有 tool call 为 `ToolUse`，否则 `EndTurn`。
- 唯一的参数重试在非流式的 `create_inner` 里：响应体提到 `max_completion_tokens` 或 `max_tokens` 的 400，会改用旧名 `max_tokens` 重发一次，照顾老网关。它与 3.6 的退避重试是两回事。
- 流式会加 `stream_options.include_usage`；否则流式的 usage 全是 0，trace 里的成本是假的。content 增量交给 `on_text`；tool call 按 `index` 放进 `BTreeMap`（id/name 覆盖，`arguments` 片段追加），最后才用 `tool_input` 解析。只有收到 `[DONE]` 或 `finish_reason` 才算流完整，否则是 `Incomplete`。`finish_reason` 映射到 `StopReason`（`tool_calls`/`function_call` 为 `ToolUse`，`length` 为 `MaxTokens`，`stop` 为 `EndTurn`）。
- 密钥为空时不发 `Authorization` 头：空的 `Bearer ` 头会让某些网关回 401。本地端点走这条路。

### Anthropic（`anthropic.rs`）

- 请求头是 `x-api-key` 和固定版本的 `anthropic-version`。`tools` 为空时省略（有些端点拒绝空数组）。
- 有 `cache_prefix` 时，`system` 变成两个 text 块，前缀块带 `cache_control: ephemeral`，最后一条消息的最后一个非 thinking 块再加第二个断点（见 3.8）。
- 扩展思考只对配置的主模型开启，且仅当预算不小于 1024 并小于 `max_tokens`；否则请求不带思考发出，而不是失败。带签名的 thinking 块保存在工作记忆里，并在 assistant 回合中排在最前面回传。
- 流式事件：`message_start` 给出 input 与缓存 token，`content_block_start` 打开 thinking/tool 槽位，`content_block_delta` 携带 `text_delta`（交给 `on_text`）、`input_json_delta`、`thinking_delta`、`signature_delta`；`message_delta` 给出停止原因与 output token；`error` 变成 `ProviderError::Api`。没有 `message_stop` 的流是 `Incomplete`。

### Responses（`responses.rs`）

无状态：每个请求都带 `store: false` 与 `include: ["reasoning.encrypted_content"]`，加密的 reasoning 条目以 `ResponseItem` 块返回，由本地工作记忆在下一次请求中回放。reasoning 缺少加密延续的响应是错误。流式把文本与拒绝的增量转给 `on_text`，并从终止事件 `response.completed` / `response.incomplete` 返回组装好的响应；流结束时没有终止事件是 `Incomplete`。

畸形的工具参数从不让整个响应失败：`tool_input` 会先解码（空参数视为 `{}`，被二次编码成字符串的对象再解一次）；仍无法解码时保留这次调用，换上带标记的替代输入，由工具注册表返回一条写明收到内容的 `Error:` 结果，模型据此重新调用。

### SSE 分帧（`sse.rs`）

`data_lines` 把 HTTP 字节流变成每项一个 `data:` 载荷，丢弃 `event:` 行、注释与空行。EOF 时，末尾没有换行的 `data:` 行仍会产出。它不解释 `[DONE]`，由各 wire 自己判断。分帧是各 wire 唯一共享的部分。

## 3.6 重试与错误

`ProviderError` 区分 `Http { status, body, retry_after_secs }`、`Network`、`ContextOverflow`、`Api`（2xx 但响应体不对）、`Parse` 与 `Incomplete`（流在终止事件之前关闭）。所有 400 及以上的 HTTP 状态都经过 `from_http_retry`，溢出检测因此只有一份实现：只有 400/413/422 且响应体匹配短语表（中英文）才算 `ContextOverflow`。

`retry.rs` 只重试 429、5xx、网络错误和 `Incomplete`（`retryable()`）；其他错误重试也会同样失败。不变量：

- **有界**：次数为 `settings.llm_retries`，指数退避从 500 ms 起，单次等待上限 8 秒、总计上限 30 秒；服务端要求的等待会被遵守，但受同样的上限约束：先看 `retry-after-ms`，再看秒数或 HTTP 日期形式的 `Retry-After`。
- **可见**：每次重试都发出 `RetryNotice`。出口是 task-local（`with_note_sink`），而不是 `Provider` 的参数，这样 trait 保持稳定契约，一个回合的重试也不会串到另一个并发回合。没有出口时回退到 stderr。
- **从不换厂商**：那是另一个决策。
- `with_retries` 包住「拿到响应」这一步。`stream_with_retries` 在流式调用以 `Incomplete` 结束、且还没有任何文本送到 `on_text` 时重新发起；已经展示过文本时如实报告中断，因为重试会把文本打印两遍。
- `ContextOverflow` 在这里从不重试；由 app-server 压缩历史并把循环重跑一次（见 [06-app-server](06-app-server.zh.md)）。

## 3.7 token 估算

`tokens.rs` 只回答一个问题：请求离窗口还有多远，供调用方决定何时压缩。计费与 trace 的数字始终来自厂商返回的 `usage`；估算只要数量级对。

- 编码器是 cl100k，词表编进二进制（不联网），是跨厂商的近似，对中文偏高估。提早压缩可以接受；低估（溢出、浪费一次请求）不可以。
- `estimate_text`、`estimate_messages`（含每条消息的开销）与 `estimate_tools`（含每请求的开销）：接入 MCP 工具后，工具声明在上下文里占的比例不小，所以要单独估。图片按字节长度估算并做上下限截断。
- 循环还把最近一次请求的估算值与实测 prompt 大小放进 `LoopResult`；app-server 用二者之比校准下一回合。

## 3.8 提示分段与缓存断点

`CreateRequest` 把系统提示分成 `cache_prefix`（灵魂、模型身份、方法段：同一会话内逐字节不变）与 `system`（每回合的尾部：时钟、待办、记忆命中、技能正文、摘要）。

- Anthropic：在前缀末尾和最后一条消息上放 `cache_control`。其他 wire：`system_text()` 直接拼接，不加字段，依赖它们的自动前缀缓存，而这只要求前缀不动。
- 尾部永远在缓存边界之外；强制压缩只重建尾部，前缀仍然命中。
- 尾部里的时钟是天粒度：精确到小时的时间戳会自己让缓存失效。
- 缓存读取/写入 token 会写入用量账本，命中率因此是可度量的。

## 3.9 `Mock`

`mock.rs` 用脚本化队列实现 `Provider`：`Mock::new`（响应）、`with_outcomes`（成功与失败，例如 `context_overflow()`）、`streaming`（文本按两个字符一块、块间 await 5 ms，让中断有确定的切入点）、`text` / `tool_use` 辅助函数。队列耗尽是错误，而不是复用上一条回复。`received` 记录每个请求，用于断言提示。测试与 eval 借它替换整层网络。

## 3.10 循环：`Turn`

`jeeves_loop::run(Turn)` 接收一次循环所需的全部输入：

- 模型客户端、模型名、拟合后的输出上限、可选的资源预算（目录与剩余上下文如何决定它，见[模型目录](../configuration.zh.md#模型目录models-catalogjson)）；
- `system_prefix` 与 `system`（见 3.8）、`history`（已由 app-server 裁剪）与 `user_message`——二者分开保存，因为来源不同（数据库对请求）——以及只挂在第一条用户消息上的 `user_images`；
- `tools`（`ToolRegistry`）与 `ctx`（`ToolCtx`）；
- 出口与控制：`observer`、`on_text`、`interrupt`、`checkpoint`、`steering`、`resume`；
- `tool_result_budget`（3.16）。

`resume` 从检查点的工作记忆继续。

## 3.11 事件与两个文本出口

`LoopEvent` 有 `Llm`、`Text`、`ToolStart`、`ToolDelta`（仍在运行的工具的实时输出）、`Tool`；工具事件带模型生成的 `tool_use_id`，因为可能有多个调用同时在途。

`on_text` 与 `observer` 是不同的通道。`on_text` 是 UI 的文本流；`observer` 是服务端的事件流（通知、trace）。一个 `Text` 事件和 `on_text` 是同一段增量的两个去处。循环总会挂一个内部记录器；只要任一出口存在就用 `client.stream`，两者都没有则用 `client.create`；调用之后的逻辑完全相同。因为有记录器，即使没有 `on_text`，被中断时也能找回已流出的文本。

## 3.12 一次迭代

每次迭代按顺序：

1. 中断检查（3.13）。
2. 转向（steering）：回合运行期间通过 `turn/steer` 加入的消息，在这个边界作为普通 user 消息并入，已完成的工具结果依然有效。`Steering` 是一个列表、两个读者：循环取走还没见过的；app-server 读取全部以写入转录。
3. 收尾决策（3.14）。
4. `trim_tool_results`（3.16），然后构造 `CreateRequest`（收尾调用不带工具）并记录本地估算。
5. 调用模型，同时与中断赛跑。累加 usage，发出 `Llm`，把 assistant 内容推入。
6. 没有工具调用：返回。否则写检查点、跑工具、推入一条由 `ToolResult` 块组成的 user 消息，并在下一次模型调用**之前**再写一次检查点，崩溃后恰好从这个边界恢复，不会重跑已完成的工具。检查点写失败会中止回合。

独立读取并发执行，每个结果落盘后即可通知客户端，模型按提议顺序收到结果。写入、命令和委派各自独占执行。外部操作效果未知时，循环以 `StopReason::UncertainEffect` 停止，在持久检查点等待核对。

## 3.13 中断

`Interrupt` 是一个原子标志加一个 `Notify`（不用 `CancellationToken`；一个标志不值得引入一个依赖）。`wait()` 先注册 `notified()` 再复查标志，堵住丢失唤醒的竞态。

三个安全点：迭代开头；模型调用（对调用做 `select!`，落败的 future 被丢弃，连接随之关闭，后台不会继续烧 token）；工具执行（每批开始前检查，运行中参与竞速；丢弃 `JoinSet` 会中止在途任务，已完成的仍如实上报）。

中断不是错误：返回 `Ok(LoopResult { interrupted: true, .. })`，带着已流出的文本（没有则是一行占位）、真正跑过的工具调用，以及已完成的迭代数。上层原样持久化。循环之前的压缩不在这场竞速里；见 [limitations](../limitations.zh.md)。

## 3.14 退出与收尾

模型停止调用工具，且没有待接收的追加输入时，循环交付回复。用户取消、无法核实的外部操作、资源额度耗尽、持续重复的工具调用和模型报错都有独立的停止路径。工具请求被模型输出截断时，Runtime 不执行该批请求。回复被输出上限截断（`MaxTokens`，不是回合的 token 预算）时最多续写两次：循环发一条提示，请模型从断处接着写、不要重复，交付的回复把各段拼起来。第三次截断以 `MaxTokens` 结束回合。回复既没有可见文字也没有工具调用就结束回合时（例如只有思考块），循环发一条提示请模型作答；若下一次回复仍为空，回合以一句固定说明结束，告诉用户模型没有返回文字，用户不会看到空白回复。

超过上下文阈值时，模型请求保留当前用户指令和最近一批完整的工具往来，用摘录压缩更早的内容。摘录会被预算截断，所以压缩说明另行确定性保留两类事实：出现过的来源 URL、文件路径与回执标识，以及每个工具最近一次尚未被成功覆盖的失败原因（按工具名去重，同一工具的新失败替换旧的，之后的成功结果清除该条）。这样压缩后的回合不会重试一个已知失败的调用。完整记录仍用来核验检查点和记账。模型在执行中报告上下文溢出时，Runtime 可缩减当前请求并重试一次，接着使用已经落定的工具结果。

## 3.15 循环守卫

`guard.rs` 防止模型因重复调用而停滞。`StallGuard` 是对 `(工具, 参数, 结果)` 哈希按请求顺序做检测的纯检测器，每回合新建（跨回合的重复由 app-server 里的另一套机制处理）。

| 判据 | 触发 |
|---|---|
| 重复 | 同一调用、同一结果连续第 3 次（单次重复可能只是无害的复查） |
| 循环 | 最近 `2p` 次调用两两相同，`p` 取 2 到 4（交替会重置连续计数，所以要单独判；整个窗口都是同一个调用算「重复」，不算循环） |
| 连续失败 | 同一工具不论参数连续第 3 次失败；该工具一次成功即清零 |

重复与循环两行不统计两类调用。成功的轮询（`action=output` 的 `job`、`job_output`，以及名字以 `_poll` 或 `_get_result` 结尾的工具）不计入，因为等待运行中的任务在它推进之前总返回同样的文字。成功的 `write_file`、`edit` 或 `apply_patch` 会清空窗口，所以改完代码再跑同一个测试会重新计数。

命中时，模型会收到 `[guard]` 提醒。过长的重复结果会用简短的引用桩代替。`LoopResult.tool_calls` 与 `Tool` 事件保留真实结果，命中次数与最后一条提示进入 `LoopResult.guard`。连续三批工具调用都触发重复或循环判定时，任务停止并等待核对。连续失败只追加一条提醒，它从不计入上述停止，因为测试命令失败本是正常输出。

提醒给的是针对失败原因的下一步，而不是一句通用的「换条路」。`FailureKind` 只读工具自己的报错文字做确定性分类，不采信模型对失败的转述：

| 原因 | 判据示例 | 提醒给出的下一步 |
|---|---|---|
| `Transient` | 超时、连接重置、429、5xx | 可以再试一次；再失败就如实说明，不要继续重试 |
| `Missing` | no such file、not found、404 | 停止猜名字或路径，先列出或搜索所在位置一次 |
| `Permission` | 403、401、unauthorized、缺凭据或未获批准 | 重试无法解决，直接告诉用户需要什么权限 |
| `Arguments` | schema 不符、缺字段、解析失败 | 重读 schema 和报错，发一个改好的调用，不要随机改值 |
| `State` | conflict、409、precondition、已存在、被锁 | 先重新读取当前状态，再决定是否调用 |
| `Unclassified` | 无法识别，或一轮连续失败里原因不一致 | 保留原有的通用提醒 |

分类按窗口统计：同一工具这一轮连续失败的原因必须一致，混合原因降级为 `Unclassified`，不会按最后一条报错给建议。该工具一次成功即清除记录的原因。

## 3.16 工具结果预算

`run_command` 会截断自己的输出，但 MCP 工具的输出没有上限，所以工具结果是回合内唯一无人看管的增长点。每次请求之前，`trim_tool_results` 检查工作记忆中结果的总大小。超过总上限时，**从最旧的开始**把符合条件的结果换成桩，直到装得下；最新的 `keep_recent` 条与 `run_command` 的结果从不动，且只有超过单条阈值的结果才符合条件。上限与默认值见 [configuration](../configuration.zh.md)。

- 顺序按年龄而不是大小：模型此刻正在推理的结果，不该最先消失。
- 桩（`jeeves_tools::spill::stub`）先把完整原文存盘，**再**裁剪，然后按整行保留头部和尾部，加一条省略说明。目标大小被限制在远低于阈值，否则一条结果会出现「符合条件却无法成桩」。
- 幂等：每次迭代都会运行，已经是桩的结果会被跳过。
- 从不沉默：替换以及仍然超预算的结果各会输出一行。当受保护的结果本身就超过上限时，会明说，而不是悄悄超出预算。
- 总量为 0 时关闭。

## 3.17 `LoopResult` 与不变量

`LoopResult` 携带 `stop_reason`、`reply`、`tool_calls`（`ToolOutcome`：id、名称、输出、耗时）、`iterations`、累加的 `usage`、完整的工作记忆 `messages`（给 trace）、`interrupted`、守卫报告，以及估算与实测的输入 token。

首次准入操作时，同一提议中尚未分发的后续调用记为未执行。每次分发前写入该调用的意图，结算后才能推进依赖它的操作。任务步骤从[持久操作结果](04-tools.zh.md#注册表契约)生成。

不变量：

- 循环只看到中间方言；厂商差异只存在于各 wire。
- 中断、收尾与不确定效果都返回 `Ok(LoopResult)`。厂商错误与检查点失败返回 `Err`，由上层处理（上下文溢出在那里被压缩并重试一次）。
- 每次模型调用的 usage 都被累加；每个 `tool_use` 都有 `tool_result`。
- 重试有界且可见；流式与非流式产出相同的 `CreateResponse`。
- 喂回的工具结果可能被守卫与预算改写；记录下来的结果不会。
