# ⑦ 外壳：CLI、仪表盘后端、前端、网关、桌面端

[English](07-shells.md) | 简体中文

每个外壳都是同一个 Runtime（[06-app-server](06-app-server.zh.md)）的客户端，说同一套协议
（[01-protocol-and-config](01-protocol-and-config.zh.md)）。外壳只负责渲染、传输和原生能力，
不重新实现智能体行为（[architecture](../architecture.zh.md)）。

目录：`runtime/crates/jeeves-cli/`、`runtime/crates/jeeves-ops/`、`web/`、`gateway/`、`integrations/channels/`、`executors/browser/`、
`desktop/src-tauri/`，以及 `Dockerfile`。

## 7.1 `jeeves` 二进制

`jeeves-cli/src/main.rs` 按第一个参数分发：

| 调用 | 行为 |
|---|---|
| `jeeves` | 终端对话：进程内打开 Server，挂上记忆 worker，运行 `repl::run` |
| `jeeves app-server` | stdio 上的 JSON-RPC；挂上记忆 worker 和调度器 |
| `jeeves runtime` | `jeeves_ops::serve`（HTTP + SSE） |
| `jeeves schedule acknowledge <name>` | 释放被中断的定时槽位；`schedule` 的其他形式一律拒绝（任务在 Runtime 内触发） |
| `jeeves home export <dir>` / `import <dir>` | 搬运一个 home（7.3） |
| `jeeves memory [rebuild\|backfill]` | 记忆维护（7.3） |
| `jeeves skill [list\|export\|install\|update]` | 技能（7.3） |
| `jeeves mcp [list\|login <name>\|serve]` | MCP 客户端配置与记忆 MCP 服务（7.3） |
| `jeeves connector [list\|feishu …]` | 人自己使用飞书连接器的入口（7.3） |
| `jeeves eval [path]` / `eval --replay [path]` / `judge [path]` / `research [path]` / `bench [path]` | 质量命令，见 [08-quality](08-quality.zh.md)；退出码即结论（`research` 与 `bench` 恒为 0） |
| `jeeves --version` / `-V` | `jeeves <version>` |

其他参数输出用法并失败。在 Windows 上二进制拒绝启动（退出码 2）：执行沙箱只有 macOS 和 Linux 后端，
hook 通过 `sh` 运行，Windows 凭据库无法读回密钥。

`home()` 取 `JEEVES_HOME`，否则取当前工作目录下的 `./.jeeves`。`app-server`、`memory`、`runtime` 和 REPL 路径都把该值写入
`settings.home`，所以显式的 `JEEVES_HOME` 总是优先。`dashboard` 自己不创建该目录，由它拉起的 app-server 创建。

## 7.2 终端对话

`repl.rs` 在 `tokio::spawn` 内用 `run_turn` 运行每个回合，并从 `EventSink` 通道逐帧绘制：
流式文本立即打印，其他通知（记忆门、工具开始/结束、整合、回合摘要行）画成一行小字。回合没有流式输出时，
整段回复一次性打印。

斜杠命令包括 `/help`、`/memory [term]`、`/compact [focus]`、`/goal <condition>`
（`/goal clear`）、`/cwd [path]`、`/model`（`use`、`add`）、`/route`（`auto`、`fixed`、`set <tier> <profile>`）、
`/quit`（`/exit`、`/q`）。未知的 `/x` 会明确报告未知。

审批在现场回答：用单独的句柄读 stdin，`y` 仅本次、`s` 本会话、`a` 记住规则，其他任何输入（含 EOF）都是拒绝。
回合已不再等待后才到的回答会被报告为已过期。

## 7.3 维护子命令

