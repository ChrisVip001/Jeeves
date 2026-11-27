# ⑧ 质量：如何证明系统仍然正确

[English](08-quality.md) | 简体中文

本层没有产品功能，它决定其他章节里的 fail-open 与「绝不裸跑」规则是否真的成立。各层按成本从低到高排列；确定性 eval 100% 通过是发布闸门。

位置：`runtime/crates/jeeves-eval/`、`evals/`、`scripts/`、`justfile`、`.github/workflows/ci.yml`。

## 8.1 分层

| 层 | 位置 | 形态 | 是否拦截 |
|---|---|---|---|
| 单元测试 | 各 Rust crate、`npm test` 和 `go test ./...` | 进程内、离线 | 是 |
| 确定性 eval | `evals/deterministic/*.jsonl` | 脚本化模型驱动真实的 `run_turn` | 是（exit 0 = 可发布） |
| 回放 eval | `evals/replay/*.jsonl` | 录制对话加期望事件序列，同一个 runner | 是 |
| 长任务基准 | `evals/long/tasks.jsonl` | 同一套装置，问自主任务能力有没有变好 | 从不 |
| Judge | `evals/judge/quality.jsonl` | 真实模型作答，裁判模型打 0-10 分 | 从不 |
| 研究质量 | `evals/research/quality.jsonl` | 真实模型、未见过的材料；引文／审查／综合各自评分 | 从不 |
| 冒烟 | `scripts/smoke*` | 真实进程、真实 stdio 与 HTTP | 发布闸门跑其中一部分 |
| 覆盖率 | CI 中的 `cargo llvm-cov` | 行覆盖率下限 60% | 是 |

成本与确定性成反比。单元测试和 eval 是确定性的（模型是假的），但 eval 跑的是真实 `run_turn` 与真实 `state.db`。Judge 分数和基准只看趋势、不做闸门，因为真实模型的输出不稳定。

## 8.2 确定性 eval

入口：`jeeves eval [path]`（默认 `evals/deterministic`；`just eval`）。实现：`runtime/crates/jeeves-eval/src/`（`scenario.rs`、`lib.rs`、`jsonl.rs`、`report.rs`）。

### 用例格式

一个用例是 `.jsonl` 文件中的一行 JSON（允许 `#` 注释行）。`Scenario` 的字段：

| 字段 | 用途 |
|---|---|
| `id`、`description` | 名称与一句话意图 |
| `message` / `responses` / `interrupt_after_deltas` / `cwd` | 单轮简写 |
| `turns[]` | 多轮：每轮有 `message`、`session_id`、`responses`、`apply_patch`、`interrupt_after_deltas`、`cwd`、`restart`、`resume_task`、`approval` |
| `settings` | 单用例的 Settings 覆盖（窗口、沉淀、检索、token、上下文窗口、压缩阈值、exec/write/delegate/plan 开关、exec 允许列表、工具结果预算、soul 模式、工具策略、技能预算、审批模式与超时） |
| `files` | 运行前写入用例 home 的文件 |
| `prereq` | `python3`、`sh` 或 `sandbox`；缺少前置条件时记为跳过，绝不记为通过 |
| `known_gap` | 尚未实现的行为；失败不拦截，通过反而拦截，所以标记不会过期 |
| `task` | 第一轮之前创建的长任务（标题、目标条件、验收、预算、轮数上限） |
| `recovery` | 把长任务基准用例标记为中断恢复用例 |
| `plan` | 规划器的脚本化回复，排在第一轮之前 |
| `expect` | 下面的断言 |

一个响应是 `{"text":"…"}` 或 `{"tool_use":{"name":"…","args":{…}}}`。`files`、`cwd` 与工具 `args` 中的 `{{home}}` 展开为用例临时 home，使用正斜杠，因此在 Windows 上仍是合法 JSON。

任务用例在主会话下创建任务，通过 `task/start` 排队独立执行。用每轮的 `approval` 脚本填写该用例预期的人类审批决定。

Mock 队列是各轮 `responses` 按执行顺序展平，`plan` 排最前。额外的模型调用（摘要、沉淀）按其实际发生的位置入队，所以用例本身记录了真实调用顺序。队列耗尽是错误，而不是重复上一条回复。

`restart` 在同一 home 上进程内重开 runtime，并不杀 OS 进程。`resume_task` 以 `message` 作为控制原因调用仅限人类的 `task/resume` 路径。两者都是仅评测器使用的字段，不是协议或模型工具。

