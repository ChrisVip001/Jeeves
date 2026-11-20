[English](README.md) | 简体中文

# Jeeves — 记忆能翻开的本地 Agent

[![CI](https://github.com/ChrisVip001/Jeeves/actions/workflows/ci.yml/badge.svg)](https://github.com/ChrisVip001/Jeeves/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/github/license/ChrisVip001/Jeeves)](LICENSE)
[![Release](https://img.shields.io/github/v/release/ChrisVip001/Jeeves)](https://github.com/ChrisVip001/Jeeves/releases/latest)

**一个跑在自己机器上的个人助手，记忆是能打开看的。**
在主聊天里可以日常交流、交办新任务，也可以继续跟进原来的任务。[主聊天与任务会话设计](docs/design/main-entry.zh.md)
Rust 独占 agent 循环、记忆和工具；TypeScript 承载共用客户端、聊天网关和独立浏览器适配。对话、`state.db` 和密钥都留在本机。驾驶舱支持中英文。

可以按公钥配对设备、撤销设备授权，见[设备协议](docs/protocol.zh.md#设备配对与撤销)。驾驶舱的[全局急停](docs/protocol.zh.md#全局急停)可冻结受管任务浏览器，重启后仍然生效。可从手机或桌面接管任务浏览器，并[将下载保存为该任务的产物](docs/protocol.zh.md#远程浏览器操作与下载)。

配对后的 iPhone 可通过[任务审批通知](docs/protocol.zh.md#任务审批通知)打开原任务，核对操作范围并决定是否允许。

在 Mac 上，可为一个任务[选择一个窗口](docs/protocol.zh.md#macos-单窗口操作)，点击按钮、填写字段、滚动列表，或预览截图并决定是否发送给屏幕模型。

> 多数 Agent 的记忆是个黑盒。Jeeves **每一轮对话只追加一行账本**：点开这一行，能看到这一轮的条目流——跑了哪些工具、哪个模型答的、花了多少 token。记忆库用 `ledger:…` 回溯到原始轮次，FTS5 索引能从账本重建。

![Jeeves macOS 应用中的 Cowork 工作台](docs/assets/mac-cowork.png)

编码任务可在独立 Git 工作目录中运行已获授权的命令行工具。父任务可以合入已完成子任务的检查点，并运行自己的最终测试。[编码任务协议](docs/protocol.zh.md)

## 安装

Jeeves 支持 macOS 和 Linux（Windows 通过 WSL2）。无论哪种安装，驾驶舱都会打开三步向导，连好模型并设好权限，见[上手指南](docs/guide.zh.md)。

**Mac 应用**——Runtime 自带前端的薄外壳，从源码构建：

```bash
just desktop-install   # 一次性准备
just desktop-build     # 把 Runtime 打包进 Mac 应用
just desktop-dev       # 开发：启动 `jeeves dashboard`；设置 JEEVES_RUNTIME_URL 则连到已运行的 Runtime
```

Mac 左上角可切换「助理」与「Cowork」。助理显示主聊天和近期活动，在 Cowork 里打开任务即可查看完整会话、补充要求。[Mac 桌面应用](docs/operations.zh.md#mac-桌面应用) · [黛青外观预览](docs/design/today-preview.html#p=teal&f=sans)

**Docker**——模型、权限和聊天渠道都在驾驶舱里一步步设置：

```bash
JEEVES_OPS_TOKEN=$(openssl rand -hex 24) docker compose up -d   # -> http://<主机>:7777
```

**源码构建：**

```bash
cd jeeves-rs && cargo build
JEEVES_PROVIDER=deepseek DEEPSEEK_API_KEY=… ./target/debug/jeeves            # 终端对话
JEEVES_PROVIDER=deepseek DEEPSEEK_API_KEY=… ./target/debug/jeeves dashboard  # -> http://localhost:7777
```

每个[发布版本](https://github.com/ChrisVip001/Jeeves/releases/latest)附带 macOS 与 Linux（x86_64 和 aarch64）的预编译 `jeeves` 二进制。没有 API key 也能用 Ollama 离线跑（`JEEVES_PROVIDER=ollama`）。从源码运行时驾驶舱需要先构建（`just dashboard-build`）。

更新正在运行的开发驾驶舱时，按 Ctrl-C 等进程退出，然后在仓库根目录用同一个 `JEEVES_HOME` 执行 `just dashboard-build && just dashboard`。浏览器事件长连接不会阻塞退出；Runtime 就绪后才会显示访问地址。

## Jeeves 能做什么

- **对话**：终端、网页驾驶舱，或聊天网关（微信公众号、飞书、Telegram；见 [docs/guide.zh.md](docs/guide.zh.md)）
- **使用飞书业务能力**：官方 CLI 配合 `feishu-cli` 技能（`just install-feishu-cli`），复用现有命令执行权限和审批
- **记忆**：语义事实（FTS5、支持中文）、带日期的情景、过程 Skills，完整回合按用户消息在本地检索（[docs/architecture.zh.md](docs/architecture.zh.md)）
- **把记忆摊开给你看**：驾驶舱的记忆中心能翻账本、记忆库和上下文占用；`/compact [重点]` 可随时压缩（[docs/operations.zh.md](docs/operations.zh.md)）
- **行动**：23 个内置工具（15 个默认常驻，8 个在开关后面）外加 MCP 服务器（[docs/configuration.zh.md](docs/configuration.zh.md)）
- **接上日历和邮件**：CalDAV/ICS 只读；IMAP 用 `BODY.PEEK` 读信；SMTP 默认只写草稿，每个真实收件人与内容组合都要单独作用域授权并写回执，手机可查看 Mac Cowork 同一任务会话，并在主聊天答复带版本的发送审批。飞书入站事件先落盘再确认。主聊天可将排队中的任务邮件改为可恢复的草稿，普通聊天和任务发件的最终 SMTP 回执丢失均须核对（[docs/configuration.zh.md](docs/configuration.zh.md#日历与邮件)）
- **早上先看一份提议**：可开启 [08:00 晨报](docs/operations.zh.md#晨报cron-友好)，查看当日日程、未读邮件和回复草稿，以及未完成的任务。
- **花得少一点**：系统提示是「稳定前缀 + 每轮尾巴」，供应商能缓存；`JEEVES_TOOL_POLICY=deferred` 时请求里只放常驻工具的 schema（[docs/configuration.zh.md](docs/configuration.zh.md)）
- **长本事，但你说了算**：能力中心把技能、MCP 连接器和定时任务并排摆着，[连接管理](docs/guide.zh.md#连接状态)显示需要你处理的连接，可配置本地邮件/日历及发起支持的 OAuth 授权。可浏览精选目录、接入自定义 MCP 并逐工具启停。常驻运行时自动续期已绑定的 MCP OAuth 登录。MCP 工具的权限可以在[连接器清单](docs/configuration.zh.md#连接器清单)中配置。自己的可以加可以删，内置项只读。
- **按项目规矩干活**：从工作目录往上找 `AGENTS.md` / `AGENTS.override.md` / `CLAUDE.md`，拼进每轮尾巴（[docs/internals/06-app-server.zh.md](docs/internals/06-app-server.zh.md)）
- **不把做到一半的活扔回来**：步数用完而待办未完时继续，最多 `JEEVES_AUTO_CONTINUE_MAX` 次（[docs/configuration.zh.md](docs/configuration.zh.md)）
- **被检验**：确定性 eval 与回放 eval 把守每次发布；judge 给回答质量打分（[docs/internals/08-quality.zh.md](docs/internals/08-quality.zh.md)）

### 内置工具

| 工具 | 用途 |
|---|---|
| `memory` | `action` = save / forget / search / list / update / delete / delete_episode / create_skill |
| `send_message` | 消息草稿进 outbox，从不真的发送 |
| `search_web` | 优先 Parallel MCP，有上限地内置兜底；保留来源与相关段落 |
| `read_page` | 读目录里某一条的全文：记忆库一页或一个技能的完整步骤；默认开 |
| `read_file` | 读文本文件，分页并带行号；默认开 |
| `glob` | 列目录，或按文件名模式找文件；默认开 |
| `grep` | 搜文件内容，返回 `path:line: 文本`；默认开 |
| `find_symbol` / `repo_map` | 有界 FTS5 代码声明导航；默认开（[详情](docs/internals/04-tools.zh.md#文件读查找写补丁)） |
| `fetch_url` | 抓取网页：优先 Parallel MCP 提取，本地 HTTP 兜底并校验公开目标 |
| `job` | 读、列、停后台作业（`action` = output / list / kill）；`JEEVES_EXEC=1` 才有 |
| `todo_write` | 维护一张每轮重新注入的待办清单 |
| `write_file` / `edit` | 写文本文件 / 一次精确匹配替换；`JEEVES_WRITE=1` 才开，范围限于会话工作目录加 `JEEVES_EXEC_WRITABLE_ROOTS` |
| `apply_patch` | 按补丁跨文件修改，全中才落盘；与 `write_file` 同一范围与审批 |
| `run_command` | 沙箱 + 放行表的 shell 命令；`JEEVES_EXEC=1` 才开（见 [SECURITY.zh.md](SECURITY.zh.md)） |
| `delegate_task` | 把自成一体的活交给子代理；`JEEVES_DELEGATE=1` 才开 |
| `browser_observe` / `browser_act` | 可选任务浏览器，单次确认、人工接管；需要安装浏览器组件 |
| `find_tools` | 在延迟加载的工具里搜索，并在本会话里打开命中的工具；默认开 |
| `schedule_job` | 新建或覆盖一个定时任务：名字、五段 cron、一句话；默认开 |
| `list_schedules` | 列出已声明的任务及其 cron 与提示词；默认开 |
| `cancel_schedule` | 按名字删掉一个文件声明的任务；默认开 |

工具失败会以文本返回给模型，循环不会因此崩溃。

### 模型与密钥

内置 12 家 LLM 厂商预适配（`jeeves-provider`），含本地 Ollama。`<home>/models-catalog.json` 可补丁内置默认值或新增一家，`scripts/gen_catalog.py` 从 [models.dev](https://models.dev) 生成这些行（[docs/configuration.zh.md](docs/configuration.zh.md)）。终端里 `/model` 看已配好的套，`/model add ds deepseek deepseek-flash --env DEEPSEEK_API_KEY` 加一套，`/model use ds` 切过去，不用重启；Web 与桌面端偏好设置里有同样的模型菜单。每个模型 profile 显式指定凭据来源（[SECURITY.zh.md](SECURITY.zh.md)）。`JEEVES_OPS_HOST=0.0.0.0` 时启动强制要求 `JEEVES_OPS_TOKEN`（[docs/operations.zh.md](docs/operations.zh.md)）。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/guide.zh.md](docs/guide.zh.md) | 上手指南：安装、向导、模型、权限、聊天网关、技能 |
| [docs/architecture.zh.md](docs/architecture.zh.md) | 原则、crate 依赖图、turn 数据流、设计决策 |
| [docs/protocol.zh.md](docs/protocol.zh.md) | JSON-RPC 方法、人工责任控制与带来源的飞书周报、通知序列与错误码 |
| [docs/configuration.zh.md](docs/configuration.zh.md) | 运行设置、模型密钥轮换与故障恢复、全部 `JEEVES_*` 变量 |
| [docs/operations.zh.md](docs/operations.zh.md) | 运行手册：各种运行形态、备份、故障排查 |
| [docs/limitations.zh.md](docs/limitations.zh.md) | 已知边界，每条都指出下一步该打开哪个文件 |
| [docs/internals/](docs/internals/README.zh.md) | 实现原理教程，一层一章 |
| [CONTRIBUTING.zh.md](CONTRIBUTING.zh.md) | 贡献指南：工程纪律与提交规则 |
| [SECURITY.zh.md](SECURITY.zh.md) | 安全模型：凭证、能力边界、OAuth |
| [CHANGELOG.zh.md](CHANGELOG.zh.md) | 更新日志 |

## 命名

| 用途 | 名字 |
|---|---|
| GitHub 仓库 | `ChrisVip001/Jeeves` |
| 包名 | `jeeves` |
| Rust crate 前缀 | `jeeves-` |
| CLI 二进制 | `jeeves` |
| 助手人格 | Jeeves |
| 环境变量 | `JEEVES_*` |
| 状态目录 | `.jeeves/` |

[English](README.md) | 简体中文 · Code is MIT.