| 命令 | 行为 |
|---|---|
| `jeeves home export <dir>` | 写出一个普通目录：一致的 `state.db` 快照、其余文件原样复制，以及 `jeeves-home.json` 清单（`format`、`schema`、`jeeves`、`exportedAt`、被排除的内容）。目标必须为空 |
| `jeeves home import <dir>` | 把这样的目录恢复进 home |
| `jeeves memory rebuild`（默认） | 先把手工修改折回，再由事实重新生成明文库 |
| `jeeves memory backfill` | 由聊天历史构建账本行，只做一次，且仅当账本为空时 |
| `jeeves skill list`（默认） | 每个技能一行 |
| `jeeves skill export --to claude,codex [--project] [--force] [--names a,b]` | 把技能复制到 `.claude` / `.codex` 的技能目录 |
| `jeeves skill install <url\|path>` | 安装一个技能；从不覆盖 |
| `jeeves skill update [index]` | 按版本从索引更新（默认 `<home>/skills/index.json`；也可以是路径或 http(s) URL） |
| `jeeves mcp list`（默认） | `mcp.json` 中的条目及其认证方式 |
| `jeeves mcp login <name>` | 对有 `url` 且 `oauth: true` 的服务器做浏览器 OAuth；这是唯一会打开浏览器的地方 |
| `jeeves mcp serve` | 通过 stdio 把事实与情节作为 MCP 服务暴露 |
| `jeeves connector list` / `feishu login\|logout\|status` | 连接器状态与飞书用户授权 |
| `jeeves connector feishu --to <id> --text <body> [--dry-run]` | 经由与工具相同的允许名单和线路代码发送；敲命令的人即审批 |

`home export` 不包含 `state.db` 的活动文件、锁文件、`secrets.json`、`mcp-auth/` 和 `connector-auth/`；
导入后需要重新提供密钥和登录。

## 7.4 `jeeves-ops`：HTTP 门面

`jeeves runtime` 运行 `jeeves_ops::serve`。该 crate 是它自己拉起的 `jeeves app-server` 子进程的客户端：
`AppServer::spawn` 用相同的 `JEEVES_HOME` 启动 `current_exe() app-server`（未设置时默认 `JEEVES_WRITE=1` 和
`JEEVES_APPROVAL=on-request`），按 id 关联请求，并向订阅者广播通知。app-server 的错误原样作为协议的 `ErrorObject` 返回。

**启动。** Rust Runtime 先核验服务凭据并等待引擎就绪，然后监听私网端口并打印 `Jeeves Runtime → http://<host>:<port>`，之后启动已配置的飞书渠道。停止信号会结束 SSE，再让回合收尾。

**认证。** Go 网关在服务端附加 Bearer 凭据。服务间的 `GET /healthz` 和有独立签名的 webhook 采用各自的协议。Runtime 提供 HTTP API 和 SSE，工作台静态文件由 Go 网关提供。

**路由**（`lib.rs` 的 `router`；处理器在 `api/` 下）。每条是对一个或几个协议方法的翻译：

| 分组 | 路由 |
|---|---|
| 首屏与配置 | `GET /api/data`、`GET/POST /api/config`、`GET /api/usage`、`GET /api/inbox` |
| 回合 | `POST /api/turn`（`ServerNotification` 的 SSE 流）、`POST /api/turn/steer`、`POST /api/turn/follow-up`、`POST /api/interrupt`、`GET /api/turns`、`POST /api/interaction`、`POST /api/run`（任务 API：一轮进、一个 `RunResponse` 出，无 SSE） |
| 审批 | `GET /api/approval/pending`、`POST /api/approval`、`GET /api/approvals`、`GET/POST /api/session/permission` |
| 会话 | `GET /api/session`、`GET /api/session/main`、`GET /api/session/meta`、`POST /api/session/update` |
| 任务与浏览器 | `GET /api/tasks`、`POST /api/task`、`POST /api/turn-operation`、`GET /api/browser`、`POST /api/browser/start\|control`、`GET /api/browser/frame` |
| 模型 | `GET /api/model/catalog\|profiles\|routes`、`POST /api/model/save\|remove\|routes/save` |
| 能力 | `/api/skills`（+ `install`、`remove`、`stats`）、`/api/mcp`（+ `save`、`remove`）、`/api/schedules/jobs`、`/api/schedules/save\|remove`、`/api/vault`（+ `remove`） |
| 记忆中心 | `GET /api/ledger/turns`、`/api/ledger/turn`、`/api/memory/index`、`/api/memory/page`、`/api/context`、`POST /api/compact`、`POST /api/memory/remember` |
| 文件 | `POST /api/upload`（单个 octet-stream，20 MB）、`GET /api/fs`、`POST /api/fs/mkdir`、`GET/POST /api/fs/recents` |
| 同步 | `GET /api/events`（ping 的 SSE：kind、turn id、session id）、`GET /api/notifications`（完整通知 SSE） |
| 连接 | `GET/POST /api/connections/lark`、`POST /api/connections/lark/claim`、`POST /api/gateway/rpc` |

