# ① 协议与配置：`jeeves-protocol` + `jeeves-config`

[English](01-protocol-and-config.md) | 简体中文

两个小 crate 位于其余一切之下。`jeeves-protocol` 是引擎与客户端之间契约的唯一定义；`jeeves-config` 是唯一知道有哪些配置项的地方。参考表在别处：方法与通知见 [protocol.md](../protocol.zh.md)，全部 `JEEVES_*` 变量见 [configuration.md](../configuration.zh.md)。

## 1.1 协议：一处定义，生成副本

契约是 `protocol/src/protocol/v2.rs`（以及 `memory_center.rs`）里的 Rust 类型，加上 `protocol/src/rpc.rs` 里的 JSON-RPC 信封。信封遵循 JSON-RPC 规范，与业务类型分开，这样改字段时不必核对信封兼容性。

TypeScript 类型、JSON Schema 和常量表都由这些类型生成，并提交在 `protocol/schema/` 下。CI 会重新生成并比对，改了类型没重新生成就构建失败。

### 线上约定

- 后缀：`*Params`（请求）、`*Response`、`*Notification`（推送）。
- 线上字段一律 camelCase。
- 整数一律 `i32`。ts-rs 把 `i64` 映射成 `bigint`，而 `JSON.parse` 得到的是 `number`；不一致能编译通过，运行时才出错。存储层用 `i64`，在边界处收窄（app-server 的 `narrow()` 钳制到 `0..=i32::MAX`）。
- `ServerNotification` 是内部打标签的枚举（`tag = "type"`）。传输层把每个变体包进 JSON-RPC 方法 `turn/notification`。

### 信封细节（`rpc.rs`）

- `RequestId` 是无标签的 `Number(i32) | String`，两种 id 风格的客户端都能解析。它派生 `Hash`，因为服务端用它作待决请求的键。
- `JsonRpcError.id` 是可选的：请求本身无法解析时没有 id 可回显。
- `codes` 包含标准 JSON-RPC 区间和 Jeeves 自己的：provider 错误、工具错误、`NOT_IMPLEMENTED`、`SESSION_BUSY`。后两者与 `METHOD_NOT_FOUND` 分开，因为「没做」「被占用」「拼错了」需要客户端做出不同反应。`SESSION_BUSY` 带着占用会话的那一轮的 id。

## 1.2 生成流水线

```
v2.rs 类型 ──(cargo test，被忽略的测试)──> TS + JSON Schema + 常量
                                        ──(prettier)──> 提交到 schema/
```

- `export.rs` 里的 `protocol_types!` 是导出类型的唯一清单。桶文件 `typescript/v2/index.ts`、展平的 `json/v2.json` 和各 schema 都由它驱动。漏写是静默的：该类型就是不出现。每个新类型都要加进这份清单。
- 方法名和错误码是值而不是类型，所以 `export.rs` 直接从 `methods` 和 `codes` 读取，写出 `typescript/v2/methods.mts`、`codes.mts` 与 `json/consts.json`。它们用 `.mts` 是因为导出运行时值；`ts-rs` 的输出只有类型，保持 `.ts`。不要往该目录加 `"type": "module"` 的 `package.json`：那会改变所有生成文件的模块类别。
- `json/v2.json` 是 draft-07，所有定义都在顶层，使用方得到的是平铺的模型。

### 为什么非测试构建里 derive 是空操作

`lib.rs` 在真实 derive（`cfg(test)`）与 `jeeves-protocol-noop-macros` 的空 derive（其余情况）之间切换 `TS` / `JsonSchema`。因此发布构建既不依赖 `ts-rs` 也不依赖 `schemars`。空操作 crate 必须声明同一组辅助属性（`ts`、`schemars`、`serde`），否则类型上的 `#[ts(...)]` 在非测试构建里会变成未知属性错误。

### 驱动脚本：`scripts/write_schema.py`

1. 把被忽略的测试 `export_tests::write_schema_fixtures` 的输出生成到临时目录。
2. 用 prettier 格式化 TypeScript。这必须先于任何比对：已提交的文件是格式化过的，原始输出不是。
3. 写入模式下把临时输出同步到 `schema/` 并删除孤儿文件（ts-rs 从不删除）。`--check` 只比对，有漂移就以非零退出。没有可用的 prettier 时，TypeScript 比对会带警告跳过；JSON 仍然比对。