### 断言

| 断言 | 含义 |
|---|---|
| `reply_contains` / `reply_not_contains` | 最终回复包含 / 不包含 |
| `tools_used` | 跨轮累计的有序工具名；空 = 不检查 |
| `tool_output_contains` | 某个工具输出包含该片段（区分「被调用」与「真的成功」） |
| `iterations` | 循环轮数 |
| `consolidation_new_facts` | 各沉淀通知里蒸馏出的事实总数 |
| `approvals` | 提出了多少次审批询问（`0` 是有效断言） |
| `events` | 线上事件的有序子序列，可写简名（`toolStarted`）或限定名（`toolStarted:run_command`、`toolCompleted:run_command:ok`） |
| `interrupted` | 最后一轮是否被中断 |
| `prompt_contains` / `prompt_not_contains` | 针对所有模型请求拼接后的文本 |
| `last_prompt_contains` / `last_prompt_not_contains` | 仅针对最后一次模型请求 |
| `task_status` / `task_checks_passed` / `task_artifacts` | 所建任务的最终状态、验收条件是否全部通过、交付物数量 |

Prompt 断言检查的是模型实际看到了什么：窗口、摘要、技能正文。

### 运行器

每个用例：检查 `prereq`，建临时 home，写入 `files`，用占位 key 打开 server，再把 provider 换成包着 `Mock`（`jeeves-provider`）的记录包装器，它捕获每一次请求。每轮通过事件 sink 跑真实 `run_turn`；中断走 `interrupt_turn`，`apply_patch` 走真实的 `config/write`。套件每个用例打印一行，在 home 下写 `eval_report.json`、`eval_report_<suite>.json` 并追加 `eval_runs.jsonl`，以失败数作为退出码。报告写入失败只告警。

### 写一个用例

一个用例只钉一个行为。按意图选断言：行为用 `reply_*` 和 `tools_used`，接线用 `prompt_contains`，安全边界用 `tool_output_contains`。机器上可能缺的东西加 `prereq`。用 `jeeves eval evals/deterministic/<file>.jsonl` 运行；每个用例必须是单行 JSON。

## 8.3 回放 eval

入口：`jeeves eval --replay [path]`（默认 `evals/replay`；`just eval-replay`）。实现：`runtime/crates/jeeves-eval/src/replay.rs`。

回放行含 `id`、`schemaVersion`、`recorded.turns[]`（`say`、`replies`、`approval`、`cwd`）与 `expect`，以及可选的 `settings`、`files`、`prereq`、`task`。它被转换成同一个 `Scenario`，因此 runner、断言与报告共用。加载器拒绝缺少 `schemaVersion` 的行，也拒绝 `expect.events` 为空的行。

事件按有序子序列匹配，多出来的流式 delta 不会让用例失败。同一轮内，`toolStarted` 随运行推进发出，`toolCompleted` 在末尾一起发出；期望序列按这个顺序写。脚本化的审批回答是 `yes`、`no` 或 `silence`（`remember`：`once`、`session`、`always`）。

五个文件各钉一个边界：`compaction`、`approval`（沉默变成拒绝）、`exec`（`toolCompleted` 之前的增量 `toolDelta`）、`mcp`（断线与重建）、`continuation`（连续工具调用与自然交付）。

## 8.4 长任务基准

入口：`jeeves bench [path]`（默认 `evals/long`；`just bench`）。用例使用确定性格式。两个数字分开记录：任务成功率（达到用例文件声明结果的用例）与中断恢复率（标记为 `recovery` 的子集）。它只记录、始终 exit 0；除共用文件外，报告还写入 `eval_report_long.json`。

## 8.5 Judge

入口：`jeeves judge [path]`（默认 `evals/judge`；`just judge`）。用例含 `message` 与 `judge.criteria`。从环境解析出的真实 provider 回答一轮；裁判模型按固定 rubric 打分，返回 JSON `{"score": 0-10, "reason": …}`。裁判不应是被测模型；`JEEVES_JUDGE_MODEL` 可覆盖默认值。超出 0-10 的分数使评判失败，而不是计入平均。没有 API key 时整组跳过并返回 0。

## 8.6 Research 质量评测