`/api/interaction` 先订阅再发送 `interaction/start`，所以不会丢失最早的通知。SSE 只有服务器到浏览器一个方向；发送消息是普通 POST。
被拒绝的人工操作（`/api/task`、能力写入）以状态 200 加 `ErrorObject` 响应体回答，页面可原样显示这句话。

**能力中心**（`capabilities.rs`）：来自 `JEEVES_SKILL_DIRS` 的技能和来自 `JEEVES_MCP_CONFIG` 的 MCP 服务器以 `source: "builtin"`
列出，所有写入都会拒绝；只有 `<home>/skills/` 和 `<home>/mcp.json` 可写。列表的合并方式与 app-server 相同（用户文件在先，同名先到先得）。

**入站 webhook**（`webhooks.rs`，`<home>/webhooks.json`）：每个 hook 有 `name`、`secret`（`env:VAR` 或字面值）、`prompt`
（`{{payload}}` 被替换为请求体，否则请求体追加在后），以及可选的 `session`（默认 `hook:<name>`）、`tier`、`enabled`、
`signature`、`limit`、`callback`。未知或已禁用的名字返回 404，密钥错误返回 401，文件损坏会禁用全部 hook 并输出一行 stderr。
`signature: true` 时调用方发送 `X-Jeeves-Timestamp` 和 `X-Jeeves-Signature`（对 `<timestamp>.<body>` 的 HMAC-SHA256，窗口 300 秒），
每个签名只被接受一次；`limit` 限制每滚动分钟接受的请求数（429 加 `Retry-After`）；`callback` 先回 202，之后把结果 POST 过去。
`Hook::dispatch` 只选一条路径：有 `event_type` 就走带游标去重的签名责任事件，不跑模型回合；否则跑一次普通 headless 回合，
工具保留各自的审批，而且没有人在场授予。`callback` 只作用于回合路径。

`POST /api/gateway/rpc` 只转发 `interaction/start`、`session/main`、`task/get`、`task/wake`、`approval/pending` 和
`approval/respond`；其余方法一律回答 method-not-found。

## 7.5 `@jeeves/client`

Node Runtime 客户端位于 `integrations/channels/runtime-client/`。`protocol.ts` 转出
`protocol/schema/typescript/v2/` 生成的类型。客户端与 Web 工作台共用 `protocol/typescript/` 下的
SSE 解析器和事项事件游标。

- `JeevesClient` 分配 id，维护以字符串化 id 为键的待决表，把每行解析为通知、错误或响应，传输关闭时拒绝所有待决请求。
  `JeevesError` 携带 `code` 和 `data`，并提供 `isProvider`、`isNotImplemented`、`isInvalidParams` 三个判断。
- `StdioTransport` 启动 `jeeves app-server`，按行切分 stdout，把 stderr 转给 `onLog`，`close()` 只结束 stdin，让服务器写完剩余帧。
  `findJeevesBinary` 依次取 `JEEVES_BIN`、从包目录向上查找的 `target/{debug,release}/jeeves`、PATH 上的 `jeeves`。
- `HttpTransport` 连接正在运行的 Runtime，向 `/api/gateway/rpc` 发起请求，订阅
  `/api/notifications`，并在配置后携带 Runtime 的私网 Bearer 令牌。

## 7.6 `@jeeves/dashboard`

Vue 和 Vite 将工作台及账号页面构建到 `web/dist/`。Go 网关在通过鉴权的工作区路径下提供工作台，浏览器使用同源会话 Cookie 调用对应的 Runtime API。运行 `just dashboard-build` 生成资源。

