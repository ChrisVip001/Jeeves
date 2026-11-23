# 贡献指南

[English](CONTRIBUTING.md) | 简体中文

## 开发环境

```bash
just build                       # 在仓库根目录执行 cargo build --workspace（Rust ≥ 1.100）
just ts-install
```

## 工程纪律（合并前必须全绿）

```bash
just check
```

等价于依次执行：`fmt-check` → `check-app-server-schema` → `check-doc-paths` →
`check-doc-links` → `check-env-reads` → `check-tool-docs` → `check-config-docs` →
`check-i18n` → `ts-check` → `eval`（release gate）→ `eval-replay` →
`cargo clippy -D warnings` → `cargo test --workspace`。`just help` 列出全部配方。

## 改动的黄金规则

1. **改了协议必须重新生成**：`just write-app-server-schema`，生成物
   check-in，漂移会被 `check-app-server-schema` 在 CI 拦下。
2. **clippy 零告警**：`-D warnings` 是门禁，不是建议。
3. **行为改动配测试**：修 bug 在复现它的测试旁写回归；新行为至少一条
   确定性测试。测试放 crate 内（单元）或 `evals/deterministic/`
   （跨模块组装）。
4. **工具错误是文本不是异常**：`ToolRegistry::execute` 永不 Err。
5. **失败开放的地方要有测试钉住**：本地记忆检索要覆盖命中、失败和超时。
6. **允许新增依赖，但要有理由**：比手写实现或标准库更合适时就引入；先看工作区里有没有等价物（URL 编解码、哈希这类小工具已有），改动保持最小，并说明它满足哪个需求。

## 提交

一次提交一件事；提交信息用一句话概括行为变化（不是文件清单）。
`just check` 全绿再推。