入口：`jeeves research [path]`（默认 `evals/research`；`just eval-research`）。这一层存在的原因是 `evals/results/long-research-validation-2026-11-14.json` 里记录的 11 月 14 日研究把三种不同失败揉成了一个 `comprehensiveQualityPassed: false`：六条引文只精确命中五条、审查回合响应解码失败、综合检查失败。现在三者各算各的分，且用的是那次研究没用过的材料：

- **引文**——每个来源至少要出现该来源点名的一句逐字原文；模型引了同一来源的另一句原文也算命中。机器判定，不采信模型自报。
- **审查**——独立回合必须返回可解析的 JSON verdict；传输或解码失败只扣这一维。
- **综合**——独立裁判按用例判据给简报打 0-10 分。

裁判不应是被测模型；`JEEVES_JUDGE_MODEL` 可覆盖默认小模型（若选手与小模型是同一个 profile，报告里会如实显示两者同名）。每例的草稿与审查回复都记进 `eval_report_research.json`。无 key 时整组诚实跳过；命令始终 exit 0，不拦发版。

## 8.7 真实可靠性示例

`runtime/crates/jeeves-eval/examples/live_reliability.rs` 是显式调用、带预算的真实 provider 运行。必需参数：`--home`、`--budget-cny`（大于 0、至多 10）与 `--out`；`--suite` 选择 `baseline`、`expanded`、`memory`、`research`、`research-guarded`、`support` 或 `research-long`。保留的输出在 `evals/results/`。`just test-research-publication` 离线验证 research 用例的发布闸门。

[11 月 14 日归档清单](../../evals/archive/2026-11-14/sources.json)记录了 4 份原件各自对应的验收报告和来源提交。在干净克隆的仓库根目录运行 `(cd evals/archive/2026-11-14 && shasum -a 256 -c SHA256SUMS)` 即可复验 SHA-256。


