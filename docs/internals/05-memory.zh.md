# 05 记忆：`jeeves-memory`

[English](05-memory.md) | 简体中文

`jeeves-memory` 负责记忆相关的决策；存储在 `jeeves-state`（[02](02-state.zh.md)），
回合装配在 `jeeves-app-server`（[06](06-app-server.zh.md)）。这里每一个依赖模型的步骤都是失败开放：
调用失败时数据保持原样，回合继续。

源码：`runtime/crates/jeeves-memory/src/`。

| 模块 | 职责 |
|---|---|
| `lib.rs` | `retrieve_context`：每回合的记忆查找 |
| `consolidation.rs`（+ `distilled.rs`） | 把 chat 行提炼成事实、状态和一条情景 |
| `store.rs` | `apply`：同时改动事实与其页面的唯一写入口 |
| `tidy.rs` | 删去变长页面上已被其他行表达过的行 |
| `bundle.rs`、`mirror.rs`、`frontmatter.rs` | `<home>/memory/` 中的 OKF bundle 及其与事实的对账 |
| `compaction.rs` | 对窗口之外的回合做按会话滚动的摘要 |
| `skills.rs`、`install.rs` | SKILL.md 的加载、触发、起步技能、导出、安装与更新 |

## 5.1 存储契约（`jeeves-state`）

- `SemanticStore`（`add`、`search`、`recent`、`update`、`delete`、`forget_subject`）与
  `EpisodicStore`（`add`、`search`、`recent`、`delete`）由 `Facts` 和 `Episodes` 实现。
  `conformance::exercise_semantic` / `exercise_episodic` 是任何后端的验收套件：
  未命中返回 `false` 或空列表而不是报错，重复删除返回 `false`，乱码查询返回空。
- `facts` 行带 `subject`、`content`、`source`（`user`、`consolidation`、`mcp` 等）、`kind`。
  `kind` 取 `fact`、`user`、`feedback`、`project`、`reference` 之一；`normalise_kind` 在写入时把其他值映射为 `fact`。
- 写入按 `(subject, lower(trim(content)))` 去重；`Facts::add` 返回 `(row, is_new)`。
- 搜索是按 `bm25` 排序的 FTS5 trigram，FTS 无命中时退到 `LIKE` 扫描；
  含 CJK 词的查询直接走 `LIKE` 扫描，按命中词数排序。空查询或纯标点查询返回空列表。
- 读取经过 `active_facts` 视图（应用有效期窗口）。`durable_facts` 进一步排除任何带有效期边界的事实；
  只有持久事实会落在页面上。
- 被删除或被纠正的论断记入 `memory_suppressions`，提炼不会把它带回来。
- `facts.embedding` 列存在（迁移 0005），但没有任何代码读写它：检索只有关键词。

## 5.2 检索

`retrieve_context(facts, episodes, query, top_k)` 每回合都运行；没有门控模型。
`turn/context.rs` 传入用户消息的前 300 个字符和 `retrieval_top_k`
（`JEEVES_RETRIEVAL_TOP_K`，默认 4），带超时，续跑的回合跳过检索。

- 常驻偏好：`Facts::personal_preferences()` 每回合排在最前，与消息内容无关：最多六条用户亲口说的
  （`source = user`）持久事实，类型为 `user` 或 `feedback`，或主题是 `user`。关键词检索没法把
  “该给我点什么？”和“喜欢绿茶”联系起来。提炼出的推测和有时限的说法仍需匹配才出现。
  查询失败只打一行 stderr，检索结果照常返回。
- 事实：`Facts::search(query, top_k)`，跳过已列出的偏好，渲染为 `- **subject**: content`；事实有 `valid_until` 时加上
  “temporary claim, valid until ...” 后缀。
- 情景：`Episodes::search(query, 3)`，渲染在 “Historical episode summaries” 行之下，形如 `- (date) summary`。
- 结果为空就是空字符串，提示里不加记忆段。出错或超时同样得到空字符串。

## 5.3 提炼

`consolidate_if_due(chat, facts, episodes, client, small_model, every_n)`：

1. `chat.claim_consolidation(every_n * 2)` 原子地认领一个退避已到期的重试批次，或至少这么多条新的 `chat_log` 行
   （最多 4096；`browser-task` 行永不认领）。行数不足或批次已被认领则返回 0。
2. 每行变成 `{sourceRef: "chat:<id>@<digest>", role, content}`。最多 20 条已知事实（本批用户消息的搜索命中，
   再补最近的）连同 id 一并列出。