| 部分 | 作用 |
|---|---|
| `main.ts`、`WebControls.vue`、`workspace.ts` | 挂载 Vue 控件并运行页面控制器 |
| `api.ts`、`hosted.ts` | 工作区请求携带网关会话 Cookie，会话失效后返回登录页。流式响应使用共用 SSE 解析器。 |
| `sync.ts` | 保持 `/api/events` 打开，把 500 ms 内的 ping 合并为一次刷新，永远重连；15 秒轮询（`POLL_MS`）作为后备 |
| `render.ts`、`timeline.ts`、`tasks.ts` | 不拼 HTML 字符串，直接构建 DOM |
| `markdown.ts` | 手写的 Markdown 渲染器，为模型回复构建 DOM 节点；不用第三方解析器 |
| `ui/` | 共享组件：`SetupWizard`、`SettingsPanel`、`TaskCenter`、`BrowserPanel`、`WorkspacePicker`、`AttachmentPicker`，以及组合式函数 `useTasks`、`useBrowsers` |
| `sw.js`、`manifest.webmanifest` | PWA 页面优先请求网络，API 响应不缓存。构建为入口与缓存生成同一版本标记。 |

单机部署的首次设置和菜单入口见[上手指南](../guide.zh.md)。

**语言。** 中文字符串就是词典的键：`t("还没有会话")`，占位符写作 `{n}`；`i18n.ts` 持有英文表。静态标记使用 `data-i18n`、
`data-i18n-title`、`data-i18n-placeholder`、`data-i18n-aria`。语言取自 `localStorage` 的 `jeeves.lang`，否则取 `navigator.language`
（以 `zh` 开头为中文）。切换语言即重新加载页面。缺少英文条目时回退为中文并输出一次 `console.warn`。模型输出、会话和任务标题是数据，
从不翻译。`scripts/check_i18n.py`（`just check-i18n`）检查每个用到的键都有翻译、每个翻译的键都被用到、占位符一致。

## 7.7 聊天网关

