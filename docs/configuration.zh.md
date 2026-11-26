# 配置参考

[English](configuration.md) | 简体中文

## 两层配置

账号、设备和服务连接可在驾驶舱菜单中管理。部署方通过 CLI 或 Runtime 管理接口配置模型、运行限额和工具权限。

环境变量提供启动默认值。`jeeves-config` 在进程启动时读取一次。
Jeeves **不读任何 `.env` 文件**（只有飞书网关入口例外，见下文）——需要 dotenv 的话由启动方自行 source
（`set -a; source .env; set +a`）。运行中的配置变更走 `config/write`
（持久化到 `<home>/settings.json`，重启后仍生效）。

## 多套模型与它们的密钥（`<home>/models.json`）

provider **预适配在代码里**（`jeeves-provider` 的 PROVIDERS，名单见下面 `JEEVES_PROVIDER` 一行），用户要做的
只是选一家、给 key：

```
/model                      看已配好的套（★ 是当前）
/model add ds deepseek deepseek-flash --env DEEPSEEK_API_KEY
/model use ds              切换（不用重发密钥）
```

REPL 的 `/model` 管理 `<home>/models.json` 中的配置，管理客户端使用 `model/*` 协议。密钥优先交给
系统钥匙串，兜底写 `<home>/secrets.json`（0600）；`--env 变量名` 则只读环境变量、
不存下来。`model/save` 的应答会如实说**实际落在哪**（`keychain` / `file` /
`env:VAR`）。切换立即生效。删除活动配置会恢复启动时的模型配置。自定义端点使用模型目录中的供应商来确定通信协议。若模型配置、运行设置或凭据库文件无法读取，需先修复文件才能保存配置。启动时仍可使用默认值。

**这套模型能看图片吗？加配置时顺手说清**，别等发图被拦再回头改：

```
/model add ds deepseek deepseek-v4.1-flash --env DEEPSEEK_API_KEY --vision
```

- 不写 `--vision`：跟随厂商目录行（`models-catalog.json` 里那一家的默认）；
- `--vision`：这套模型看得见图，即使厂商的默认模型是纯文本；
- `--no-vision`：这套是纯文本，即使厂商默认模型看得见图。

同一家既有看图模型也有纯文本模型时（`deepseek-v4.1-flash` 与 `deepseek-v4-pro`），
profile 才是说得准的那一层。等价的文件写法是在 `models.json` 那条里写
`"vision": true`；要让**整个厂商**默认都算，再去改 `models-catalog.json`
（见下文「模型目录」）。

**只有人能改这些**：模型没有 `model/*` 工具，也拿不到密钥。

替换模型密钥时，旧密钥会一直有效，直到 Jeeves 保存新配置。如果运行时中途退出，重启或下次编辑模型配置时会根据本地恢复记录清理未使用的密钥。中断的删除也会在这时继续。如果凭据服务仍忙或拒绝删除，Jeeves 会报告错误并保留记录以便重试。恢复记录只包含配置和存储位置的引用。钥匙串错误只报告操作状态，辅助程序输出和凭据值均不会返回。

## 模型路由（`<home>/model-routes.json`）

不自己选模型，让服务端按轮选 —— 任务档位抽象
（"You don't pick the model, the task does."）：

```json
{
  "mode": "auto",
  "routes": { "reasoning": "ds-r1" }
}
```

- `mode: "fixed"`（默认 —— 文件不存在或损坏同样是 fixed）：每轮都用
  active profile，路由完全不介入。
- `mode: "auto"`：每轮选一个**档位**，再查表映射到 profile id ——
  token 估算超过 chat 档上下文窗口的一半 → `reasoning`；否则 `chat`。
  落在模糊带（≥2k token、未过半窗口）的消息会额外问一次便宜小模型
  （「这是要动脑的活吗？」），失败或含糊一律落回 L0 结论（fail-open）。
- 档位缺项按缺省链回落：`reasoning`/`fast` → `chat` → active profile。
  指向已删除 profile 的映射回落 active，并在 `TurnMeta.routeReason`
  里如实说明。
- `JEEVES_MODEL_ROUTE=auto|fixed` 从环境强制指定模式（CI / 无人值守改不了
  home 文件时用）。
- 编辑入口：REPL 的 `/route`、`/route auto|fixed`、`/route set <档位> <配置名>`
  （文件每轮重读 —— 改完即生效）。
- 路由是**服务端策略**：模型没有换自己模型的工具，人类的 active profile
  永远优先。

路由对自己是透明的：真的发生了选择或回落时，`TurnMeta.routeReason`
记下 `auto:reasoning → ds-r1` 或 `fallback: …`，驾驶舱页脚照实打印。

### 备用模型

同一文件里的 `"fallback": ["profile-id", …]` 按顺序列出主对话模型的备用 profile，
`fixed` 与 `auto` 两种模式都生效。

- 只有当前供应商自己的重试用完后，且错误换一家供应商可能解决时，才换到下一个 profile：
  HTTP 429、5xx、401、402、403，网络失败，或在显示任何文字前就断开的流。上下文超限和其他
  请求错误照旧报告。
- 已有文字显示给用户后，本回合不再切换，否则第二个回答会接在第一个后面。
- 每次切换都发一条 `turn/retry` 通知，原因以 `switching to fallback <id>` 开头。
  同一回合后续调用从答复成功的 profile 开始；下一回合重新先试主 profile。
- 带图片的请求会跳过不支持看图的备用 profile。
- 用量记在实际答复的供应商和模型名下。
- 只有主对话模型会回落；路由分类、摘要、记忆和屏幕调用仍用各自的模型。
- 不存在的 id，以及与当前所用模型重复的 profile，会跳过并在 stderr 打一行。
  保存档位表（`/route`、`model/routes/save`）会保留这个列表。

### 档位可以由任务声明

`task/create` 与 `turn/start` 都接受 `tier`（`fast` / `chat` / `reasoning`）：

- **声明优先于模式**：即使 `mode: "fixed"`，任务或调用方说了 `reasoning`，
  这一轮就走 reasoning 档对应的 profile —— fixed 是默认策略，不是否决权；
- 任务声明的档位挂在任务行上，它的每一轮都沿用，
  不必每轮重说；
- 拼错的档位当场报错，不会静默当成 `chat`（那会让账单落在错误的模型上）。

## 入站 webhook（`<home>/webhooks.json`）

别的系统敲一下，Jeeves 跑一轮（CI 失败、表单提交、支付到账……）。没有这个文件
就**没有这扇门**（404），模型也完全不知道它存在：

```json
{"hooks": [{
  "name": "deploy",
  "secret": "env:JEEVES_HOOK_SECRET",
  "prompt": "线上出事了，按这个事件处理：\n{{payload}}",
  "tier": "reasoning",
  "signature": true,
  "limit": 6,
  "callback": "https://ci.example.com/jeeves/hook-result"
}]}
```

- `POST /api/hook/<name>`，请求头 `X-Jeeves-Secret` 必须与 `secret` 一致
  （`env:VAR` 在请求时读取；读不到 = 拒绝，不退化成空串），错或缺失一律 401；
- 这条路由**故意在 dashboard 的 token 层之外**：敲门的机器不知道
  `JEEVES_OPS_TOKEN`，它只证明自己知道这个 hook 的密钥；反过来，hook 只能
  **递一句话**，读不了任何状态、开不了第二个回合；
- 未知名字、`enabled: false`、没有配置文件 —— 三种情况都是 404
  （门不存在，也不透露它存在过）；
- `{{payload}}` 换成请求体原文；没有占位符就把它接在 prompt 后面
  （静默丢掉 body 更糟）；
- 这一轮仍然是普通的一轮：工具照旧要审批卡，而键盘前没有人 ——
  破坏性工具会在超时后被拒。webhook 不新增任何能力；