离线研究回放经过主聊天和任务运行时：
`cargo test --locked -p jeeves-app-server --lib interaction::tests::research_replay`。
在同一批来源中交换船流摘要与码头吞吐量原始记录的排序，第三组拿掉吞吐量记录，
另测读取首个来源后重开任务。检查结论与引用是否同句、缺证据是否明确说尚未核实、
有没有提前产出终稿，以及恢复后是否重复抓取已成功读取的来源。转载摘要不能代替缺失的指标。
案例取材于[马里兰州港口委员会 2024 年 10 月会议纪要](https://mpa.maryland.gov/Documents/port-commission/MPCMeetingMinutes10012024FINALApproved.pdf#page=4)
和[美国陆军工程兵团航道恢复公告](https://www.nab.usace.army.mil/Media/News-Releases/Article/3802402/us-army-corps-of-engineers-announces-full-restoration-of-baltimores-fort-mchenr/)。
来源为改写后的固定样本，模型行为由脚本指定。它验证运行链路和验收规则，不代表模型会自主选证，
也不验证语义审查或实时网页准确率。


另有显式启用的真实 DeepSeek Flash 回放：
`cargo test --locked -p jeeves-app-server --lib deepseek_research_replay -- --ignored --nocapture`。
通过环境变量提供获准使用的 `DEEPSEEK_API_KEY`，不把密钥写入命令行。
三个聊天对照组各最多四次固定样本网页调用、八次模型请求；重启组先用脚本建立检查点，
再由真实模型决定如何继续。未知 URL 不会被抓取。测试打印读取记录、回复或报告及模型用量供审查。
运行完成不代表质量合格，真实模型也不代表使用了实时网页。见
[11 月 26 日受控回放](../../evals/results/deepseek-flash-research-ranking-replay-2026-11-26.json)。

## 8.8 冒烟脚本

冒烟使用真实的 `jeeves` 二进制，因为它们测的是进程内 eval 绕过的进程边界。

| Recipe | 脚本 | 启动什么 |
|---|---|---|
| `smoke` | `scripts/smoke.sh` | 真实 `jeeves app-server`，经 stdin 喂 JSON-RPC，以 `fake_sse_server.py` 作 provider |
| `smoke-resume` | `scripts/smoke-resume.sh` | app-server 在轮中被 `kill -9`，新进程接着恢复检查点（POSIX） |
| `smoke-recovery` | `scripts/smoke-recovery.py` | 在分发、副作用、检查点边界处 SIGKILL，用本地 provider 与 MCP 夹具（POSIX） |
| `smoke-web` | `scripts/smoke-web.py` | 带认证的 dashboard HTTP：设置、策略、流式、审批 |
| `smoke-interaction` | `scripts/smoke-interaction.py` | 主对话到任务工作区、追问与 worker 审批 |
| `smoke-dashboard` | `scripts/smoke-dashboard.sh` | 用 curl 驱动 `jeeves runtime`（HTTP 到 JSON-RPC、SSE） |
| `smoke-gateway` | `scripts/smoke-gateway.sh` | app-server 加 TypeScript 网关 |
| `smoke-connector` | `scripts/smoke-connector.sh` | `jeeves connector feishu` 对 `fake_feishu_server.py` |
| `smoke-business` | `scripts/smoke-business.sh` | 任务 API、入站 webhook、分级路由（回调用 `fake_callback_server.py`） |
| `smoke-desktop` | `scripts/smoke-desktop.py` | 带脚本化 provider 的 app-server，即桌面壳的用法 |
| `smoke-browser` | `scripts/smoke-browser.py` | 浏览器 worker 测试加离线浏览器/任务流程（Playwright 运行时来自 `browser-runtime`） |

共用辅助在 `scripts/lib.sh`。所有冒烟都离线、使用隔离 home。

## 8.9 CI 与 `just check`

`.github/workflows/ci.yml` 的 job：

| Job | 运行内容 |
|---|---|
| `rust` | `cargo fmt --check`、`clippy --workspace --all-targets -D warnings`、`cargo test --workspace` |
| `rust-coverage` | `cargo llvm-cov --workspace --fail-under-lines 60` |
| `windows` | 构建 `jeeves-cli`；测试排除 `jeeves-tools`，再跑 `jeeves-tools` 并跳过 sandbox 与 spill 测试 |
| `schema` | `scripts/write_schema.py --check` |
| `docs` | `check_tool_docs.py`、`check_doc_paths.py`、`check_env_reads.py`、`check_i18n.py`、`check_config_docs.py` |
| `ts` | 在仓库根目录运行 `npm ci`、`npm run typecheck`、`npm test` |
| `desktop`（macOS） | `just desktop-check`、`just smoke-desktop` |
| `gate` | 构建 `jeeves-cli` 与 dashboard，然后 `jeeves eval`、`jeeves eval --replay`、`smoke.sh`、`smoke-recovery.py`、`smoke-web.py` |

`just check` 是本地的超集：`fmt-check`、`check-app-server-schema`、`check-doc-paths`、`check-doc-links`、`check-env-reads`、`check-tool-docs`、`check-config-docs`、`check-i18n`、`ts-check`、`eval`、`eval-replay`，然后是 `clippy -D warnings` 与 `cargo test --workspace`。提交前运行。网关、dashboard、connector、business、interaction、resume 与 browser 冒烟只是本地 recipe。

### 文档检查

| 脚本 | 失败条件 |
|---|---|
| `check_doc_paths.py` | 文档（`planning` 之外）里反引号包裹的仓库路径不存在 |
| `check_doc_links.py` | 相对链接失效，或英文文档缺少标题层级相同的 `.zh.md` 孪生 |
| `check_tool_docs.py` | 工具文档或 README 中的工具清单与数量和代码注册表不一致 |
| `check_config_docs.py` | 代码读取的 `JEEVES_*` 缺失于 `.env.example` 或配置文档，或反之 |
| `check_i18n.py` | dashboard 英文表缺少已用字符串、保留了未用字符串，或占位符不一致 |
| `check_env_reads.py` | `jeeves-config` 与脚本内宿主级白名单之外出现进程环境变量读取 |

### 协议产物

`just write-app-server-schema` 从 `jeeves-protocol` 重新生成 TypeScript 类型与 JSON Schema；`just check-app-server-schema` 是漂移检查。见 [01-protocol-and-config](01-protocol-and-config.zh.md)。

## 8.10 已知缺口

见 [limitations](../limitations.zh.md)。Judge 需要真实 key，不在 CI 中运行。发布闸门之外的冒烟不在 CI 中运行。

相关：[architecture](../architecture.zh.md)、[protocol](../protocol.zh.md)、[configuration](../configuration.zh.md)、[operations](../operations.zh.md)、[03-model-and-loop](03-model-and-loop.zh.md)、[06-app-server](06-app-server.zh.md)、[07-shells](07-shells.zh.md)。