`just write-app-server-schema` 写入，`just check-app-server-schema` 检查；`just check` 已包含检查。

### 新增一个方法

1. 在 `methods` 里加常量，在 `v2.rs` 里加 `*Params` / `*Response` 类型。
2. 把新类型名加进 `protocol_types!`。
3. 运行 `just write-app-server-schema` 并提交产物。
4. 在 app-server 的 `dispatch.rs` 里处理该方法。
5. 在 `integrations/channels/runtime-client/src/client.ts` 的 `Methods` 映射里加上条目，类型化客户端才能调用它。

## 1.3 配置：环境变量优先，一层覆盖

`jeeves-config` 的规则：

1. **环境变量是运行时配置项的来源。** `Settings::from_env()` 在启动时读取一次；`Settings` 是普通结构体，没有全局可变状态。
2. **Jeeves 不读 `.env` 文件。** `.env.example` 是供 `source` 的模板。
3. **解析从不 panic，空串视为未设置。** `env()` 先 trim，空串视为不存在。`env_bool` 只接受 `1`、`true`、`yes`。无法解析的整数和浮点数回退到默认值；最终值是否可接受由边界检查决定。
4. **模型与密钥不是环境变量配置项。** 当前模型、端点和密钥来自 `<home>/models.json` 与凭据存储（`models.rs`、`vault.rs`），由 app-server 在启动时应用。只保留 `JEEVES_PROVIDER` 作为兜底厂商，让本地模型零配置即可启动。

字段名、变量、默认值和取值范围只写在 [configuration.md](../configuration.zh.md)；代码与文档不一致时，`scripts/check_config_docs.py` 和 `check_env_reads.py` 会失败。

### 边界

`BOUNDS` 是数值上下限的唯一一张表。两个调用方共用它，所以一个值无论从哪里进来，判定方式都一样：

- 启动时的 `Settings::validate`：非法值拒绝启动，并指出变量名；
- `config/write` 的 `validate_patch_values`：非法补丁整体被拒。

枚举（`JEEVES_APPROVAL`、`JEEVES_SOUL_MODE`、`JEEVES_TOOL_POLICY`）以及 exec 允许规则的卫生检查（`MAX_EXEC_RULES`、`MAX_EXEC_RULE_CHARS`，不允许空规则或多行规则）在表旁边的代码里检查。`JEEVES_MAX_TOKENS` 是上限，不是每次请求的固定大小：模型目录的输出上限与剩余上下文会把每次请求拟合到窗口内，因此它可以大于被覆盖的 `JEEVES_CONTEXT_WINDOW`。`patch_name: None` 的配置项只能通过环境变量设置。

`ensure_home()` 在 `JEEVES_HOME` 下创建 `traces/`、`outbox/`、`skills/` 和 `spill/`。

### 已保存的补丁

设置页需要重启后依然有效的修改。`SettingsPatch`（因为要跨 JSON-RPC，所以是协议类型）只含 `Option` 字段，存为 `<home>/settings.json`。它只携带一个刻意收窄的子集：五个数值上限、`execAllow`、`writeAllowDirs` 和三个能力开关。模型与密钥被有意排除，所以密钥永远不会落进 `settings.json`。

- `load_patch`：文件缺失或无法解析都视为没有补丁，坏文件不会阻止启动。
- `save_patch`：空补丁会删除该文件。
- `apply_patch`：只有 `Some` 字段会覆盖。
- `SettingsPatch::merge_newer`：累积，同一字段以新补丁为准。`is_empty` 必须列全每个字段，否则只设置其中一个字段的补丁保存时会把文件删掉。

分层：`Settings::from_env()` → `apply_patch(已保存)` → `validate()` → 启动，此时再应用模型文件和凭据。因此对可打补丁的字段，已保存的补丁在被清除前优先于环境变量。

`config/write`（`server/settings.rs` 里的 `apply_config_patch`）的流程：校验 → 并入已保存的补丁 → 写 `settings.json` → 应用到内存 → 重新解析 provider → 替换 settings 与解析出的客户端。校验失败不改任何东西。先写文件再动内存，因为「改了却没记住」比「没改」更误导人。解析失败会在 stderr 报告，并由 `turn/*` 去说明缺少模型。

### 不变量

- 每个配置项都出现在 `.env.example` 和两份配置文档里。
- 错误配置是启动错误，绝不静默回退为默认值。
- 补丁先校验、再持久化、最后生效。