3. 用 `SUMMARIZER_PROMPT` 调一次小模型，`max_tokens` 为 4096。回答是 JSON：
   `facts[{subject, content, kind, replaces[]}]`、`states[{subject, content, observation}]`、`episode`。
   `Distillation::validate` 拒绝超过 128 条事实或 32 条状态、主语超过 256 字符、内容超过 4096、情景超过 4096 的结果。
4. `persist` 写入结果。事实在 `lifetime` 不是 `durable`、`temporary_marker` 命中（固定的中英文列表做不区分大小写的子串匹配，
   如 “for now”、“this session”）、`task_receipt` 命中（主题或内容带任务编号，即 `task` 后跟十位以上数字，
   或含 `/workspaces/` 下的路径；这些由任务账本保存）或被抑制时被跳过。提示词也要求提炼时跳过任务进度、
   完成回执、验证运行和工作文件内容。事实走 `add_consolidated_from`（来源 `consolidation`，带来源引用），
   状态走 `add_transient_from`（带时限，附引用的观察），情景走 `add_consolidated`，日期为当天。每次跳过都是一行 stderr。
5. `replaces`：仅当旧 id 在展示给模型的已知事实之中、来源为 `consolidation`、与新事实同主语且无时限时，
   才用 `Facts::supersede` 退役。退役会让其有效期在此刻结束、保留该行，并抑制其措辞。
6. 只有所有写入都成功后才 `complete_consolidation` 清除认领。

失败（调用出错、无 JSON、结构不对或 persist 出错）调用 `mark_consolidation_failed`：
`consolidation_tries` 加一，`consolidation_next_at` 推后 `min(60 * 2^min(tries,6), 3600)` 秒；
这些行回到未提炼状态，到期后同一批次被重新认领。`recover_consolidation_claims` 在独占运行时启动时重置未完成的认领。

## 5.4 写入口与整理

`store::apply(facts, bundle, Change)`，`Change` 为 `Remember`、`Update`、`Delete`、`ForgetSubject`：

1. 先把该主语页面上的手工编辑并入事实（`sync_page`）。
2. 在 `facts` 中做出改动。
3. 仅当页面仍是 Jeeves 自己的（`mirror::owned`，在改动前判定）时，才根据事实重写该主语的页面，没有事实时删除页面。

`memory` 工具、`dispatch.rs` 中的记忆 RPC 以及整理都经过它。提炼直接写表，页面留给 worker 的镜像。

`tidy::after_consolidation` 在写入了事实的提炼之后运行。对最新事实所属的主语（每次最多 2 个），
若至少有 4 条事实且页面归 Jeeves 所有，就向小模型（`max_tokens` 1024）要 `{"drop":[{"id","keptBy"}]}`。
只有来源为 `consolidation` 的事实可被删，`keptBy` 必须是同页的另一条事实，每个 id 至多出现在一对中。
每个被接受的删除是 `apply(Change::Delete)` 加 `Facts::confirm(keptBy)`。不可用的回答不改动任何东西。

## 5.5 OKF bundle

`<home>/memory/` 下每个主语一个 `<slug>.md`，另有 `index.md` 和 `changelog.md`。

- `Page` 前置信息：`type`（`fact`、`person`、`project`、`reference`、`concept` 之一，其他值变为 `concept`）、
  `title`、`subject`、`summary`、`sources`、`updated`、`digest`（去空白正文的 sha256）。
  镜像生成的页面用 `type: fact`、`sources: fact:<id>, ...`。
- `Bundle::write` 是原子的（临时文件 + rename）。`page_path` 校验 slug（1 到 64 个字符，仅限字母、数字、`-`、`_`；
  `index` 与 `changelog` 保留，不区分大小写），因此 slug 无法逃出目录。
  `rewrite_index` 根据页面重新生成 `index.md`；`changelog_append` 追加一条 `- <time> <line>`。
- 页面要点行以 `<!-- fact:<id> kind=<k> source=<s> at=<t> -->` 结尾。
- 事实的元数据（`FactMeta`：`page_path`、`digest`、`last_confirmed_at`、`confirm_count`）记录 Jeeves 上次写入该页面时的摘要。
  摘要不同的页面说明被手工编辑过。

`mirror.rs`：

- `mirror_facts`：按主语用其事实写页面，除非现有页面的摘要与记录的不同（此时计为跳过并保持原样）。
- `sync_edited_pages` / `sync_page`：对 Jeeves 写出且被手工编辑的页面，按尾注 id（或按文本）匹配的要点若文本变了则更新该事实，
  未匹配的事实被删除，未匹配的要点成为 `source=user` 的事实。纯列表页随后按事实重写；含自有正文的页面保留其文本。