- 应答是 `RunResponse`（`turnId` / `reply` / `model` / `provider` /
  `routeReason`），与任务 API 同一个形状（异步模式下它被推到 callback）。

以下示例的 `RUNTIME_ORIGIN` 指向私网 Runtime。在通过 `just local-up` 启动的源码部署中，
从 `<home>/runtime-endpoint.json` 读取系统分配的回环端口。在 Kubernetes 中，使用租户的私网 Runtime Service。

周期委托可接收签名连接器事件。配置 `event_type`、开启 `signature`，并用
`event_fields` 声明可核对和筛选的字段名及类型。支持 `string`、`integer`、
`boolean` 和 `stringArray`。例如来源可以提供原消息引用和附件媒体类型列表。
沿用上述签名请求头。JSON 正文包含 `eventId`、`occurredAt`（带偏移的 ISO 时间）、
`previousCursor`（首个事件为 null）、`cursor` 和 `fields`。唤醒和游标一同
提交后返回 202。出现 409 时，来源应核对上一游标或事件标识。事件模式不会执行
提示词或触发下游回调。

### 三个可选栅栏

- **`signature: true` —— 防重放**。请求必须带 `X-Jeeves-Timestamp`（秒级整数）与
  `X-Jeeves-Signature` = `hex(HMAC-SHA256(secret, "<timestamp>.<body>"))`。
  时间戳与本地时钟相差超过 ±300 秒 → 401；同一个签名在窗口内第二次出现 →
  409。验证方式就是另一侧已有的工具：

  ```bash
  BODY='{"status":"failed"}'; TS=$(date +%s)
  SIG=$(printf '%s' "$TS.$BODY" | openssl dgst -sha256 -hmac "$JEEVES_HOOK_SECRET" -r | cut -d' ' -f1)
  curl -X POST "${RUNTIME_ORIGIN}/api/hook/deploy" \
    -H "x-jeeves-timestamp: $TS" -H "x-jeeves-signature: $SIG" -d "$BODY"
  ```

  **重试要重新签名**（换一个新时间戳）：同一个签名只被接受一次。
  不开 `signature` 时 `X-Jeeves-Secret` 照旧能用 —— 但它只能证明「知道密钥」，
  **不能否认重放**（同一份请求重发就跑第二轮）。这是要开签名的理由。
- **`limit: N` —— 限流**。每个 hook 每分钟最多 N 次，计数只算**通过鉴权**的
  请求（乱试不会烧掉额度）；超了回 429 + `Retry-After` 秒数。进程内计数：
  重启清零，多进程不共享。`limit: 0` 在加载时直接报错（「配了但永不放行」
  是打错字，不是策略）。
- **`callback: "https://…"` —— 异步**。调用方立刻拿到 `202` +
  `{"accepted":true,"hook":…}`，这一轮在后台跑完，结果 POST 到
  该 URL：`{"hook","ok":true,"result":RunResponse}` 或
  `{"hook","ok":false,"error":{code,message}}`。三次投递（间隔 1s、
  2s），**没有队列**：三次都没成（或进程中途退出）就丢了这次投递 —— 但答案
  已经在会话历史里，不会消失。callback 必须是 http(s) URL，否则加载即报错。

## 任务 API（`POST /api/run`）

程序用的「一轮进、一段 JSON 出」：请求体就是 `TurnStartParams`
（`TurnStartParams`），不流式，等这一轮结束才返回：

```bash
curl -sS -X POST "${RUNTIME_ORIGIN}/api/run" \
  -H "Authorization: Bearer $JEEVES_OPS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"message":"把昨天的日志总结成三点","tier":"reasoning"}'
```

- 驾驶舱仍走 SSE（人想看着字一个个出来），这个端点是给 CI / cron /
  另一个 agent 用的；
- 使用与其他私网 Runtime API 相同的 Bearer 令牌；
- `model` 与 `routeReason` 是「档位真的生效了吗」的可见证据。

## 非法值启动即失败

`Settings::validate` 只跑一次，位置在**环境变量与 `<home>/settings.json` 合并
之后**；非法值直接终止进程，并在消息里点名是哪个变量 —— 配错不该静默变成默认值、
过一会儿才以「行为有点怪」的形式暴露出来。同一张边界表（`jeeves-config` 的
`BOUNDS`）也校验 `config/write` 的补丁，所以两个入口不会各说各话。

| 变量 | 范围 |
|---|---|
| `JEEVES_MAX_TOKENS` | 0 或 128 – 200000 |
| `JEEVES_HISTORY_TURNS` | 0 – 1000 |
| `JEEVES_CONTEXT_WINDOW` | 1024 – 10000000 |
| `JEEVES_COMPACT_THRESHOLD` | 0.05 – 0.95 |
| `JEEVES_TOOL_RESULT_TOTAL_CHARS` | 0 – 4000000 |
| `JEEVES_TOOL_RESULT_MAX_CHARS` | 0 – 1000000 |
| `JEEVES_LEDGER_INLINE_MAX` | 1024 – 1048576 |
| `JEEVES_MAX_TURNS` | 0 – 64 |
| `JEEVES_TASK_WORKERS` | 1 – 8 |
| `JEEVES_HOME_MAX_MB` | 0 – 10000000 |
| `JEEVES_TOOL_RESULT_KEEP_RECENT` | 0 – 50 |
| `JEEVES_TURN_TOKEN_BUDGET` | 0 – 2000000 |
| `JEEVES_RESUME_MAX_AGE` | 0 – 10080 |
| `JEEVES_CONSOLIDATE_EVERY` | 1 – 1000 |
| `JEEVES_RETRIEVAL_TOP_K` | 1 – 100 |
| `JEEVES_LLM_TIMEOUT` | 1 – 3600 |
| `JEEVES_LLM_RETRIES` | 0 – 5 |
| `JEEVES_THINKING_BUDGET` | 0 – 200000 |
| `JEEVES_GOAL_MAX_ROUNDS` | 1 – 20 |
| `JEEVES_EXEC_TIMEOUT` | 1 – 3600 |
| `JEEVES_APPROVAL_TIMEOUT` | 5 – 3600 |
| `JEEVES_SKILL_INDEX_TOKENS` | 32 – 2000 |
| `JEEVES_SKILL_BODY_TOKENS` | 64 – 8000 |
| `JEEVES_CONTEXT_WARN_RATIO` | 50 – 99 |

`JEEVES_APPROVAL` 只能是 `never` 或 `on-request`，`JEEVES_SOUL_MODE` 只能是 `auto` / `thin` / `full`，`JEEVES_TOOL_POLICY` 只能是 `eager` / `deferred`。`JEEVES_EXEC_ALLOW` 也在校验范围内：最多 64 条、每条非空、不含换行、不超 200 字符。
**空表是合法的** —— 它表示「什么都不放行」，而那正是默认。

## 核心变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `JEEVES_HOME` | `./.jeeves` | 状态目录：state.db、SOUL.md、skills/、traces/、outbox/、mcp.json、settings.json |
| `JEEVES_PROVIDER` | `anthropic` | 模型厂商：anthropic / openai / deepseek / gemini / kimi / glm / minimax / xai / openrouter / opencode_zen / opencode_go / ollama。**只**是一套都没配时的兜底（本地模型靠它零配置起步）；真正的模型与密钥在 `<home>/models.json` |
| `JEEVES_LLM_TIMEOUT` | `120` | 单次模型调用超时（秒） |
| `JEEVES_LLM_RETRIES` | `2` | 429/5xx/网络抖动时重试几次 —— 指数退避（500ms 起、单次上限 8s、总预算 30s），**每次必发通知**，绝不换厂商；`0` 关闭 |
| `JEEVES_THINKING_BUDGET` | `0` | 为 Anthropic Messages 主模型设置思考方式。`0` 使用模型默认值。当预算至少为 1024 且低于本次请求的输出额度时，支持手动思考的模型使用设定的预算，Claude 5.5 和 MiniMax M3 系列使用自适应思考。思考计入输出。 |
| `JEEVES_GOAL_MAX_ROUNDS` | `5` | 一个目标最多自动续几轮。超了就置 `round-limit` 并如实说明没完成 —— 不伪装完成、也不清掉目标 |

