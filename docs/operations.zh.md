# 运行手册

[English](operations.md) | 简体中文

日常运行的每一种形态、推荐的环境变量与常见故障。

## 支持的平台

| 平台 | 状态 | 原因 |
|---|---|---|
| macOS（Apple Silicon / Intel） | ✅ 一等公民 | seatbelt 沙箱、钥匙串、`sh` hooks |
| Linux | ✅ 一等公民 | bubblewrap 沙箱、secret-service、`sh` hooks |
| Windows（原生） | ❌ 拒绝启动（exit 2） | 没有沙箱后端——按「无沙箱不执行」铁律 `run_command` 永远跑不了；hooks 要 `sh`；凭据库读不回密钥，只能落明文 `secrets.json` |
| Windows + WSL2 | ✅ 用 Linux 版即可 | 上面的一切都是真的 Linux 用户态 |

拒绝发生在 `jeeves` 二进制启动时（`cfg!(windows)`），原因直接打印——
「看起来健康但处处受限的半成品」不如一声清楚的不。CI 保留 Windows job
但只做 build + 单测（沙箱与 hooks 测试照旧跳过）；库在哪儿都能编译，
二进制只是不启动。

## 终端对话

```bash
cargo build
JEEVES_PROVIDER=deepseek DEEPSEEK_API_KEY=… ./target/debug/jeeves
```

终端打开 Runtime 的主对话。`/memory [词]` 查记忆，
`/quit` 退出。中断：`Ctrl-C` 在终端里是退出进程——网页端才用
`turn/interrupt` 打断。

显式查询记忆或会话时，终端会报告存储错误。若一个记忆索引可用，终端仍会显示其结果。

## 网页驾驶舱

在部署提供的 HTTPS 地址注册或登录。有可用会话时，首页直接进入自己的工作台。在同一账号里使用主对话、查看「工作」和复验文件交付。首次启用工作区时可以提前写草稿。

### 放到一台服务器上（自托管）

按[云端部署手册](../control-plane/README.zh.md)安装 Go 网关、Go 控制面和 Rust Runtime。只在公网发布网关的 HTTPS 入口。