- `reindex_from_pages`：尾注 id 不在表中的要点按原 id 恢复（`Facts::restore`）；被抑制的论断和已被占用的 id 不恢复。
- `recall_catalog`：注入提示的目录，读取时构建。它为 `fact:` 来源全部仍为持久事实、且正文仍与这些事实一致的页面列出
  `title · slug.md · kind — summary` 形式的条目；过期页面被省略。

`jeeves memory rebuild` 依次执行 reindex、sync、mirror。

## 5.6 记忆 worker

`jeeves-app-server/src/memory_worker.rs`：每进程一个 `MemoryWorker`，回合结束后由 `wake()` 唤醒，另有 60 秒心跳。
`run_once`：

1. 若已解析出模型，用 `consolidate_every`（`JEEVES_CONSOLIDATE_EVERY`，默认 6）调用 `consolidate_if_due`；
   新事实计入待报计数，下一回合由 `take_pending` 以 `consolidationCompleted` 通知报告。
2. 若写入了事实，运行 `tidy::after_consolidation`。
3. `reindex_from_pages`，然后 `sync_edited_pages`。
4. `mirror_facts`；若全部事实与情景的摘要与上次镜像时相同则跳过。

`stop()` 最多等一次运行 5 秒，然后中止；其未完成的认领在下次独占启动时恢复。
没有 worker（测试、嵌入方）时，`finalize_turn` 内联调用 `consolidate_if_due`。

## 5.7 压缩

窗口为 `min(history_turns, turns_that_fit(pairs, budget))`：`JEEVES_HISTORY_TURNS`（默认 12）是上限，token 预算是闸门。
`turns_that_fit` 从最新回合向前填充，始终至少保留一个回合。预算由上下文窗口乘以 `JEEVES_COMPACT_THRESHOLD`（默认 0.8）
再减去已承诺的开销得出（[06](06-app-server.zh.md)）。

- `newly_evicted(pairs, window, covered)`：取回合 `[covered, len - window)`。`covered` 即
  `context_rollups.covered_turns`，因此摘要只向前滚动。
- `roll_forward_with`：对旧摘要加新回合调用一次（`max_tokens` 700），可附带人工给出的重点一行。摘要使用固定段落（目标、以过去式事实写成的已完成、决定与偏好、未完成、事实），空段省略，已完成的事项从未完成移到已完成。
  出错或回复为空时用 `fallback_summary`：旧摘要加每轮用户文本截到 120、助手文本截到 160 个字符，总长截到 600 字符。它从不返回 Err。
- `refresh` 用 `save_rollup` 保存结果，该函数拒绝覆盖更多回合的已存摘要。保存失败或被拒是一行 stderr。
- `summary_section` 在提示尾部渲染 `Earlier in this conversation:` 加摘要。
- 每回合结束后 app-server 派生 `precompact`，在回合计时之外准备下一份摘要。
- `chat_log` 从不被改写，摘要也从不被再次摘要。
- 上下文溢出：provider 返回 `ContextOverflow` 时，回合以 `FORCED_RETRY_TURNS`（2）的强制窗口重新压缩并重试一次。
- `session/compact`（`memory_center::compact_session`）是人工触发的压缩：窗口 2，可选 `focus`，使用主模型。任何工具都无法触达它。

## 5.8 技能

技能是 `<dir>/<name>/SKILL.md`。`frontmatter::split` 先规范化 CRLF，再读取两道 `---` 之间的 `key: value` 行
（值去空白、去掉成对引号）。`parse_skill_text` 要求 `name` 和 `description`，并读取可选的 `schedule`
（cron 行；调度器把带它的技能变成作业）、`version`、`allow-model-invocation` / `allow_model_invocation`
（默认 true；`false`、`no`、`0` 关闭）和 `dependencies`（逗号或空格分隔的技能名）。空值视为缺省；未知键忽略。

- 目录：`<home>/skills` 加 `JEEVES_SKILL_DIRS`（按路径分隔符分隔）。`loaded_skills`（供导出与 CLI 列表使用）
  最后插入 home 目录，使本地技能在重名时胜出，并跳过 `_incoming` 以及直接放在根目录下的 `SKILL.md`。
- 起步技能：`seed_default_skills`（运行时启动时调用）写入 `summarize-document`、
  `tidy-folder`（来自 `jeeves-memory/default-skills/`），除非 `<home>/skills/.seeded` 已存在；已存在的同名技能跳过，随后写下该标记。