## 行为旋钮

| 变量 | 默认 | 说明 |
|---|---|---|
| `JEEVES_MAX_TOKENS` | `0` | 可选的人工上限。`0` 由[模型目录](#模型目录models-catalogjson)和上下文余量计算 |
| `JEEVES_HISTORY_TURNS` | `12` | 工作记忆滑窗的**上限**：只把最近 N 轮塞进 prompt（更老的折进滚动摘要，不是丢掉） |
| `JEEVES_CONTEXT_WINDOW` | provider 默认 | 覆盖上下文窗口的估算值（本地模型窗口差异极大，表里只是常见默认） |
| `JEEVES_COMPACT_THRESHOLD` | `0.8` | 用到窗口的这个比例就开始压缩 —— token 才是闸门，轮数是上限 |
| `JEEVES_TOOL_RESULT_TOTAL_CHARS` | `200000` | 一轮里所有工具结果的字符总量上限；超了就从**最旧**的开始换成桩，最新的整条留着（`0` 关闭） |
| `JEEVES_TOOL_RESULT_MAX_CHARS` | `30000` | 单条结果超过它才有资格被换桩（`0` 关闭） |
| `JEEVES_LEDGER_INLINE_MAX` | `32768` | 账本里超过这个大小（字节）的载荷改存 `<home>/ledger/`，行里只留指针与摘要 —— 一轮应该还是一行能读的记录 |
| `JEEVES_MAX_TURNS` | `0` | 每个租户同时在跑的回合数上限（跨所有会话）；超了直接报错拒绝，不悄悄排队（`0` = 不限）|
| `JEEVES_TASK_WORKERS` | `1` | 同时并行跑几个用户发起的任务；每个任务持有一份租约，见[任务 worker 与租约](operations.zh.md#任务-worker-与租约) |
| `JEEVES_HOME_MAX_MB` | `0` | MB；home 目录占用达到这个值后拒绝开新回合，读取、导出和清理照常可用（`0` = 不限）|
| `JEEVES_TOOL_RESULT_KEEP_RECENT` | `3` | 最新的几条结果永不换桩（清理从最旧的开始） |
| `JEEVES_TURN_TOKEN_BUDGET` | `0` | 单个 turn 的 token 额度。已用量与下一次预计输入接近预算时收起工具并汇报进度。`0` 为关闭 |
| `JEEVES_RESUME_MAX_AGE` | `60` | 分钟；比这更旧的 `open` checkpoint 在启动时标 `abandoned`，不自动续跑（`0` = 一律续跑） |
| `JEEVES_CONSOLIDATE_EVERY` | `6` | 每 N 轮新对话触发一次 consolidation |
| `JEEVES_RETRIEVAL_TOP_K` | `4` | 每轮最多取回的匹配事实数 |
| `JEEVES_SKILL_DIRS` | — | 冒号分隔的额外技能目录（`<home>/skills` 之外）；里面的技能在设置里是内置项（置灰只读） |
| `JEEVES_MCP_CONFIG` | — | 冒号分隔的额外 mcp.json 文件（`<home>/mcp.json` 之外）；里面的服务器在设置里是内置项（置灰只读），同名时用户的文件赢 |

## 提示形状与上下文计量

| 变量 | 默认 | 作用 |
|---|---|---|
| `JEEVES_SOUL_MODE` | `auto` | `thin` / `full` / `auto`：包在 `SOUL.md` 外面的内置脚手架有多少 |
| `JEEVES_SKILL_INDEX_TOKENS` | `200` | 常驻的技能目录上限（名字 + 一句话） |
| `JEEVES_SKILL_BODY_TOKENS` | `1200` | 单个被选中技能的正文上限；超出部分由 `read_page` 取 |
| `JEEVES_TOOL_POLICY` | `eager` | `deferred` 只把常驻工具放进请求，其余用 `find_tools` 打开 |
| `JEEVES_CONTEXT_WARN_RATIO` | `85` | 占用率到这个百分比时，分层条给一句提示 |

系统提示分成两半：**稳定前缀**（灵魂 + 模型身份）与每轮的尾巴（日粒度时间、待办、
记忆命中、技能目录、滚动摘要）。Anthropic 在前缀末尾打 `cache_control` 断点；有自动
前缀缓存的 wire 只需要这段前缀不要移动。用量账本记 `cache_read_input_tokens` 与
`cache_creation_input_tokens`，命中率是个数字，不是一种信念。

`thin` / `full` 只决定**内置文本**往 `SOUL.md` 里写哪一份、以及「用户没动过的默认值」
变成哪一份 —— 用户改过的 SOUL 在两种模式下都原样返回。`deferred` 永远不会藏起人已经
打开的工具：`run_command`、`write_file`、`edit`、`delegate_task` 常驻工具表，因为
「先发现再授权」是反的。激活集合按会话放在内存里：重启后让模型重新找一次，便宜且诚实。

## 轮内工具结果预算

历史有滑窗和 token 预算，但**轮内**没有 —— `run_command` 自己会截到 8000 字符、
`search_web` 的每条摘要截到 400，而 MCP 工具的输出没人管。一轮里十次 MCP 调用各回 5 万
字符，就是 50 万字符进请求。

打开之后（默认就是打开的，阈值很大），一轮里工具结果的**字符总量**超过
`JEEVES_TOOL_RESULT_TOTAL_CHARS` 时，从最旧的那条开始换成一个「桩」（最新的整条保留）：完整原文写进
`<home>/spill/<日期>/`，上下文里只留**头尾各一半的完整行**和一句说明
（`…（结果共 N 字符，已截断，省略了 M 行；完整输出在 spill/…）`）。

几条规矩：

* **只动够大的**：单条没超过 `JEEVES_TOOL_RESULT_MAX_CHARS` 的结果不换 —— 一百条中等
  结果撑爆预算时，给每条都建一个文件比省下的上下文更贵。那种情况只在 stderr 记
  一行，不改内容。
* **绝不切半行**：半个 JSON、半条日志比少一行更难读，模型还会以为那就是全部。
  首行本身就超预算时如实说明，不硬塞。
* **落盘失败照样成立**：桩里不写路径（「完整输出在 …」而文件不存在，比不写更坏），
  更不会把一次成功的调用变成错误。
* **自预算的工具不碰**：`run_command` 自己就落盘了。
* 换桩是**幂等**的：每轮请求前都过一遍，已经是桩的直接跳过。

## 执行命令

| 变量 | 默认 | 说明 |
|---|---|---|
| `JEEVES_EXEC` | `0` | 开启 `run_command` 工具（关着 = 模型看不见它）。通过 `config/write` 保存的开关优先于环境变量，下一次请求起生效 |
| `JEEVES_WRITE` | `0`（驾驶舱默认 `1`） | 开启 `write_file` / `edit` / `apply_patch` 工具（关着 = 模型看不见它们）。可写范围**只有**本轮选的工作目录与 `JEEVES_EXEC_WRITABLE_ROOTS`；没选目录一处也写不了 |
| `JEEVES_DELEGATE` | `0` | 开启 `delegate_task` 工具（子代理有自己的上下文，且不能再派生）。设置页也有开关，优先级规则同上 |
| `JEEVES_EXEC_ALLOW` | — | 放行表，逗号分隔，支持末尾 `*` 通配（`cargo test,git status,ls *`）。空 = 全部拒绝 |
| `JEEVES_EXEC_TIMEOUT` | `30` | 单条命令的超时（秒） |
| `JEEVES_APPROVAL` | `never`（驾驶舱默认 `on-request`） | 放行表没匹配上时怎么办：`never` = 拒绝，`on-request` = 问一句（没人答 = 拒绝）。对硬拒名单与沙箱无效 |
| `JEEVES_WEB_PUSH_CONTACT` | 未设置 | 操作者的 mailto 或 HTTPS 联系地址，用于 VAPID 身份声明。设置后可注册[任务审批通知](protocol.zh.md#任务审批通知)。 |
| `JEEVES_APPROVAL_TIMEOUT` | `120` | 等回答的秒数，超时按拒绝算 |
| `JEEVES_EXEC_NETWORK` | `0` | 让沙箱内的命令能联网。**默认关** —— 一条被放行的命令不该有能力把数据送出去 |
| `JEEVES_EXEC_WRITABLE_ROOTS` | — | 额外可写的目录（冒号分隔，必须是已存在的绝对路径），典型用途是构建缓存 |
| `JEEVES_WRITE_ALLOW_DIRS` | — | 写文件**不再逐次确认**的目录（冒号分隔，绝对路径）。这是「别再问我」名单，**不是**可写根：边界外照样写不进去 |

通过 `repo` 或 `parentTaskId` 创建的任务，会将文件修改、委派写入、沙箱命令和命令验收限制在自己的 Git 工作目录内。可写构建缓存应放在该目录中；沙箱命令也在其中存放临时文件。普通会话仍使用上表中配置的共享目录。

命令一律在沙箱里跑（macOS `sandbox-exec` / Linux `bubblewrap`），写权限限制在
工作目录、Jeeves 的 home 与临时目录，并且**默认断网**（`JEEVES_EXEC_NETWORK=1`
才联网）；没有沙箱的机器拒绝执行任何命令。硬拒名单
（`sudo`、`mkfs`、把下载内容交给 shell……）不可配置。见
[SECURITY.zh.md](../SECURITY.zh.md)。

## 交互式批准一条命令

默认情况下，没匹配上放行表的命令直接拒绝。`JEEVES_APPROVAL=on-request` 把它变成
一个问题：这一轮发 `approvalRequested`（工具、命令原文、为什么没放行、时限）
然后**等**。终端打 `y`/`a`/其他，驾驶舱渲染一条确认小条；两边都发
`approval/respond`。沉默就是拒绝 —— 超时、答得太晚、没有界面，结局一样。

`remember` 决定这次「是」能管多久，三档：`once`（默认，只这次）、`session`
（本进程记住，不落盘）、`always`（写进 `settings.json`）。终端里 `y` / `s` / `a`
分别对应这三档，驾驶舱的卡片是「拒绝 / 只允许这次 / 本会话总是允许 / 记住这条」。
记下的永远是**被批准的那条命令本身**（不加通配）或写文件时**那个目录**（不记文件
名），并且**答完立刻生效** —— 不用重启进程。记住的规则不比硬拒、沙箱、可写边界大。

## 写文件（`JEEVES_WRITE`）

`JEEVES_WRITE=1` 会注册三个工具：`write_file`（整文件写入，覆盖原内容）、`edit`
（一次精确匹配替换 —— `old_string` 必须在文件里恰好出现一次，0 次或多次都会被
拒绝，返回里带变更 hunk）和 `apply_patch`（多文件多处修改，要么全成要么全不写）。终端与裸 `jeeves app-server` 默认关；**驾驶舱默认开**，因为
那里有人能看着、也能审批（显式设了 `JEEVES_WRITE` / `JEEVES_APPROVAL` 就听人的）。

驾驶舱通过文件夹选择器设置工作目录。文件工具须先绑定工作目录；普通会话还能写入显式配置的额外根。托管代码任务只在自己的 worktree 内写入。进程目录与 Jeeves home 都不会被自动选中。
范围外的写入在审批之前就被硬拒。内容必须是 UTF-8 文本
（不含 NUL 字节）且不超过 1,000,000 字节。驾驶舱里 `JEEVES_APPROVAL=on-request` 是默认值：
写入尚未批准的目录会显示审批请求：拒绝 / 只允许这次 / 本会话总是允许 /
记住这条，没人答就超时按拒绝算。「本会话」只活在内存里，「记住这条」把**目录**
写进 `settings.json`（`writeAllowDirs`），下次启动也不再问 —— 但边界外的路径
照样写不进去，记住的是「别问我」，不是「放宽范围」。

## 任务执行计划（`JEEVES_PLAN`）

`JEEVES_PLAN=1` 时，长任务在**第一轮之前**多一次模型调用，让它先把计划写下来：3–6 步，
短、可执行、能看出做没做完。计划以 `task_steps` 行保存（`kind = "plan"`，`run_id` 为空 ——
计划比某一次执行活得久），并作为一段文本附在本轮消息后面；验收没过时允许**改一次**
计划（最多两版，改到第三版是来回折腾，不是进展）。

**默认关**：多一次模型调用、并且改变了 prompt，属于「由人按下的开关」。
计划永不携带验收条件 —— 那是人定的事实，模型不能靠重新计划把它放松。

## 模型目录（`models-catalog.json`）

二进制里预置了一批厂商（`jeeves-provider`）。要加一家、或调整内置厂商的默认
值而不改代码，写 `<home>/models-catalog.json`：一个 JSON 数组，每行要么**按 id
补丁一个已有厂商**（字段任意子集），要么**新增一家**（`wire`、`model` 必填；
`key_env` 留空或不填 = 本地免 key；`context_window` 缺省 32768）。例子：

```json
[
  { "id": "deepseek", "context_window": 999999 },
  { "id": "mybox", "wire": "openai", "model": "qwen3.5:9b",
    "base_url": "http://127.0.0.1:11434/v1",
    "model_output_limits": { "qwen3.5:9b": 8192 } }
]
```

`model_output_limits` 按该厂商的精确模型 ID 配置输出上限。内置数据取自 OpenCode 随附的 models.dev 快照。目录查不到该模型时，用 32,000 token 作为候选上限，目录上限最多取 256,000。对话、任务和子代理都按当前请求装入的内容计算可用空间，系统提示和工具声明也计入。输入估算加 15% 余量，并预留窗口的 1%。估算余量很小时，沿用 OpenCode 的 1,024 token 下限，仍受模型目录和显式预算约束。

文件**只在启动时读一次**并合并进目录；`/model` 与
`model/list` 看到的都是合并后的结果，`model/save` 也按它校验。未知字段会被
拒绝（悄悄无效的错字比报错更糟）；坏文件或坏行只欠一行 stderr 并退回内置表
——绝不拦启动。改完要重启进程。

不想手工拼这一行 JSON，可以用生成脚本（只有标准库，无第三方依赖）：

```bash
python3 scripts/gen_catalog.py --list        # 看看上游有哪些厂商
python3 scripts/gen_catalog.py glm --model glm-5.3              # 内置 id → 补丁
python3 scripts/gen_catalog.py zhipuai --model glm-5.3 \
  --base-url https://open.bigmodel.cn/api/anthropic \
  --out ~/.jeeves/models-catalog.json                              # 新 id → 新厂商，直接合并写盘
```

数据源是 [models.dev](https://models.dev)（MIT，OpenCode 团队维护的模型元数据库）：
context 上限、是否看图、是否支持工具调用都从它取；wire 和 key_env 由脚本里的
jeeves 映射表决定。脚本会挑「支持工具调用里上下文最大的」当默认模型 —— 那只是
起点，主模型和小模型值得人工确认（候选列表会打在 stderr 上）。

### 内置默认模型

模型配置留空模型 ID 时使用这里的默认值。需要其他版本时，可以在配置里指定模型 ID。

| 厂商 | 主模型 | 辅助模型 |
| --- | --- | --- |
| [Anthropic](https://platform.claude.com/docs/en/models/overview) | `claude-sonnet-5-5` | `claude-haiku-5-5` |
| [OpenAI](https://developers.openai.com/api/docs/guides/model-selection) | `gpt-6.2-sol` | `gpt-6-luna` |
| [OpenRouter](https://openrouter.ai/collections/free-models) | `nvidia/nemotron-3.5-super-vl:free` | `nvidia/nemotron-3.5-lightning:free` |
| [Gemini](https://aistudio.google.com/docs/models) | `gemini-4-neon` | `gemini-4-hydrogen` |
| [DeepSeek](https://api-docs.deepseek.com/quick_start/pricing/) | `deepseek-v4-pro` | `deepseek-flash` |
| [MiniMax](https://platform.minimax.cn/docs/api-reference/text-anthropic-api) | `MiniMax-M3.1-Flash` | `MiniMax-M3` |
| [Kimi](https://platform.kimi.ai/docs/pricing/chat.md) | `kimi-k3.1-pro` | `kimi-k2.7-code` |
| [GLM](https://docs.z.ai/guides/llm/glm-5.3) | `glm-5.3` | `glm-5.3-flash` |
| [xAI](https://docs.x.ai/developers/models) | `grok-4.7` | `grok-4.3` |
| [OpenCode Zen](https://opencode.ai/docs/console/models/) | `deepseek-v4.1-flash-free` | `deepseek-v4.1-flash-free` |
| [OpenCode Go](https://opencode.ai/docs/console/go/) | `deepseek-v4.1-flash` | `deepseek-v4.1-flash` |
| [Ollama](https://ollama.com/library/qwen3.5/tags) | `qwen3.5:9b` | `qwen3.5:4b` |

OpenRouter 和 OpenCode Zen 使用免费模型 ID，OpenCode Go 使用订阅端点。Ollama 保留小体积模型，Runtime 的默认上下文仍是 32,768 token。

## 把活交给子代理

`JEEVES_DELEGATE=1` 会多出 `delegate_task`：把一件自成一体的活交给子代理，只把结论
带回来 —— 适合「这件事会往对话里灌一堆检索与阅读」的场合，或者你想让它在一个
干净上下文里做。子代理拿到的是：

* 空历史 + 一份简短 system（人格 + 一句「只干这一件事」），不带检索、不带技能、
  不带摘要；
* 父轮工具表**去掉 `delegate_task`** 的副本 —— 再派一层不是「被拒绝」，而是根本
  不存在这个选项；
* 与主 Runtime 共用模型目录和上下文输出策略，另有独立的 token 预算；
* 没有交互批准通道，也没有向你提问的途径。

结论回来时带着它用过的工具，**失败的调用会被标出来** —— 父轮因此能判断这份结论
是不是建立在一次失败之上。子代理的对话不留档。它与父轮同进程、同一份状态，除了
上下文之外没有别的隔离。

## 本地推理（Ollama）

`JEEVES_PROVIDER=ollama` 完全跑在这台机器上：不要 key、不要网络、对话不出本机。
装好 [Ollama](https://ollama.com) 之后：

```bash
ollama pull qwen3.5:9b     # 主模型
ollama pull qwen3.5:4b     # 固化记忆用的小模型
JEEVES_PROVIDER=ollama jeeves  # 不需要任何 API key
```

Ollama 暴露的是 OpenAI 兼容端点（`http://127.0.0.1:11434/v1`），所以复用
云端 provider 同一条 wire。LM Studio 与 vLLM 形状相同——配一套指过去即可
（`--url` 给端点、模型名写进 `--model`）。零配置的本地起步：

```bash
JEEVES_PROVIDER=ollama jeeves          # 或：/model add 本地 ollama qwen3.5:27b
```

这是隐私路径、零成本路径，也是断网（或 key 失效）时照样能用的路径。
Ollama 没起来时报的是网络错误——`ollama serve` 把它启动起来。

## 各厂商的 key 变量

Jeeves **不会**自动去读这些变量——密钥只来自你配置进模型的那套（`--env 变量名`
是显式声明「读这一个」）。变量名照旧是各家的习惯名：
`ANTHROPIC_API_KEY`、`OPENAI_API_KEY`、`DEEPSEEK_API_KEY`、
`GEMINI_API_KEY`、`MOONSHOT_API_KEY`（kimi）、`ZHIPU_API_KEY`（glm）、
`MINIMAX_API_KEY`、`XAI_API_KEY`、`OPENROUTER_API_KEY`、
`OPENCODE_ZEN_API_KEY`、`OPENCODE_GO_API_KEY`。

没配（或配的那套缺 key）时报 `PROVIDER_ERROR (-32000)`，错误信息直接给出
`/model add ...` 这条命令。

## 搜索与评测

| 变量 | 说明 |
|---|---|
| `JEEVES_SEARCH_MCP` | 默认 `1`：Parallel MCP 作为主搜索及阅读服务；设 `0` 仅使用内置工具 |
| `TAVILY_API_KEY` / `JEEVES_SEARCH_API_KEY` | 内置兜底中的 Tavily（无 key 时默认 DuckDuckGo Lite，再尝试 Bing HTML） |
| `BRAVE_API_KEY` | 链上第二个付费后端（Tavily 之后尝试） |
| `JEEVES_SEARCH_MAX_PER_TURN` | 每轮 `search_web` 上限（默认 20；超过 200 按 200 算，0 或无法解析回落到 20） |
| `JEEVES_SEARCH_BING` | `0` 关闭免 key 的 Bing 后备引擎（默认开） |
| `JEEVES_SEARCH_DDG` | `0` 关闭免 key 的 DuckDuckGo Lite（默认开，无 key 时优先尝试） |
| `JEEVES_JUDGE_MODEL` | `jeeves judge` 的裁判模型（默认用 small model；裁判不是选手） |

HTTPS 连接使用 Rustls，支持内置和操作系统信任根。在受管环境中，请按操作系统要求安装组织的可信代理 CA，或使用平台支持的证书包（例如在 Linux 上用 `SSL_CERT_FILE` 指向可信 PEM 证书包）。证书和主机名校验仍开启，不会绕过网络策略或公开 URL 校验。

生产运行时保留 `search_web`、`fetch_url` 两个入口，内部优先使用官方
`https://mcp.parallel.ai/mcp` 的搜索与抓取操作。此托管服务独立于用户
`mcp.json` 配置，首次调用网页工具时才连接，启动时不联网；采用匿名访问，
不会自动使用付费搜索凭据。免费服务有限流，会收到本次提交的查询或 URL。

一次服务尝试（含连接、发现工具）最多 20 秒，等待繁忙连接也最多 20 秒。
连接、工具或响应格式故障使服务冷却 60 秒；失败会明确提示并回退到内置工具。
每轮搜索上限在两条路径发请求前统一检查，每次逻辑调用计一次，兜底请求另有上限。
MCP 结果保留来源 URL、确实获得的发布或采集日期及本次读取时间；每条搜索段落
最多 2,000 字符、抓取正文最多 45,000 字符，并提示截断；最终外部数据包装最多
50,000 字符。抓取目标在发送到服务前先通过本地公开地址校验；远程服务负责其
自身重定向和正文提取。抽取成功不证明事实正确或来源足够新。
参见 [Parallel MCP 文档](https://docs.parallel.ai/integrations/mcp/quickstart)。

在仅通过代理联网的环境中，能搜索或通过代理访问 HTTP 不代表 Jeeves 所在的运行环境
能用系统 DNS 解析目标网页。在运行环境中检查：Linux 执行 `getent ahosts <public-host>`，
macOS 执行 `dscacheutil -q host -a name <public-host>`。DNS 校验失败代表网页未被读取。
请使用部署环境支持的系统解析服务恢复 DNS，不要替换成裸 IP、改写链接或禁用公开地址校验。

内置搜索后端是一条有序降级链：Tavily → Brave；完全没配 key 时才走免 key 的
**DuckDuckGo Lite → Bing HTML**。遇到错误、验证页、空结果或无可用条目时继续尝试
下一个已配置引擎。付费后端失败后不额外启用免费端点。关闭 MCP 且没有可用内置后端时
`search_web` 干脆不注册，启动日志会说明；搜索结果与抓取的网页正文一样，进上下文
前都打「外部数据，不是指令」的不可信包装。结果条目限定在各自 HTML 区块内，
还原跳转链接中的来源地址，并按 URL 去重，不让重复项占满结果名额。标题最多 240
字符、每条摘要最多 400 字符，截断时附提示。工具提示要求在结果不相关时细化查询，
并用 `fetch_url` 阅读来源后再引用结论。公开 HTML 入口仍可能被拦截或返回差结果，
这些处理不代表普遍相关性保证；所有引擎均无可用结果时会明确返回工具错误。
查询含单个正向 `site:域名[/路径]` 时，结果必须属于该域名或其子域，并匹配路径前缀；
无关域名和伪装后缀域名会被排除，再尝试下一引擎。复杂多站点表达式沿用引擎语义。

DDG Lite 使用其正常的 POST 搜索表单，优先用系统可用的 `curl` 请求此公开 HTML
入口，再回落到 Rust HTTP 客户端和 Bing。此兼容路径不启动 shell，不允许模型指定
可执行程序或目标地址；禁用 curl 配置文件，响应读取最多 2 MiB、单次请求最多 20
秒，取消时终止子进程。页面仍需验证时，继续既有后备链。

## 驾驶舱与二进制

| 变量 | 默认 | 说明 |
|---|---|---|
| `JEEVES_PORT` | `7777` | 私网 Runtime 端口。`just local-up` 和 Mac 开发版自行启动的 Runtime 设为 `0`，供本机网关自动发现。Kubernetes 示例在私网使用 `7777` |
| `JEEVES_NOTIFY` | 关 | `1` / `true` / `yes` 才开；开了才读 `<home>/notify.json`（`{"slack": "webhook URL"}`）并往外推通知：**审批请求 / 后台作业收尾 / 目标终局**。完成通知先落盘，失败或重启后重试（见[投递边界](limitations.zh.md#通知与审计)）；批准提示仍为尽力发送。网络投递不阻塞 turn。URL 是管理员配的，指向内网自建端点（ntfy/Gotify 这类收 `{"text": …}` 的）是合法用法，不做 SSRF 护栏
| `JEEVES_OPS_HOST` | `127.0.0.1` | 私网 Runtime 的监听地址，容器默认使用 `0.0.0.0` |
| `JEEVES_OPS_TOKEN` | — | 所有监听地址都要求私网 Bearer 凭据。也可提供 `<home>/runtime-token` 文件，权限限所有者。网关在服务端附加凭据，浏览器使用登录会话 |
| `JEEVES_DISPATCHERS` | — | 逗号分隔的聊天发送者 id（飞书 `open_id`），只有他们能在群里派任务。其他人的消息照常回答，但没有 `task` 工具；留空表示谁都不行。本地视图不受影响。不在 `config/read` 里 |
| `JEEVES_BIN` | — | TS 客户端与你自己的脚本寻找 `jeeves` 二进制的显式路径 |
| `JEEVES_PERSONAL_HOME` | `~/.local/share/jeeves-personal` | `just local-up` 的账号与 Runtime 数据目录 |
| `JEEVES_PERSONAL_PORT` | `8743` | `just local-up` 的 Go 网关回环端口 |
| `JEEVES_PERSONAL_BROWSER` | `1` | `just local-up` 自动打开浏览器。完成首次设置后可设为 `0`，用于无人值守启动 |
| `JEEVES_WEB_GATEWAY_URL` | — | 开发版 Mac App 打开的 Go 网关地址 |
| `JEEVES_DESKTOP_BINARY` | — | 只有桌面壳的调试构建读：要启动的 `jeeves` 二进制绝对路径（默认 `target/debug/jeeves`） |

## 连接器（`<home>/connectors.json`）

「草稿，永不发送」是默认；连接器是唯一的例外，并且被三道栅栏围住：

1. **不开就没有。** 没有 `connectors.json` → 没有任何连接器工具，模型连名字都看不见。
2. **默认还是草稿。** 配好连接器但 `"live": false`（默认）时，消息只写进
   `<home>/outbox/`，不出机器。
3. **每次实际发送问一次。** 审批卡写清收件人、主题和正文；拒绝/超时 = 不发。
   白名单是硬边界，**留空 = 谁都不许**。邮件授权绑定账号、收件人和确切内容；
   `once` 授权用过立即作废。

```json
{
  "feishu": {
    "app_id": "cli_a1b2c3",
    "app_secret": "env:FEISHU_APP_SECRET",
    "receive_id_type": "open_id",
    "allow_recipients": ["ou_xxx", "oc_yyy"],
    "live": false
  }
}
```

| 字段 | 说明 |
|---|---|
| `app_secret` | `env:VAR` 或 `SecretStore` 条目名（钥匙串优先，回退 0600 明文并如实报告） |
| `allow_recipients` | `open_id` / `chat_id` 精确匹配 |
| `live` | 只有 `true` 才会真的发送；从 `false` 改成 `true` 是人的动作 |
| `receive_id_type` | `open_id`（成员）或 `chat_id`（群） |
| `base_url` | 飞书自建/私有部署时改这里，默认 `https://open.feishu.cn` |
| `as` | 飞书发送身份：`app`（默认）或 `user`（需先 `jeeves connector feishu login`） |

人的两个命令（终端直接发不弹审批卡 —— 敲命令的人就是批准人）：

```bash
jeeves connector list
jeeves connector feishu --to ou_xxx --text 正文 [--dry-run]
```

模型侧工具是 `feishu_send`，只在连接器启用且白名单非空时注册。
`connectors.json` 里的未知键会被拒绝（`allow_recipents` 这种笔误不能悄悄变成「谁都不许」）。

### 日历与邮件

日历只读。CalDAV 集合走 REPORT；以 `.ics` 结尾的 URL 直接 GET。重复事件按 IANA
时区展开，所以每周事件跨夏令时仍保持墙上时间；全天事件保持日期不变。

```json
{
  "calendar": {
    "url": "https://caldav.example.com/calendars/alice/",
    "username": "alice@example.com",
    "password": "env:CALDAV_PASSWORD",
    "timezone": "Asia/Shanghai"
  },
  "email": {
    "username": "alice@example.com",
    "password": "env:IMAP_PASSWORD",
    "from": "Alice <alice@example.com>",
    "imap": {"host": "imap.example.com", "port": 993, "security": "tls"},
    "smtp": {"host": "smtp.example.com", "port": 587, "security": "starttls"},
    "allow_recipients": ["bob@example.com"],
    "live": false
  }
}
```

| 字段 | 说明 |
|---|---|
| `calendar.url` | CalDAV 集合或直接 `.ics` URL |
| `calendar.timezone` | IANA 显示时区；也用于解释 floating 与全天事件的开始时间 |
| `email.imap` / `email.smtp` | host、port 与 `tls` / `starttls` / `plain`；plain 只给本地假服务，绝不自动选择 |
| `email.allow_recipients` | 收件地址精确匹配；留空仍保留 `email_read`，但不注册 `email_send` |
| `email.live` | `false` 只把 JSON 草稿写进 `<home>/outbox/`，从不连接 SMTP |
| 凭据 | `env:VAR` 或 `SecretStore` 条目名；应答与能力卡只带 `secretPointer` |

任务邮件的完整信封保存在本地数据库中。在主聊天说「刚才那个先别发，改成草稿」。若有多个候选，在回复中附上列出的动作 ID 和改为草稿的指令。排队中的邮件会转为一份持久草稿。任务有独立执行会话时，同一事务也把草稿写入任务对话。发送器已领取时，主聊天显示记录的结果。同一任务、账号和信封在重启后仍共用一次发送权。普通聊天直接发件也会持久记录结果。同一调用重放，或在原会话中再次请求发送结果尚未确定的相同信封，都不会触发新发送。

IMAP 和 SMTP 的 TLS 连接校验服务器证书与主机名。TLS 初始化或握手失败时返回工具错误，
不会改用明文重试。

`calendar_list`、`email_read` 声明 `read` 效果，在只读会话和批处理里直接可用。
`email_read` 使用 `BODY.PEEK[]`，读信不会把邮件标成已读。`email_send` 声明 `send`：
Standard 会话交给它按收件人发卡，Full Access 也不会跳过这张卡。匹配的作用域授权会被
消费；人工批准会持久化授权，`once` 在同一操作里立即作废。SMTP 接受即确认回执。明确的 4xx 或 5xx 拒收会记录失败及状态码。
最终回复丢失或不完整时，回执保留为未确认，需先核对再决定是否重发。

连接管理表单每次完整校验并原子保存一个日历或邮件段落，保留其他段落。日历 URL 必须使用 HTTPS，不含内嵌凭据、查询参数或片段。配置带认证的服务根地址（例如 `https://caldav.icloud.com/`）可通过只读 PROPFIND 自动发现主体、日历主页及最多八个日历集合，然后用 REPORT 查询。畸形的发现条目或 REPORT 中的失败条目属于读取错误，不得当作成功的空结果或不完整结果。iCloud 的 [App 专用密码](https://support.apple.com/zh-cn/102654) 存入凭据库；凭据只跟随已配置的源或 Apple 的编号 CalDAV 分片地址。`jeeves connector calendar test` 走真实读取路径，仅显示未来七天事件条数。设置页仅允许 TLS 或 STARTTLS 邮件连接。非 `env:VAR`、`vault:NAME` 的旧凭据引用需先修正。邮件白名单为空只允许读取；明确打开真实发送开关后仍需逐次审批。连接器工具在新回合和新启动的子智能体重建，进行中的工具表不变。飞书身份/后台续期配置的变更仍需重启。未声明 effect 的工具一律按 `write` 处理，绝不按 `read` 放行。

### 飞书用户身份（`"as": "user"`）

默认以应用身份（`tenant_access_token`）发言；要以「某个人」的名义发，得先走一次
浏览器授权：

```bash
jeeves connector feishu login     # 授权码流程，token 落 <home>/connector-auth/feishu.json（0600）
jeeves connector feishu status    # 有效 / 已过期（下次发送前会续期）
jeeves connector feishu logout    # 删掉 token
```

然后在 `connectors.json` 的 feishu 块里写 `"as": "user"`。回调固定
`127.0.0.1:41766/callback`，这个地址要在飞书应用后台**一模一样**地登记；流程没有
PKCE。

token 是**轮换**的：refresh token 用一次就换新的，所以续期只能串行（进程内走
`connector-auth/feishu.lock`）。长住的进程（app-server）会在**过期前十分钟自动续**
—— 发送不必为它等一次网络往返，笔记本睡过到期时间也能醒来继续用；续期被飞书拒绝
（授权被撤销/refresh token 已死）时它只喊一次，让人重新 login，不会每五分钟刷一次
日志。`jeeves connector feishu status` 里的「已过期，下次发送会尝试续期」仍然成立，那是
CLI 自己的惰性路径。

## 渠道适配器

| 平台 | 变量 |
|---|---|
| 飞书 | `LARK_APP_ID`、`LARK_APP_SECRET`、`LARK_ALLOW`（open_id）、`LARK_DISPATCHERS`、`LARK_DOMAIN=lark`、`LARK_CLAIM=1`（Runtime 托管网关时由它设置） |
| Telegram | `TELEGRAM_BOT_TOKEN`、`TELEGRAM_ALLOW` |

`JEEVES_GATEWAY_BIN` 是 Runtime 托管的网关脚本（设置 → 渠道连接），Docker 镜像已设好。飞书网关的 `JEEVES_GATEWAY_URL` 指向已有 Web 运行时；不设置则启动独立 app-server。
访问私网 Runtime 时还需相同的 `JEEVES_OPS_TOKEN`。飞书入口（`bin-lark.ts`）读取仓库根目录 `.env`，
不覆盖已导出的环境变量，接法见[上手指南](guide.zh.md#渠道飞书与-telegram)。

## MCP 外挂（`<home>/mcp.json`）

```json
{"servers": [
  {"name": "fs", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]},
  {"name": "notes", "url": "https://host/mcp", "auth_env": "NOTES_API_KEY"},
  {"name": "cloud", "url": "https://host/mcp", "oauth": true}
]}
```

`command` 走 stdio、`url` 走 Streamable HTTP。`auth_env` 放环境变量**名字**
（值作 Bearer）；`oauth: true` 走浏览器授权（`jeeves mcp login <名>`），token
落 `mcp-auth/<名>.json`（0600）。连不上的服务器跳过并警告，Jeeves 照常启动。

常驻 Runtime 每 15 秒检查一次已配置 MCP 的 OAuth 凭据，剩余 60 秒时开始续期。浏览器授权会记录资源地址和 token 端点。连接改址须重新登录；旧版未绑定地址的凭据仍可在连接时刷新，再登录一次即可开启后台续期。临时服务错误保留原凭据，实际到期后停止使用。服务商返回 `invalid_grant` 时清除 token，能力卡提示重新登录。运行中的连接在下一次请求读取新 token。续期只访问授权服务，日志记录状态变化。

MCP HTTP 响应（包括流式响应）读取上限为 2 MiB。工具仅返回
`structuredContent` 时保留该 JSON，不再丢成空结果；超过上限会明确报错，
避免响应无界占用内存。

连接中断且工具结果未知时，Jeeves 为下一次调用重连。先核对远端记录，避免重复执行。设置里的 MCP 页把配置保存在 `<home>/mcp.json`，下一回合重新加载。每个回合保留启动时的工具表。

### 连接器清单

在 MCP 服务器配置中加入 `"manifest": "team/connector.json"`，即可从同目录下的相对路径加载清单，也可以把下列对象直接写进 `manifest`。清单由连接的所有者维护。工具名采用服务端公布的原名。

```json
{
  "category": "tasks",
  "tools": {
    "list": {"effect": "read", "resource": "tasks"},
    "close": {"effect": "write", "disabled": true}
  },
  "verbs": {"tasks.list": "list"}
}
```

类别可以是 `calendar`、`mail`、`tasks`、`notes`、`files`、`contacts`、`maps` 或 `other`。每个工具可声明 `read`、`draft`、`write`、`send` 或 `delete`。缺少清单、工具项或 effect 时均按 `write` 授权。在标准会话中，每次有副作用的调用都需要审批。禁用工具会将其移出下一回合的工具表。资源标签用于标识审批对象，远端访问范围由服务商配置决定。

可选的标准动词为 `calendar.list_events`、`calendar.create_draft`、`mail.search`、`mail.draft_reply`、`mail.send`、`tasks.list`。映射的工具须声明相符的类别和 effect。非法清单整体拒绝。精选目录列有配置方式和官方文档，连接状态见能力卡。

### 内置 MCP 服务器（`JEEVES_MCP_CONFIG`）

冒号分隔的额外配置文件，和 `JEEVES_SKILL_DIRS` 同一个分法：
`<home>/mcp.json` 是用户的，额外的是运营方的（整机或随包分发）。用户的
文件先读，**同名服务器用户赢**；坏掉的额外文件只是一行警告，不会让工具表
变空。设置里这些行带「内置」标签并置灰——所有写操作一律拒绝：从一台
机器的界面上删掉它，会让每一台共享它的机器一起失效。

要让 Jeeves 使用真人已登录的真实浏览器，先安装 OpenCode Browse，再把它作为
stdio 连接器写进 `<home>/mcp.json`：

```json
{"servers":[{"name":"browse","command":"/绝对路径/browse","args":["mcp"],
"manifest":{"category":"other","tools":{
  "status":{"effect":"read"},
  "tabs":{"effect":"read"},
  "execute":{"effect":"write"}
}}}]}
```

`execute` 会在用户自己的浏览器标签页里跑 JavaScript，是唯一的写工具；保持它的
effect 为 `write`，让标准会话每次调用前都征询人。登录、2FA 与付款确认由 Browse
通过 `handoff()` 交回人处理，Jeeves 不破解验证、也不保存站点凭据。这个连接器只
是传输通道：它不会给任务浏览器加上代际栅栏或基于画面的接管，需要这些保证的
任务请继续使用内置浏览器工具。

配对必须人工完成：从 Chrome 网上应用店给一个你掌控的 Chromium 浏览器装 Browse 扩展并打开它，运行 `browse pair` 取得一次性配对码，在扩展里输入。`browse status --json` 会报告 `connected`、已配对的浏览器以及未连接时的下一步。**不要运行 `browse install -y` 或 `-a '*'`**：那会改写本机其他多个 agent 的配置。请用专用浏览器 profile，只登录任务需要的站点；relay 保持默认的 `127.0.0.1` 绑定，`--remote` 会把它暴露到网络。

### 发现顺序

一条规则覆盖技能、MCP 服务器和能力中心：`user > pack > builtin`。用户文件或目录先读、同名时胜出，随包配置永远不会盖掉人的选择。写操作正好相反：只有用户自己的 `<home>/skills` 与 `<home>/mcp.json` 可改；名字已被 pack 占用时直接拒绝，绝不静默覆盖。`config_paths_for` 与 `skill_rows_in` 是可单测的纯函数核心，由 `config_paths_put_the_users_file_first` 和 `skill_rows_in_*` 测试钉住。各面的归属见[能力地图](internals/09-capability-map.zh.md#改动该落哪里)。

## `settings.json`（config/write 的持久化）

dashboard 或协议的 `config/write` 把补丁累计写入 `<home>/settings.json`，
启动时叠加在环境变量之上。清除模型覆盖：把字段写成空串后提交。校验失败
的补丁整体拒绝——不合法的值不会进文件。

### 自带模型自动救援

`modelAutoSwitchToByok` 是这个补丁里仅限人工的开关（默认 `false`）：

```json
{"modelAutoSwitchToByok": true}
```

当**平台分配**的模型处于活动状态、且调用因额度耗尽（HTTP 429）或网络错误失败时，该轮可以回退到你自己的某个带密钥的模型
profile。其它错误（5xx、401/402/403、流中断、上下文溢出，或已显示文本之后失败）绝不触发切换。开关关闭时，额度耗尽会明确
报错，由你自己选另一个模型。模型不能改这个设置。

### 原生 OpenAI Responses

内置 OpenAI 使用 `wire: "responses"` 与 `/responses`；其他 OpenAI 兼容厂商仍使用 `wire: "openai"` 与 `/chat/completions`。原生适配支持文本、图片、函数工具、SSE 文本流和加密推理接续。请求明确设置 `store: false`，接续数据保留在本地工作记忆与断点中。断流按错误处理，不执行未完整生成的函数调用。此适配未启用托管工具、WebSocket 引导或 API 异步工具调用。

自定义端点若只支持 Chat Completions，可修改目录后重启：

```json
[{ "id": "openai", "wire": "openai" }]
```

包含原生 Responses 接续数据的断点不能换到其他 wire 恢复；应使用 Responses 配置或开始新回合。接口依据当前[函数调用](https://developers.openai.com/api/docs/guides/function-calling)与[Responses 迁移文档](https://developers.openai.com/api/docs/guides/migrate-to-responses)。
## 单模型能力声明

`model/save` 接受可选 `capabilities` 对象，保存到 `models.json` 中对应配置，并由
`model/profiles` 返回。已有配置可传入：

```json
{"id":"oa","capabilities":{"tools":true,"contextWindow":128000,"reasoningEffort":"high"}}
```

`tools:false` 在调用供应商前拒绝携带函数工具声明的主模型请求。`contextWindow` 必须为正数，
替换供应商默认的上下文估计；既有 `JEEVES_CONTEXT_WINDOW` 运行时覆盖仍然生效。图像支持仍用
配置中的 `vision` 声明。这些是人工声明，不是根据模型名称推断或通过联网探测证明的能力。

`reasoningEffort` 当前要求 Responses 适配器。接受 `none`、`minimal`、`low`、`medium`、
`high`、`xhigh`、`max`，具体模型可能仅支持其中一部分。已知 GPT-6 Astra / GPT-6.1 Sol
无效组合及其 Chat Completions 函数工具不兼容在本地拒绝；其他模型特有限制由供应商判断。
显式力度只用于配置中的主模型，不应用到另一个 `smallModel`；未声明时不发送力度字段。
参见[官方推理参数协议](https://developers.openai.com/api/docs/guides/reasoning#reasoning-effort)。

省略 `capabilities` 保留原声明；传入 `{}` 则整体替换为继承默认值。未知字段拒绝处理，
不会被当作无作用的开关接受。`model/save` 在写入凭据前拒绝无效声明。
托管工具、原生压缩和 API 异步工具仍未支持。

## 可选任务浏览器

| 环境变量 | 默认 | 含义 |
|---|---|---|
| `JEEVES_BROWSER_RUNTIME` | 未设置 | `just browser-runtime` 生成的独立组件目录；未安装时不注册浏览器工具。含浏览器的桌面包自动定位内置组件 |
| `JEEVES_DESKTOP_HELPER` | 未设置 | 指向可信的 macOS 单窗口助手的绝对路径。打包的 Mac 桌面端启动本地服务时自动定位助手，开发环境见[单窗口操作](protocol.zh.md#macos-单窗口操作)。 |
| `JEEVES_BROWSER_HEADLESS` | `0` | 仅测试使用；无可接管窗口，人工接管会明确拒绝 |
| `JEEVES_BROWSER_CDP_ATTACH` | `0` | 允许通过字面回环 CDP 地址（`http://127.0.0.1`、`ws://127.0.0.1`、`[::1]`）接管本机已打开的浏览器 |
| `JEEVES_BROWSER_VIEW_ONLY` | `0` | 保留观察、导航与滚动，但点击、填写等浏览器写动作在审批前即被拒绝；子代理继承该限制 |

Web 控制的是运行服务所在机器上的任务浏览器。本阶段不支持远程窗口交互。

打开浏览器时可填写登录档案名（字母、数字、`-` 和 `_`，最多 64 字符），新任务上下文会继承该档案保存的 Cookie 和本地存储。只有人工接管期间才能保存或清除档案；Runtime 通过凭据库保存登录状态，绝不交给模型，并按账号隔离。清除档案或急停会使新浏览器不再恢复旧登录态；已经打开的浏览器在交还控制后仍需重新观察。

设置 `JEEVES_BROWSER_CDP_ATTACH=1` 后，驾驶舱可通过字面回环 CDP 地址接管已经打开的浏览器。操作者需要自行以调试模式启动该浏览器；Jeeves 会把它预留给一个任务，结束时断开而不关闭浏览器，并继续强制来源白名单、代次与人工接管。Attach 模式不能使用 Runtime 的本地 DNS 钉住代理，因此只应允许你信任的网站来源，并在用完后关闭调试端口。
