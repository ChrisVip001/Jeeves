# AGENTS.md

[English](AGENTS.md) | 简体中文

Jeeves 是能处理日常事务和复杂委托的通用个人助理。Rust 持有助理逻辑与状态，Go 负责网关与控制面，TypeScript 负责浏览器与渠道适配。改 `runtime/` 前先读 [docs/architecture.md](docs/architecture.zh.md)。本文件是 AI 助手与贡献者共用的仓库规范。

## 如何选择下一项 agent 改进

以[主会话与任务设计](docs/design/main-entry.zh.md)为准，从每次请求要达成的结果、用户约束和验收证据出发；指令模糊时，Jeeves 先把它总结澄清成明确目标，再交付结果。改进周期事务、中断恢复或交付核验时，参考[长任务路线图](docs/planning/long-task-roadmap.zh.md)。托管和自托管部署共用引擎契约。脚本回归、真实模型表现与部署验收分别记录。可选能力由用户配置开启，Shell 命令在沙箱内执行。

## 仓库布局

```
protocol/               跨语言契约、生成的 schema 和共用 TS 传输辅助代码
runtime/crates/         助理执行、状态、工具、记忆、HTTP 和 CLI 的 Rust crate
gateway/                Go 统一网关，管理账号、会话、设备和 Runtime 路由
control-plane/          Go 私网服务，负责租户实例、配额和模型用量
internal/cloudkit/      Go 服务共用代码
web/                    浏览器工作台和账号界面
desktop/                Tauri Mac 设备客户端和源码开发外壳
integrations/channels/  渠道适配器及其 Node Runtime 客户端
executors/browser/      浏览器任务执行器
deploy/                 部署清单
evals/                  确定性、回放、长任务和 judge 用例
docs/                   架构、运维和实现指南
scripts/                协议生成、仓库检查和端到端冒烟测试
```

## 命令

和用户一起测试 Mac 功能时，使用用户正在操作的 Jeeves 窗口和 Runtime。首次交互前核对应用 PID，后续操作继续指向同一实例。仓库构建的应用使用[共享启动入口](docs/operations.zh.md#mac-桌面应用)。测试结束后保留窗口，方便用户接着操作。需要临时数据和进程重启的场景另有独立的自动回归命令。

优先使用仓库已有的命令和工具。引入新工具时，说明现有流程无法满足哪项需求。

```sh
just check                    # 全部门禁：fmt + schema 漂移 + 工具/配置/i18n 文档漂移
                              # + ts-check + eval + 回放 eval + clippy -D warnings + cargo test
just check-tool-docs          # internals/04-tools.md 的工具表 vs. 代码里的工具名单
just check-i18n               # 驾驶舱 t("…") 的文案 vs. i18n.ts 里的英文表
just eval / just judge        # 确定性 eval（exit 0 = 可发布）/ judge 出分
just write-app-server-schema  # 改协议后必跑；just check-app-server-schema 为 CI 漂移检查
just smoke smoke-recovery smoke-web smoke-gateway smoke-dashboard   # 端到端冒烟

# 日常 Rust 循环：按 crate 构建和测试，不用 --workspace --all-targets
cargo build -p jeeves-cli              # CLI / 驾驶舱入口
cargo test -p jeeves-app-server        # 改动 crate 的引擎测试
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
- **一个引擎、多个外壳**：Rust `app-server` 独占逻辑、状态与权限判定，`jeeves-protocol` 是唯一契约。外壳只是适配器，不得各自实现 agent 行为。平台边界与两个 OS 适配例外详见[架构文档](docs/architecture.zh.md#设计原则)，由 `check_architecture` 强制。
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

- 正式上线前，接口变更时一次性切换调用方，并在同一改动里删除被替代的路由、实现和使用说明。
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