在一台电脑上从源码使用时，运行 `just local-up`。它会启动回环网关、创建独立的个人账号库和 Runtime 数据目录，并打开所有者设置页面。重启时使用原目录，浏览器继续访问命令打印的地址。[个人部署配置](../gateway/README.zh.md#个人部署)

## MCP 外挂

写 `<home>/mcp.json`（stdio 或 HTTP 服务器），工具自动并入工具表。
远程服务器用 `"oauth": true` 时先 `jeeves mcp login <名>`；token 过期自动
刷新，没有 refresh_token 时重跑 login。

驾驶舱的**设置 → MCP 连接器**是同一个文件的图形门：加 / 替换 / 删一个连接器，
传输与鉴权规则和服务端同一套（`url` 与 `command` 二选一；`auth_env` 是
变量名，不是密钥）。改动重启 Jeeves 后生效。要发整机共用的服务器，设
`JEEVES_MCP_CONFIG`——那些行在设置里带「内置」标签置灰，所有写操作
一律拒绝。

运维侧的已知边界（没有日志轮转等）见
[limitations.zh.md](limitations.zh.md)。

### 把 Jeeves 的记忆给别的 agent 用

反方向也有：`jeeves mcp serve` 让 Jeeves 自己成为一台 stdio 的 MCP 服务器，暴露
五个记忆工具（`memory_search`、`memory_remember`、`memory_forget`、
`memory_list`、`memory_episodes`），这台机器上的其他 agent 因此读写同一份
事实。**只暴露记忆**——不暴露任何「替别人动手」的能力。

```json
{"mcpServers": {"jeeves-memory": {
  "command": "jeeves",
  "args": ["mcp", "serve"],
  "env": {"JEEVES_HOME": "/Users/you/.jeeves"}
}}}
```

任何 MCP 客户端（Claude Code、codex……）配上这一段就与 Jeeves 共享长期记忆。
服务器用同一个 `open()` 开同一个 `state.db`（WAL + busy_timeout，多进程本来
就能共存）；别的 agent 写进来的事实标着 `source: mcp`，来源始终可查。

## 周期委托升级

曾通过启动环境开启的晨报已迁入「我的工作」，可查看旧草稿。请在对话中确认新的来源、时间和接收位置。见[周期委托](design/main-entry.zh.md#6-周期委托)。

## 用脚本驱动它

没有要安装的客户端库：`jeeves app-server` 说的是 stdio 上的 JSON-RPC，一行一个
JSON 对象，任何语言几行就能驱动。

```python
import json, subprocess

proc = subprocess.Popen(["jeeves", "app-server"], stdin=subprocess.PIPE,
                        stdout=subprocess.PIPE, text=True, bufsize=1)
proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": "turn/start",
                             "params": {"message": "hi", "stream": True}}) + "\n")
proc.stdin.flush()
for line in proc.stdout:               # 通知与响应共用同一个流
    print(json.loads(line))
```

环境变量由调用方决定（子进程继承）；`JEEVES_BIN` 可以指向二进制。

## 打包

从仓库根目录运行 `docker build -t jeeves .` 构建 Rust Runtime 镜像。租户状态挂载到 `/home/jeeves/.jeeves`，Runtime 在私网上监听，提供 `/healthz` 和鉴权 API。镜像带有 `bubblewrap`、CA 证书和可选的飞书渠道适配器。构建 Go 网关和控制面镜像见[部署手册](../control-plane/README.zh.md#构建与安装)。

Ctrl-C 和 SIGTERM 会结束 SSE 并让正在执行的回合收尾。超过 25 秒仍未完成的工作可恢复。引擎子进程异常退出时 Runtime 也退出。`just smoke-lifecycle` 验证两个停止信号、重复数据目录和引擎崩溃边界。

### CLI 发布

在 `Cargo.toml` 更新 Rust workspace 版本，并添加 `.github/release-notes/v<版本>.md`。在发布提交上运行 `just check` 和 `just release-package`。可在分支上手动运行[发布流水线](../.github/workflows/release.yml)，构建并验证四个平台的 CLI 压缩包。推送对应的 `v<版本>` 标签后，流水线会发布 macOS 和 Linux 的 arm64、x86-64 CLI 压缩包与 `SHA256SUMS`。云端镜像和 Mac 应用分别按各自的构建说明打包。

## 保持常驻

`jeeves service` 会写一份守护单元，让 Runtime（`jeeves runtime`）在关掉所有客户端、重启电脑后仍然在跑。守护者是操作系统自己的，Jeeves 不再另造一个。

```bash
jeeves service print            # 只显示将要写入的单元
jeeves service install          # 写入单元，不启动任何东西
jeeves service install --start  # 写入并加载（立刻启动，之后每次登录自启）
jeeves service status           # 单元是否已安装、端口是否有响应
jeeves service uninstall        # 卸载并删除单元；JEEVES_HOME 里的状态不动
```

* macOS：`~/Library/LaunchAgents/ai.jeeves.runtime.plist`，失败后重启，间隔 10 秒。
* Linux：`~/.config/systemd/user/jeeves.service`（`Restart=on-failure`，5 秒）。想不登录也开机启动，运行一次 `loginctl enable-linger $USER`。
* Windows 没有模板（Runtime 在那里本来就起不来）。


单元固定安装时的 `JEEVES_HOME` 和 `JEEVES_PORT`，并把日志写到 `<home>/jeeves.log`。启动前，在数据目录创建仅所有者可读的 `runtime-token` 文件，填入与 Go 网关 `GW_RUNTIME_TOKEN` 一致的私网凭据。容器由 Kubernetes 管理。

## Mac 桌面应用

托管版 Mac 应用将电脑连接到云端账号。用安装服务的公网地址运行 `just desktop-device-build https://jeeves.example.com` 打包。打开应用，点击**连接**并在浏览器确认配对。任务需要操作电脑时，Jeeves 会在 Mac 上请求窗口访问权限。在账号的设备列表管理访问。通过浏览器或 iOS PWA 打开工作区，与助理对话并跟进事项。

下节命令适用于源码仓库中的 Runtime 开发应用。

开发版 Mac App 打开 Go 网关。`just local-up` 的默认地址是 `http://127.0.0.1:8743`，开发者也可以设置 `JEEVES_WEB_GATEWAY_URL`。`just desktop-build` 打包 Runtime，`just desktop-build-browser` 附加可选的任务浏览器运行时。

日常使用和共同测试时，运行 `just desktop-open`，或在 Finder 双击本仓库的 `desktop/src-tauri/target/release/bundle/macos/Jeeves.app`。两个入口打开同一份应用。应用已运行时，macOS 会把原窗口带到前台，沿用当前 Runtime、登录状态和会话。首次缺少应用包时命令会构建它。修改代码后需要重新打包时，运行 `just desktop-build`。

关闭开发版 App 的窗口只会隐藏它，Runtime 继续执行定时任务，点 Dock 图标即可恢复窗口。在任何应用里按 ⌥Space 会把 Jeeves 调到最前，光标落在输入框；再按一次隐藏 Jeeves，回到原来的应用。如果 ⌥Space 已被别的应用占用，Jeeves 照常启动，只是没有这个快捷键。Dock 图标上的数字是等你决定的事项数，浏览器标签页标题也会显示为 `(N) Jeeves`。Jeeves 不在最前时这个数字一旦增加，App 会按系统语言发一条 Mac 通知；第一次会由 macOS 询问是否允许。用 ⌘Q 退出。App 会在上次的窗口大小和位置打开，位置保存在应用配置目录下的 `window-frame`。如果这个位置已经不在任何已连接的显示器内，窗口改为居中。

共同测试前运行 `just desktop-status`，核对已有 App 与 Runtime 的 PID、监听地址、数据和追踪路径。命令显示的提交号是源码版本，正在运行的应用可能来自较早的构建。共同验收沿用同一窗口和账号，完成后把窗口留在对应的事项或报错处。复现问题时记下操作步骤和本机时间，有任务 ID 或回合 ID 就一并记下。可到命令显示的追踪目录查找对应回合。App 将 Runtime 诊断日志写到 `<home>/jeeves.log`。自动冒烟使用测试数据，结果单独记录。

Mac 和 Web 共用开发工作区时，在 `<home>/desktop-gateway` 写一行 Go 网关地址，例如 `https://localhost:8743`。启动网关时把 `GW_RUNTIME_HOME` 设为同一个 `JEEVES_HOME` 的绝对路径。网关使用 `GW_RUNTIME_TOKEN`，Runtime 使用仅所有者可读的 `<home>/runtime-token`，两者使用同一服务凭据。App 启动 Runtime 时由系统分配端口，网关每次重启后都会发现它。浏览器和 App 使用同一个网关地址。Finder 启动仓库构建的 App 时可沿用仓库中的 `.jeeves`，安装到系统的副本默认使用 `~/.jeeves`。

在仓库根目录运行 `just dashboard-build` 后，重新载入网关页面即可更新前端。Runtime 代码在替换二进制并重启后生效。

应用打开后进入「对话」。从「工作」或回执打开交付页面并补充要求。[对话、事项与交付](design/main-entry.zh.md)

在 Mac 的仓库目录运行 `just smoke-mac-cockpit` 可复验桌面端。命令会构建 Jeeves.app，在自己的前台窗口中操作主聊天和 Cowork；运行时使用临时目录，模型和发布服务均由本机模拟。测试随后退出并重新打开应用，用同一份临时数据核对任务 ID、完整会话和唯一的完成摘要。运行终端需已有辅助功能权限。若已有屏幕录制权限，测试会将 Jeeves 单窗口截图和 JSON 报告一起保存在 `target/acceptance/mac-cockpit/<运行时间>/`。

## 定时任务

常驻的 Runtime（`jeeves app-server`、`jeeves runtime`）按五字段 cron（`*`、`*/步长`、`a-b`、`a,b`）触发
定时任务，不再有单独的调度进程。两种声明方式：

* 技能的 frontmatter 里带一行 schedule：

  ```markdown
  ---
  name: weekly-review
  description: 汇总这一周并起草周一简报
  schedule: 0 8 * * 1
  ---
  ```

* 存储的任务：在**工作 → 长期**添加，或直接让 Jeeves 建（`schedule_job`）。任务存在 `state.db` 里；
  旧的 `<home>/schedules.json` 在启动时一次性导入，并改名为 `schedules.json.migrated`。

通用任务每次触发都在自己的会话里跑完整一轮（`schedule:<任务名>`），所以每条任务带着
自己的历史与滚动摘要；结果写进 `<home>/outbox/schedule-<名>-<时刻>.md`，
而不是往聊天里塞。同一分钟最多触发一次，任务串行执行，技能改动不用重启就
生效。用 [`jeeves service`](#保持常驻) 让 Runtime 常驻；停机期间错过的触发，下次启动补跑一次（只补最近错过的那一分钟，最多回看七天；`catch_up` 关闭则放弃）。

两种声明都会出现在驾驶舱的**工作 → 长期**分组，排在长期职责下面：一行一个
任务，带 cron、提示词、下次触发、上次触发和投递方式。来自技能的那些行是灰的、只读 ——
那条任务**就是**那个技能，要删就去改 `SKILL.md` 或删技能。新增/删除存储的任务只改
`state.db` 里的那一行：

```
name      morning-news        小写字母、数字、连字符（它同时是日程的键）
cron      0 8 * * 1-5         五段：分 时 日 月 周
prompt    给我一份今天的科技简报   一句话 —— 它会变成一轮对话
delivery  outbox | notify | quiet
catch_up  错过了补跑一次（默认），还是放弃
run_at    2026-11-25 15:00    代替 cron：在这个本地时间只跑一次，跑完任务自动删除
```

`run_at` 任务是提醒而不是周期：只触发一次，随后删除那一行。如果到点时 Runtime 没在运行，
`catch_up` 开着且错过不到一周，就补跑一次（提示里会说明晚了）；否则报告出来，在
`schedule-runs.jsonl` 记一条 `missed`，然后删除。保存时，已经过去或超过一年后的时间会被拒绝。

这块面板是**任务的编辑器**，不是调度器：触发的是 Runtime，面板底部就写着这句话 —— 一个悄悄依赖运行中进程的表单，比没有表单更糟。

面板不是唯一的门：直接在对话里说「每天八点给我一份科技简报」也行 —— 模型会调
`schedule_job`，走同一个校验存同一行，回答里带上「下次触发」的时间，并提醒
Runtime 得在运行。「明天下午三点提醒我」也一样，会存成一个 `run_at` 任务。删是 `cancel_schedule`，列是 `list_schedules`。

为什么要做成工具、而不是「让它自己去写文件」：任务声明是 Jeeves 自己的状态，不是项目内容，
所以存在 `state.db` 里，由管它的工具来写，不再有人问你要目录。

## 数据与备份

`<home>/` 下值得备份的只有几样：`state.db`（记忆与对话，事实来源）、
`SOUL.md`（人格）、`skills/`（过程记忆）、`mcp-auth/`（OAuth token，
0600）。`traces/`、`outbox/` 是可再生成的
派生物。

`jeeves home export <目录>` 把整个 home 写到一个新目录：`state.db` 是一致性快照（Jeeves 运行中
也能导出），其余文件原样拷贝，`jeeves-home.json` 写明格式与 schema 版本。`jeeves home import
<目录>` 把它恢复到一个空的 `JEEVES_HOME`，并把较旧的 schema 迁移到最新。换机器、本地与云端
之间搬家都用这一对命令。密钥与登录不导出（`secrets.json`、`mcp-auth/`、
`connector-auth/`），导入后需要重新填写。目标目录必须位于源目录之外。复制和数据库检查成功后，完整的备份或恢复后的 home 才会出现在目标位置。

## 表结构变更：expand / migrate / contract

每次改 `state.db` 都按三步走，降级不会破坏在途工作：

1. **expand**：只加可空列或新表，新旧版本都能在结果上运行。不就地改列名。
2. **migrate**：在代码里回填，且可重复执行。
3. **contract**：旧结构在没有任何读取之后，至少再晚一个版本才删除。

在预发布版本切换时，Runtime 校验旧增量 v24 或 v25 的完整迁移历史，归档该历史后应用当前基线及后续迁移。版本号相同但校验和不符时拒绝迁移。升级已有存储前用 SQLite 快照保留一次 `state.db.pre-<目标版本>`。从旧版打开 v24 时还会归档已有的会话来源。

在仓库根目录运行 `cargo build -p jeeves-cli`，再运行 `python3 scripts/smoke-storage-cutover.py <旧版state.db> <网关accounts.sqlite>`。脚本以只读方式复制两个库，导入并重新导出临时 Runtime，在原列上比对事项、产物、回执和用量，并核对独立网关账号记录。正式升级前先在副本上演练。

## 故障排查

| 症状 | 处置 |
|---|---|
| `PROVIDER_ERROR (-32000)` 缺 key | 用 `/model add` 补 key（或 export 该配置指向的变量）后重启进程 |
| 401 / 403 | key 失效或地区不可达；`/model add <名字> <厂商> --key 新的` 换掉它 |
| MCP 工具不见了 | 看 stderr 警告：连不上 / 未登录（`jeeves mcp login`）/ mcp.json 坏 |
| 回答里 `[tools used: …]` 重复 | 属预期折叠标记，说明工具活动已入历史 |
| 想知道"它当时为什么这么答" | 打开 `traces/<日期>.jsonl` 对应 turnId 那一行 |
| 私网 Runtime 的 7777 端口被占用 | 单独启动的 Runtime 会尝试下一端口，并在日志中显示实际地址。Mac 开发版和个人部署由系统分配空闲内部端口。 |

## 压缩与看记忆

* REPL 里的 `/compact [重点]` —— 或驾驶舱「记忆」页的**现在压缩** —— 现在就把较早的轮次
  折进滚动摘要，最近两轮留在上下文里。重点是给摘要材料加一句话（「盯着迁移方案」），
  不是换一套提示：摘要必须逐轮可比。
* 记忆中心（设置 → 记忆）三个只读视图。**账本**：一轮一行，点开看这一轮的条目流；
  `折叠` 标的是回填来的轮次，那几轮的工具输出只以摘要形式存在。**记忆库**：上面目录、
  下面正文，`ledger:…` 的 chip 能跳回这页是从哪几轮提炼出来的。**上下文**：六段分层条
  （灵魂 / 记忆 / 技能 / 工具 / 历史 / 摘要），下面是已用、预算，以及到
  `JEEVES_CONTEXT_WARN_RATIO` 时那句提示。
* 那条带子量的是**会随会话增长的部分**。有四样东西每轮都在提示里、但故意不计入：时钟、
  待办板、能力缺口说明（`JEEVES_EXEC` / `JEEVES_WRITE` 没开时那两三行），以及从工作目录到 `.git`
  之间读到的**项目约定**（`AGENTS.md` / `AGENTS.override.md` / `CLAUDE.md`）。前三样各几十个
  token；约定封顶 32 KB —— 所以仓库里有一份很厚的 `AGENTS.md` 时，带子会**低估**真实占用一个
  文件。这是取舍：带子回答的是「一个会话里什么在长」，而按目录恒定的文件不长。
* 执行中，Runtime 按上下文窗口整理模型请求。配置 token 预算可约束单个 turn 的用量。
  [Runtime 配置](configuration.zh.md#行为旋钮)
* `jeeves memory rebuild` 把 `<home>/memory` 下手改过的库页折回事实并重新生成整个库；
  `jeeves memory backfill` 从旧聊天记录重建账本行。

## 崩溃恢复有年龄上限

checkpoint 停在 `open` 意味着上一个进程死在回合中途。启动时 Jeeves 会接着跑 —— 那份活是人
交代的，续上它不算「模型给自己派活」。`JEEVES_RESUME_MAX_AGE`
（分钟，默认 `60`，最大 `10080`）限定这一行最多能有多旧；设成 `0` 就关掉上限。有预算且预算未用完的未结束任务，其检查点不受此限，任何时候都恢复。
被人打断的回合、以及在自己进程里失败的回合同样标成 `abandoned`，不续跑。

为什么是一小时而不是「永远」：checkpoint 是崩溃恢复，真实跨度是秒到分钟。续跑一个两天前
的回合已经不是「同一个回合接着跑」—— 两天前问「今天有什么科技新闻」，两天后给出的答案
回答的是一个已经没人在问的问题，而且为此真的花掉一次模型调用和一次抓取。超龄的行会标成
`abandoned`，启动日志里说清楚：

```
(jeeves) reconcile: turn t… stopped before 2 days (limit 1 hour), not resuming on its own — to carry on, say that sentence again
```

账本里那行照样留着：`abandoned` 是一个状态，不是删除。

## 任务 worker 与租约

`JEEVES_TASK_WORKERS`（1–8，默认 1）决定同时跑几个用户发起的任务；为 1 时行为不变。每个在跑的任务持有一份租约，worker 每 10 秒续一次，30 秒不续即失效。失效的租约只会被清扫一次：任务从未结束的检查点恢复；没有检查点则进入 `needs_attention`。两种情况都会有一条 `lease_expired` 任务事件和一行 stderr。重启会清空所有租约，由已有的启动恢复决定哪些继续。

重启后的任务恢复仍然等 0 号 worker 槽，所以多 worker 时恢复在那里串行。

## 定时任务恢复与投递

任务和定时作业的完成通知按稳定回执键进入本地 webhook 发件箱。即使生产者在入队或确认送达后重启，也只保留一份本地通知。若接收端已经接收 HTTP 请求但最终响应丢失，重试仍可能导致外部重复。

机器睡眠而 Runtime 进程保留，和进程停机不是一回事。调度器发现两次 tick 之间墙钟跳过 90 秒以上，就对这段空档做一次对账，规则与启动补跑相同：最近一次漏掉且未领取的触发只补跑一次（不连环补，最多回看七天）；`catch_up` 关闭的任务只在 stderr 报告、不运行；已领取但未结算的触发绝不重放。用 `task/wait` 声明的等待本来每秒轮询墙钟，所以睡眠期间过期的截止时间会在唤醒后的第一次轮询处理。

`catch_up` 只补未领取的漏触发。已领取但未结算的触发可能已经产生外部效果，重启不重跑，并保留领取以阻止后续分钟覆盖。先停止 Runtime，核对实际效果，再执行：

```sh
jeeves schedule acknowledge morning-news
```

每次触发先以 `schedule:<名称>:<分钟>` 为键记入 wake inbox；重复的键会被跳过，并输出一行 stderr 和一条通知，绝不静默。重启后已认领但未结算的触发转为 `reconcile`，不会重新投递；它只阻塞同名任务，直到人工确认，其他定时任务照常触发。

这只释放已核对的旧领取，不重跑它；未来合法触发可继续。未核对的回合不要通过删库、改时间或重复开始绕过。领取之前落盘失败时不会调用模型。

结果、结算与投递意图保存在 SQLite。outbox 文件未写成功时，用已存内容重试投递；不同的已有文件保留待核对，不覆盖人工修改。启用通知时，文件落地后才向原配置端点入队；失败重启重试，不静默换端点。`produced` 表示已产出，不代表领域验收或用户已读。

## 浏览器接管跨重启

任务浏览器是子进程，随 Runtime 一起结束。重启那一刻若有人工接管，会被记成一条 hold（`browser_holds`）：浏览器会话标为 `needsAttention`，提示里只写站点（仅 origin，绝不写页面 URL），崩溃恢复拒绝在你背后恢复该任务，并有一行 stderr 和一条任务提示。多次重启 hold 都在。只有你为该任务重新打开浏览器才会结束它：Jeeves 启动新浏览器、做一次全新观察，然后才清除 hold，旧页面状态不被信任。只有 `human` 控制算 hold；由代理控制或被策略暂停的浏览器仍按原样直接结束。

## 等待任务输入

在**开始任务**前，选择**等待输入**并说明所需数据，可选设置最晚到达时间。**提供输入**会保存数据；收到数据本身不满足验收条件。任务保留草稿等待，输入到达后恢复，原会话仍可继续聊天。**暂停**阻止自动唤醒，数据仍会保存，只有**继续**才明确授权恢复。继续请求最多等待旧执行收尾十秒；被拒绝时保留暂停状态。

SDK 宿主使用鉴权的 `task/wait` 和 `task/signal` 方法，详见[协议](protocol.zh.md#持久化外部输入)。重试提交时保持事件 ID 和数据不变。输入期限仅限制到达时间；按时收到的数据可以在期限后使用。正常重启运行时即可恢复已提交的事件和草稿；未知工具副作用仍须先核对。

## 核对未知操作

对于已暂停或需要确认的任务，选择**核对操作**。检查真实外部服务或文件，选择已观察到的结果，填写支持证据或外部回执。传输错误本身不能证明没有执行。服务端拒绝保存时表单保留证据。记录结果后任务仍停止；全部未决操作结算后，另行选择**继续**。

SDK 宿主通过[操作协议](protocol.zh.md#操作日志与人工核对)检查和结算。每结算一个操作后重新读取检查点摘要。核对不可伪造 `toolsOk` 成功测量，也不构成服务端幂等保证。

## 给多个用户运行 Jeeves

Web 和 iOS PWA 用户在公网 HTTPS 地址登录，Mac 设备端与同一账号配对。部署 [Go 统一网关](../gateway/README.zh.md)、私网 [Go 控制面](../control-plane/README.zh.md)，以及供租户 Pod 使用的 Rust Runtime 镜像。控制面、PostgreSQL 和 Runtime 均使用私网端点。

个人源码部署运行 `just local-up`，在这台电脑的浏览器中创建所有者。网关和单实例 Runtime 均监听回环接口。
