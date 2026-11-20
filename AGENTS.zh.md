# AGENTS.md

[English](AGENTS.md) | 简体中文

Jeeves 是一个本地优先的个人助手：Rust 独占逻辑与状态，TypeScript 负责浏览器
与聊天网关。改 `jeeves-rs/` 前先读
[docs/architecture.md](docs/architecture.md)；本文件是 AI 编码助手与人类
贡献者共用的仓库工作规范。

## 仓库布局

```
jeeves-rs/
  jeeves-protocol/      跨语言契约，唯一事实来源（schema/ 生成物 check-in）
  jeeves-config/        JEEVES_* 环境变量，启动读一次
  jeeves-state/         SQLite + FTS5(trigram)；只追加的会话账本（ledger）；记忆后端契约与 conformance
  jeeves-provider/      12 家厂商（含本地 Ollama），三种 wire format + SSE，重试与 token 估算、
                        cached token 记账
  jeeves-tools/         工具注册表（带 schema 校验）+ 内置工具、沙箱执行、子代理、批准、
                       待办清单、后台作业宿主接口、延迟加载（find_tools）
  jeeves-agent/         确定性的任务辅助（verify / observe / plan / paths），不联网
  jeeves-connector/     日历、邮件（IMAP/SMTP）与飞书连接器：白名单硬边界，默认只出草稿
  jeeves-loop/          agent 主循环（可打断）
  jeeves-mcp/           MCP 客户端（stdio/HTTP）+ 浏览器 OAuth + 记忆服务器（`jeeves mcp serve`）
  jeeves-memory/        本地检索 + consolidation + Skills + `home/memory` 的 OKF bundle
  jeeves-app-server/    JSON-RPC over stdio，每个 state.db 只运行一个执行 Runtime，一轮的全流程、批准等待表、
                       钩子落地、待办注入、后台作业与目标循环；账本写入、后台整理 worker、
                       记忆中心（memory_center）与稳定前缀/动态尾巴的提示装配
  jeeves-ops/           驾驶舱后端（axum + SSE）
  jeeves-cli/           jeeves 二进制（REPL / dashboard / eval / mcp / skill）
  jeeves-eval/          确定性 eval + judge + release gate
jeeves-ts/                 @jeeves/client、@jeeves/gateway、@jeeves/dashboard、@jeeves/ui、
                        @jeeves/browser-worker
desktop/                Tauri macOS 外壳（src-tauri），包住 Runtime 自己的前端（启动 `jeeves dashboard` 并加载其地址）
evals/                  确定性、回放、长任务与 judge 用例（JSONL）
docs/                   guide / architecture / configuration / protocol / operations / limitations（含 internals/）
                        internals/ —— 实现原理教程（每层一章）
scripts/                schema 生成、文档漂移检查、端到端冒烟与本地 fixture
```

## 命令

优先使用仓库已有的命令和工具。引入新工具时，说明现有流程无法满足哪项需求。

```sh
just check                    # 全部门禁：fmt + schema 漂移 + 工具/配置/i18n 文档漂移
                              # + ts-check + eval + 回放 eval + clippy -D warnings + cargo test
just check-tool-docs          # 06-tools §6.0 与 README 的工具表 vs. 代码里的工具名单
just check-i18n               # 驾驶舱 t("…") 的文案 vs. i18n.ts 里的英文表
just eval / just judge        # 确定性 eval（exit 0 = 可发布）/ judge 出分
just write-app-server-schema  # 改协议后必跑；just check-app-server-schema 为 CI 漂移检查
just smoke smoke-recovery smoke-web smoke-gateway smoke-dashboard   # 端到端冒烟
```

## 不可违反的不变量

- **协议单一定义**：传输类型统一定义在 `jeeves-protocol` 的 RPC 信封及协议模块中，
  改动后必须跑 `just write-app-server-schema` 并提交生成物；wire 字段
  camelCase、整数一律 `i32`、新类型登记进 `export.rs` 的清单。
  修改生成文件时，只改源文件，再用仓库的生成命令重新生成。
- **工具错误是文本，不是异常**：`ToolRegistry::execute` 永不返回 Err。
- **失败开放必须配测试**：本地记忆检索失败或超时后，回合应在原有权限下继续。
- **clippy 零告警**（`-D warnings`）；crate 目标 <500 行/模块。
- **环境变量提供默认值，已保存的设置可覆盖支持的选项**；`config/write` 的持久化补丁走校验，非法值
  整体拒绝。
- **不新增重型依赖**：先确认工作区里没有等价物（URL 编解码、哈希等
  已有实现）。