`@jeeves/channels` 包含 `JeevesBridge` 以及飞书和 Telegram 适配器。可用
`just gateway-lark` 和 `just gateway-telegram` 独立启动。
见[渠道适配器配置](../configuration.zh.md#渠道适配器)。

| | Lark / 飞书 | Telegram |
|---|---|---|
| 连接 | 长连接 WebSocket，使用 Lark 私有 protobuf 帧（`pbbp2`，在 `lark-proto.ts` 手写） | 长轮询；不需要公网地址 |
| 允许名单 | `LARK_ALLOW`：open_id（`ou_…`） | `TELEGRAM_ALLOW`：数字用户 id |
| 消息上限 | 30000 字符 | 平台限制 |
| 时序 | 收到的数据帧先持久化，再确认 ACK 并处理。心跳默认 120 秒（最小 10 秒），重连退避上限为 30 秒。 | 轮询 bot API；答案走同一个 Runtime 回合 |
| 其他设置 | `LARK_APP_ID`、`LARK_APP_SECRET`、国际版用 `LARK_DOMAIN=lark`、`LARK_DISPATCHERS`、`LARK_CLAIM` | `TELEGRAM_BOT_TOKEN`，仅从环境变量读取 |

`bridge.ts`（`JeevesBridge`）承载与平台无关的部分：

- **唯一主会话。** 桥接器按顺序提交消息，在 `jeeves-main` 通过 `interaction/start` 发起回合。
  平台会话 ID 用来定位回复与审批的来源。
- 桥接器先用平台消息标识匹配提前到达的通知，再用返回的 `turnId` 跟踪回合直至结束。
- `JeevesBridge` 自己启动 app-server，或在给定 `url` 和 `token`（`JEEVES_GATEWAY_URL`、`JEEVES_OPS_TOKEN`）时附着到 Runtime。
- `explain(error)` 把 `JeevesError` 转成发给聊天的通俗句子。
- **调度人。** Runtime 按 `JEEVES_DISPATCHERS` 决定哪些群成员可以交办事项。桥接器携带
  `InteractionSource`，并生成稳定的消息 ID 用于去重。`reports.ts` 将事项的结束状态回报给源线程。
  `approvals.ts` 发送审批卡，获授权的决定经带版本的 `approval/respond` 写回。

`splitMessage(text, limit)`（`text.ts`）只有一份实现：在过半位置之后的换行处切分，否则硬切。

## 7.8 Runtime 托管的 Lark 网关

设置 → 连接（`api/connections.rs`、`gateways.rs`）让所有者无需第二个终端，直接在仪表盘里运行 Lark 网关：

- 设置存于 `<home>/gateways.json`（`enabled`、`appId`、`allow`、`dispatchers`、`domain`）；应用密钥存入密钥库，账户名
  `gateway:lark:app-secret`，API 从不读回。`allow` 和 `dispatchers` 中的 id 必须以 `ou_` 开头。
- `start` 启动 `node integrations/channels/src/bin-lark.ts`（可用 `JEEVES_GATEWAY_BIN` 覆盖），传入 Lark 变量和指向本 Runtime 的
  `JEEVES_GATEWAY_URL`，设置了令牌时还传 `JEEVES_OPS_TOKEN`。网关通过 `POST /api/gateway/rpc` 和 `/api/notifications` 附着。
- **认领流程**：`allow` 为空时进程以 `LARK_CLAIM=1` 运行，不回答任何人；第一个发消息的人以 `claim candidate: <open_id>`
  写到 stderr，由 Runtime 捕获。带该 id 调用 `POST /api/connections/lark/claim` 会把它写成唯一的允许名单条目并重启网关；
  已有所有者或 id 不是等待中的候选人时失败。
- 状态为 `stopped`、`running` 或 `exited`，附带候选人和最近 20 行 stderr。启动、退出、停止各输出一行 stderr。
  当 `enabled` 且设置了 app id 时网关随启动自动运行，并在关闭时停止。

## 7.9 任务浏览器 worker

模型看到 `browser_observe`、`browser_act` 和 `browser_batch`（`jeeves-tools/src/browser.rs`，位于注入的 host trait 之后）。所有权、批准的来源、
进程生命周期和控制代数在 `jeeves-app-server/src/browser/`（`api`、`process`、`store`）；`/api/browser*` 路由只做传输，
`browser/start` 只能由人调用。

`@jeeves/browser-worker` 是 Node/Playwright（Chromium）进程，每个会话一个浏览器上下文、一个页面：

- 传输：stdin/stdout 上每行一个 JSON 对象；超过 64000 字节的行或无效请求会使 worker 停止，stdin 关闭也会停止。命令契约由 `BrowserWorkerCommand` 生成。每个响应带 `info`（schema 版本、实例 id、支持的能力、`maxOrigins` 12、`maxFrameBytes` 800000）、会话状态、可选的帧、错误和 `uncertain`。
- host 对一次操作施加 20 秒期限，超时即关闭 worker。
- `network.ts` 通过本地代理强制来源范围；`observe.ts` 生成观察结果和帧；`document.ts` 跟踪文档变化，使过期引用失效；
  `session.ts` 串行化命令并监视心跳。
- 运行时按需启用：`JEEVES_BROWSER_RUNTIME` 指向含 `runtime.json`（Node 与 Playwright 版本）和 worker 的目录；`JEEVES_BROWSER_HEADLESS`
  无窗口运行。`just browser-runtime` 准备 `desktop/src-tauri/browser-runtime/`（`scripts/prepare-browser-runtime.py`）；
  `just smoke-browser` 演练完整流程。
- 仪表盘一侧是 `ui/BrowserPanel.vue` 和 `useBrowsers.ts`：从任务卡片打开、接管、交还、暂停、关闭，也可[远程操作或接受任务下载](../protocol.zh.md#远程浏览器操作与下载)。

策略（审批、范围、不确定的副作用）与循环和工具一起记录在 [04-tools](04-tools.zh.md) 和 [06-app-server](06-app-server.zh.md)。

## 7.10 桌面外壳

Tauri 开发壳打开 Go 网关，并可按工作区配置启停私网 Runtime。托管版 Mac 设备应用用于配对和授权所选窗口。构建与测试命令见 [Mac 桌面应用](../operations.zh.md#mac-桌面应用)。

## 7.11 容器镜像

根目录的 `Dockerfile` 构建私网 Rust Runtime 和可选渠道适配器。Go 网关镜像携带浏览器资源。部署见[云端部署手册](../../control-plane/README.zh.md)。
