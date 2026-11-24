# 上手指南

[English](guide.md) | 简体中文

使用部署提供的链接注册或登录，就可以提问、交办或跨设备跟进事项。[使用主对话](design/main-entry.zh.md)

## 1. 安装

| 用法 | 开始使用 |
|---|---|
| 托管 Web 和 iOS PWA | 打开部署的 HTTPS 地址，注册或登录。首次开通时等待工作区启动。 |
| 单机源码部署 | 在仓库根目录运行 `just local-up`，随后在打开的页面创建所有者账号。 |
| Mac 应用 | 托管版在浏览器确认设备配对。开发版接入同一个 Go 网关。见 [Mac 桌面应用](operations.zh.md#mac-桌面应用)。 |

## 2. 首次启动向导

单机部署登录后，尚未配置模型时会打开向导：

1. **连接一个模型。** 选服务商、粘贴密钥。密钥进系统钥匙串；Ollama 不需要密钥。
2. **让 Jeeves 认识你。** 你的称呼和最近关心的事会存成记忆，你可以随时打开、编辑。

然后随便说点什么。一个好的第一句：“总结我下载文件夹里的文件，并建议怎么整理。”

## 3. 模型

在 Jeeves CLI 中使用 `/model` 管理配置，切换后下一轮生效。一套配置 = 服务商 + 模型名 + 密钥来源。密钥优先进系统钥匙串，没有可用钥匙串时退回 `<home>/secrets.json`（0600 明文）；配置也可以改成指向某个环境变量（`env:VAR`），只读取、不复制。保存的应答会说密钥实际落在哪。

```
/model                                       列出配置（★ = 当前）
/model add ds deepseek deepseek-flash --env DEEPSEEK_API_KEY
/model use ds                                切换
```

添加时用 `--vision` / `--no-vision` 声明是否支持图片，否则按厂商默认行决定。本地推理可以装好 Ollama，[下载默认模型](configuration.zh.md#本地推理ollama)，再添加一套 `ollama` 配置。按轮路由和模型配置选项见[配置参考](configuration.zh.md#多套模型与它们的密钥homemodelsjson)。

## 4. 权限

在对话内核对审批卡的动作、工作目录和预览，再选择允许一次或拒绝。Runtime 的命令、写文件和委派开关由部署方管理。会话权限模式见[权限协议](protocol.zh.md)，沙箱边界见[安全模型](../SECURITY.zh.md)。

## 5. 渠道：飞书与 Telegram

两个网关都连到 Jeeves Runtime，会话 id 从平台身份派生。

### 连接状态

从精选目录查看服务商文档，或添加自己的 MCP 连接。远程服务填写 URL 并选择无认证、Bearer 环境变量或浏览器 OAuth。本地 stdio 服务填写命令和参数。服务商要求额外配置的条目提供设置指引。展开 MCP 卡片的工具权限可以查看 effect、禁用或恢复用户连接的工具。改动从下一回合生效，重新启用的工具继续遵守原审批规则。

从设置打开 **连接管理**，集中查看已配置的能力，需要处理的项排在前面。卡片显示本地检查状态、凭据位置（绝不显示值）、最近检查时间及下一步动作。检查可能使用 60 秒缓存，不代表远程服务已接受请求。在 **本地连接器** 中添加或编辑日历、邮件；密钥单独存入凭据库，再引用 `vault:NAME` 或 `env:VAR`。已配置 OAuth 的 MCP 或飞书用户连接可选择**开始授权**，在运行 Jeeves 的设备打开真实链接（回调在该设备的本地回环地址监听）。完成服务方同意页面后重新检查；启动失败时提供 CLI 指引。不会虚构二维码流程。协议状态见[能力卡](protocol.zh.md#能力卡)。

### 飞书 / Lark

飞书走长连接，不需要公网地址。在开发者后台创建自建应用，记下 App ID 与 App Secret，然后：

1. 开启**机器人能力**。
2. 订阅 `im.message.receive_v1`。
3. 订阅方式选**长连接**，不是“推送到你的服务器”。
4. 要用审批卡，还要订阅回调 `card.action.trigger`。

前三项缺任何一项，都会表现为“已连接、永远没动静”，且没有任何报错。然后打开菜单 → **连接管理**，填 App ID 与 App Secret（密钥进钥匙串），打开网关。Runtime 会替你启动它、显示状态和最近日志，并随 Runtime 一起重启；需要安装 Node.js。白名单留空，网关谁都不回：第一个来发消息的人会作为候选出现，点一下确认为所有者。国际版 Lark 请把域设为 Lark。

白名单里填的是 **open_id**（`ou_…`），不是 App ID。日志出现 `outside the allowlist` 说明发送者的 open_id 没匹配上；同一个人在不同应用里的 open_id 不同。群里机器人只在被 @ 时回答。

白名单只有一人时，这位所有者在私聊或普通群聊中发起的操作若需审批，Jeeves 会在原对话回复审批卡，显示操作内容和有效时间。原发起人可点击按钮，也可按卡片提示发送确认文字。卡片发送失败时，Jeeves 会改发文字审批。群聊用文字回复时需同时 @机器人。白名单内的派发者也可在私聊使用同样的流程。请求过期或网关重启后，先核对操作状态，再按需重新发送命令。

不用驾驶舱时，设好 `LARK_APP_ID`、`LARK_APP_SECRET`、`LARK_ALLOW` 后运行 `just gateway-lark`；设 `JEEVES_GATEWAY_URL`（再加相同的 `JEEVES_OPS_TOKEN`）则接到已运行的 Runtime，而不是另起引擎。变量见[配置参考](configuration.zh.md#渠道适配器)。

#### 从群里派任务

把能派活的人写进 Runtime 的 `JEEVES_DISPATCHERS`（或连接管理界面）。其中一人在群里 @ 机器人时，Jeeves 可以创建一个任务，任务记住这个群、这条消息和发送者。群里只听到回执与最终报告；进展留在任务板上。其他人仍会得到回答，但在单独的会话里，没有 `task` 工具，也看不到所有者的记忆。名单由 Runtime 自己检查，网关无法放宽。

被派发的回合需要审批时，网关在该话题里发一张交互卡。只有名单内派发者的点击有效，且只答一次；聊天永远无法授予常设规则。

#### 通过官方 CLI 做飞书业务

网关只承载对话。要用飞书业务 API，请通过 `run_command` 与自带的 `feishu-cli` 技能使用[官方 lark-cli](https://github.com/larksuite/cli)：

```sh
just install-feishu-cli                     # 固定版本，装在 .jeeves/tools/lark-cli
just feishu-cli auth status                 # 复用 CLI 自己的登录
export JEEVES_SKILL_DIRS="$PWD/skills"
export JEEVES_EXEC=1 JEEVES_EXEC_NETWORK=1 JEEVES_APPROVAL=on-request
```

带着这些变量重启 Runtime。没匹配的命令会请求审批；不要只为 CLI 就授予完全访问。CLI 有自己的账号与凭据，与 `LARK_*`、`connectors.json` 无关。如果令牌刷新需要可写缓存目录，只把那个目录加进 `JEEVES_EXEC_WRITABLE_ROOTS`。

### Telegram

Telegram 用长轮询，不需要公网地址。先用 @BotFather 建一个 bot，然后：

```sh
TELEGRAM_BOT_TOKEN=… TELEGRAM_ALLOW=<你的数字用户 id> just gateway-telegram
```

token 只从环境变量读取。`TELEGRAM_ALLOW` 列数字用户 id；不设则找到这个 bot 的人都能用。`TELEGRAM_CLAIM=1` 表示谁都不回答，只把第一个发信人记成候选。只回答私聊。审批会以带 **Approve once** / **Deny** 按钮的消息发给名单里的每个人；Runtime 接受第一个回答，拒绝过期的。按钮键只存在内存里，所以网关重启前发出的审批不能再用旧按钮回答。更新偏移保存在 `<home>/telegram-offset`。这个网关还没有对着真实的 Telegram 服务跑过。

## 6. 记忆

Jeeves 保存语义事实、带日期的情景和技能，完整回合使用本地关键词检索匹配的记忆。这一切都可以检视：设置里的**记忆**页有**账本**（一轮一行，点开看这一轮的条目流）、**记忆库**（目录与一次一页的正文，`ledger:…` chip 跳回来源轮次）和**上下文**计量（会随会话增长的部分，到 `JEEVES_CONTEXT_WARN_RATIO` 时给提示）。**现在压缩**按需把较早的轮次折进滚动摘要。

记忆库是 `<home>/memory` 下的纯文本：手改一页，`jeeves memory rebuild` 会先把你的修改收回事实、再重新生成其余；`jeeves memory backfill` 从旧聊天记录回填账本行。这台机器上的其他 agent 可以通过 `jeeves mcp serve` 共享同一份记忆（[运行手册](operations.zh.md#把-jeeves-的记忆给别的-agent-用)）。

## 7. 技能

技能是过程记忆：一个 `SKILL.md`，描述一类任务**怎么做**，相关消息出现时自动加载。新 home 第一次启动会得到两个起步技能：`summarize-document`、`tidy-folder`。它们就是 `<home>/skills/` 里的普通文件；改了、删了就保持那样。

仓库的 `skills/` 目录放着可选技能，用 `export JEEVES_SKILL_DIRS="$PWD/skills"` 加载。其中 `dashboard-verification` 是改驾驶舱时的验收清单：它记下要证明哪些功能、用什么命令证明、以及要留什么证据。改完驾驶舱、宣布完成之前先过一遍。

```markdown
---
name: weekly-review
description: 汇总这一周并起草周一简报
---

1. 从记忆里取上周的情景。
2. 提炼成三条。
3. 把摘要起草进 outbox。
```

### 技能怎么被选中

每轮都带一份「一个技能一行」的目录（受 `JEEVES_SKILL_INDEX_TOKENS` 限制）；技能正文只在需要时进入提示：你用 `$weekly-review` 点名，或它的名字加描述与你的消息至少有两个不同的词重合（至少三个字符的 ASCII 词，或相邻的汉字对），或模型看了目录后用 `read_page` 读取（单个正文受 `JEEVES_SKILL_BODY_TOKENS` 限制）。所以把 `description` 写成触发条件。技能引用的文件只在模型要时才读。命中会追加到 `<home>/skill-hits.jsonl`，并在驾驶舱的**技能**下汇总。

两个可选的 frontmatter 字段设策略：`allow-model-invocation: false` 让技能不因关键词重合而触发（只有用 `$name` 点名才加载），`dependencies: a, b` 列出必须存在的技能，缺了就不会隐式触发。`version: 1.2.0` 开启更新，`schedule: 0 8 * * 1` 让技能本身成为[定时任务](#8-定时)。描述过程，别借工具：导出到别的 agent 后，Jeeves 的内置工具并不存在。

### 安装、更新与分发

```bash
jeeves skill list                                    # 已加载的技能
jeeves skill install <SKILL.md 的 url 或本地路径>     # 先校验；从不覆盖
jeeves skill update [index.json 或 url]              # 只更新到更新的版本；默认 <home>/skills/index.json
jeeves skill export --to claude,codex                # 复制到其他 agent（--project、--force、--names a,b）
```

更新先校验、原子替换、旧版本留在 `.backup/`，也从不降级。目录约定是 `<home>/skills/<name>/SKILL.md`，名字为小写 slug；重名时 `<home>/skills` 胜过 `JEEVES_SKILL_DIRS`。在对话里教会 Jeeves 一套流程后，说“把这个存成技能”，它会调用 `create_skill`（只在你同意之后，且从不覆盖）。

设置的**技能**页列出每个已加载的技能及其版本、策略与缺失依赖，并允许添加或删除 `<home>/skills/` 里的技能。来自 `JEEVES_SKILL_DIRS` 的技能是内置的、只读。安装在下一轮生效。

## 8. 定时

在对话里说：「每个工作日早上 8 点把今天的日历发给我」。查看时间、来源和投递位置后确认。之后可以在同一对话中修改或暂停。见[周期委托](design/main-entry.zh.md#6-周期委托)。

## 9. 凭据

菜单 → **连接管理** → **凭据库**以大写名字（字母、数字、下划线）存一个密码或令牌。Jeeves 的命令能用它但读不到：模型在命令里写 `$JEEVES_SECRET_GITHUB_TOKEN`，Runtime 把值注入那一个进程，并从输出中抹掉。值存在钥匙串里；列表只显示名字。它只覆盖 `run_command`，见[限制](limitations.zh.md#凭据库只管命令)。

## 10. 收件箱

设置 → **收件箱**是一份只读的列表，汇总所有在等你的事：需要你拍板的任务，以及 Jeeves 留在 `<home>/outbox/` 的草稿（简报、被要求写但从不发送的消息）。审批在对话里有自己的卡片。

尚未创建的草稿目录视为空。如果已有的草稿目录无法读取，页面会显示错误，修复后可点击刷新。

## 11. 在本地与云端之间搬家

Jeeves 知道的一切都在一个文件夹里，也就是它的 *home*（默认 `./.jeeves`，或 `JEEVES_HOME`）：数据库、记忆、技能、设置。这样搬：

```bash
jeeves home export <dir>      # 导出到一个新的空目录；Jeeves 运行中也安全
jeeves home import <dir>      # 导入到一个空的 JEEVES_HOME；较旧的 schema 会被迁移
```

同一对命令可以在机器之间、也可以在本地安装与托管 Pod 之间搬家。密钥与登录不导出（`secrets.json`、`mcp-auth/`、`connector-auth/`）；导入后重新添加。多用户托管见[运行手册](operations.zh.md#给多个用户运行-jeeves)。

## 进一步

[配置参考](configuration.zh.md)（给运维）· [运行手册](operations.zh.md) · [安全](../SECURITY.zh.md) · [限制](limitations.zh.md)