- 热加载：`SkillLoader` 每次使用前比较所有 `SKILL.md` 的修改时间，变化则重新扫描。
- 触发（`match_message`）：小写词元是长度 3 及以上的 ASCII 字母数字连续段，外加相邻汉字二元组。技能的名称 + 描述词元与
  消息词元至少重合 2 个时触发；按重合数取前 2 个。`allow-model-invocation: false` 或依赖缺失的技能被排除。
- `hits_capped(message, body_tokens)` 是每回合的唯一入口。它提取 `$skill-name` 引用（显式加载，绕过调用开关和依赖检查），
  构造 `### name\nbody` 段落，每个正文按 `JEEVES_SKILL_BODY_TOKENS`（默认 1200）截断（尾部提示用 `read_page(kind=skill)` 读其余），
  返回去掉引用的消息（只影响给模型的副本；持久化的历史保留原文），并为每个不指向任何技能的引用返回一行提示。
  每个被选中的技能向 `<home>/skill-hits.jsonl` 追加一行，供技能统计使用。
- `index_text(max_tokens)`：每个技能一行 `- name — description`（视情况标注 `explicit $name only` 或 `needs <dep>`），
  受 `JEEVES_SKILL_INDEX_TOKENS`（默认 200）限制，超出的以 “N more skills not listed” 收尾。
- `is_slug`：小写 ASCII 字母、数字和 `-`，且不以 `-` 开头或结尾。它是 `create_skill`、`install_from_text`、`install`
  和更新路径共用的唯一命名规则。
- `install_from_text`：解析、要求 slug 名称、拒绝已存在的名称、写入 `<home>/skills/<name>/SKILL.md`。
  `install_from_url` 先抓取再调用它；`raw_url` 把 GitHub 页面 URL 改写成 raw URL。
- `export_skills(home, targets, force)`：对每个技能，目标文件夹逐字节相同则为 `unchanged`，内容不同且未加 `force` 则为 `kept yours`，
  否则删除后复制（跳过 `.DS_Store`、`__pycache__`）。
- `install.rs`：`parse_index` 读取 `{name, url, version}` 条目的索引。`needs_update` 在没有已装版本时为 true，相等时为 false，
  两者都是点分数字时取 `offered > installed`，否则为 false。`install` 要求 slug、可解析的技能且其 `name` 与索引名一致；
  替换时先把旧文件夹复制到 `.backup/<name>-<timestamp>/`，把新文件暂存到 `.staging/<name>/`，再 rename 到位；
  全新安装走 `install_from_text`。`update_all` 把抓取函数作为参数，对每个条目返回 `Installed`、`UpToDate`、`Ahead` 或 `Failed`。

## 5.9 记忆如何进入提示

| 路径 | 内容 | 规模控制 |
|---|---|---|
| 检索（5.2） | 匹配的事实与情景，位于每回合的尾部 | `JEEVES_RETRIEVAL_TOP_K`，300 字符的查询 |
| 记忆目录 | `recall_catalog` 的行，每个当前页面一行 | 整页不进入 |
| 技能索引与命中 | `index_text` 加被触发的正文 | `JEEVES_SKILL_INDEX_TOKENS`、`JEEVES_SKILL_BODY_TOKENS` |
| 摘要 | `summary_section` | 每次折叠 `max_tokens` 700 |
| 按需 | `read_page(kind=memory\|skill, name)` 返回页面或技能正文 | 由模型发起 |

面向模型的工具是 `memory`（动作 `save`、`forget`、`search`、`list`、`update`、`delete`、`delete_episode`、`create_skill`；
`search` 与 `list` 是只读动作）和 `read_page`。

`AGENTS.md` / `CLAUDE.md` 项目约定与记忆是两回事：由 `agents_md.rs` 从磁盘读取（上限 32 KB），从不被提炼或检索。
上下文仪表（`memory_center::usage_of`）统计六个部分：soul、memory（检索文本 + 目录）、skills（索引 + 正文）、tools、history、summary。

## 5.10 记忆中心

`memory_center.rs` 以只读为主：`memory/ledger`（来自账本的回合与条目，默认 30，限制在 1 到 200）、
`memory/index` 与 `memory/page`（bundle 页面）、`context/usage`（每个会话最近一次的仪表读数）、
技能统计，以及 `session/compact`。它从不写账本或 bundle。线上形态见 [protocol](../protocol.zh.md)。