- **模型凭据**按 profile 指定的来源读取。见[凭证处理](SECURITY.zh.md#凭证处理)。应答和日志只包含来源指针。
- **消息草稿**：`send_message` 只写入草稿箱。
- **扩展一律注入**：工具层只认 trait（`SubagentRunner` / `ApprovalBroker` /
  `HookRunner` / `JobRegistry`），实现全在 app-server，`ToolCtx` 只加字段。
  工具层不认识 app-server —— 依赖方向反过来就会成环。
- **一个引擎、多个外壳**：Rust `app-server` 独占逻辑、状态与权限判定，
  `jeeves-protocol` 是唯一契约。Web 与桌面端只是适配器（渲染 + 传输 + 原生能力），
  不得各自实现一遍 agent 行为；平台差异只允许出现在壳与适配层，引擎里不许有
  `#[cfg(target_os = …)]`、能力判断或状态，壳之间也不许互相 import。引擎里唯一的例外是
  OS 能力适配——凭证存储（`jeeves-config/src/models.rs`）与打开 OAuth 浏览器
  （`jeeves-connector/src/feishu_oauth.rs`）；`check_architecture` 只放行这两个文件，
  新增例外必须在此写明理由。所有 WebView
  前端都放在 `jeeves-ts/` 下，共用 dashboard 的 `src/ui` 组件与生成的协议类型；将来的原生移动端
  通过 FFI 复用同一个引擎与契约，而不是复用 Vue 组件。增加平台只增加壳，不增加引擎。
- **能力默认值**见[配置](docs/configuration.zh.md)。命令与子代理需要开启；交互式驾驶舱默认开启文件工具并按需审批。
- **人工授权**：审批决定与目标启动由人发起。主聊天可将用户消息中的原文片段派给任务。记忆工具依照本轮权限维护事实和技能；启用外部记忆 MCP 后，接入的客户端也可保存和遗忘事实。
- **不静默**：重试、熔断、跳过重复写入、目标续轮、后台作业结束、钩子被信任哈希
  挡下 —— 每一条都要有一行 stderr 或一条协议通知。用户对着不动的界面猜原因，
  是我们这一轮反复在修的同一类问题。

## 代码库就是智能体的记忆

- Rust / TS 源码里不写普通 `//` / `/* */` 注释，`check-comments` 会强制检查。
  文档注释（`///`、`//!`、`/** */`）是结构性信息，保留。
- 反复出现的错误靠改结构或写会失败的 lint / 测试来根治，不留「下个智能体请照此办理」的便签。
  确需例外只在 `scripts/check_comments.py` 的 `ALLOWLIST` 里登记并写明理由。

## 避免不必要的防御式代码

- 在系统边界校验不可信输入；系统内部依赖已有类型、契约与不变量，避免重复检查。
- 仅在现有契约、已知故障场景或任务要求需要时添加兜底、重试、兼容分支或默认值。
  保留上述失败开放与配置校验要求。
- 传播错误并提供有用的上下文，不吞掉错误，也不让无效状态看起来像成功；遵循现有的
  工具错误与失败开放契约。
- 优先采用满足当前需求的最简单实现，避免为假设中的未来需求添加抽象或扩展点。
  检查新增的校验和错误处理是否对应具体边界、故障场景或文档要求。
- 如果修复需要扩大任务范围，或改变任务未授权变更的契约，先说明触发条件、影响和
  方案取舍，由用户决定后再实施。
- 当抽取函数能消除重复，或能给复杂逻辑起一个让调用处更易读的名字时，再拆函数。
  简单的前置检查直接写在调用处即可。
- 注释只解释从代码本身看不出的约束、原因或反直觉的行为。

## 测试

行为改动配测试：单元测试放 crate 内，跨模块组装行为放
`evals/deterministic/*.jsonl`（一行一个用例，支持多轮/prompt 断言/
打断/配置注入）。修 bug 先在测试里复现。`jeeves eval` 失败阻塞发布。

测试运行真实实现，断言可观察的行为，避免在测试里复制业务逻辑来计算预期结果。
需要控制外部依赖或稳定触发故障时可使用测试替身。例如，模拟记忆查询失败，断言回合可以继续。

## 文档

用户可见的行为改动同步更新 README 与 `docs/` 对应分册，同一次改动里完成。
文档以**英文为主**（`*.md`），每份文件配有中文对译（`*.zh.md`），两份一起改；
术语以英文版为准。一段一个事实，不叙述控制流。

## 文档规则

- 新事实只写在唯一的位置，其他地方链接过去。
- 计划与评审只在未落地时放在 docs 下的 planning 目录；落地后决定写进正式文档，文件删除（历史留在 git）。
- 每份英文文档都有标题结构一致的 `.zh.md` 孪生；`just check-doc-links` 检查，并检查相对链接可达。
