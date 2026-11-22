# 05 Memory: `jeeves-memory`

English | [简体中文](05-memory.zh.md)

`jeeves-memory` owns the memory decisions; storage lives in `jeeves-state` ([02](02-state.md)) and
the turn assembly in `jeeves-app-server` ([06](06-app-server.md)). Every model-backed step here
fails open: a failed call leaves the data where it was and the turn goes on.

Source: `runtime/crates/jeeves-memory/src/`.

| Module | Role |
|---|---|
| `lib.rs` | `retrieve_context`: the per-turn memory lookup |
| `consolidation.rs` (+ `distilled.rs`) | Distill chat rows into facts, states and one episode |
| `store.rs` | `apply`: the one write entry that changes a fact and its page together |
| `tidy.rs` | Drop lines on a grown page that another line already says |
| `bundle.rs`, `mirror.rs`, `frontmatter.rs` | The OKF bundle in `<home>/memory/` and its reconciliation with the facts |
| `compaction.rs` | Rolling per-session summary of turns beyond the window |
| `skills.rs`, `install.rs` | SKILL.md loading, triggering, starter skills, export, install and update |

## 5.1 Storage contract (`jeeves-state`)

- `SemanticStore` (`add`, `search`, `recent`, `update`, `delete`, `forget_subject`) and
  `EpisodicStore` (`add`, `search`, `recent`, `delete`) are implemented by `Facts` and `Episodes`.
  `conformance::exercise_semantic` / `exercise_episodic` is the acceptance suite for any backend:
  misses return `false` or an empty list instead of erroring, double deletes return `false`,
  garbage queries return empty.
- `facts` rows carry `subject`, `content`, `source` (`user`, `consolidation`, `mcp`, ...), `kind`.
  `kind` is one of `fact`, `user`, `feedback`, `project`, `reference`; `normalise_kind` maps anything
  else to `fact` on write.
- Writes are deduplicated on `(subject, lower(trim(content)))`; `Facts::add` returns `(row, is_new)`.
- Search is FTS5 trigram ordered by `bm25`, falling back to a `LIKE` scan when FTS has no hit;
  a query containing CJK terms goes straight to a `LIKE` scan ranked by how many terms match.
  An empty or punctuation-only query returns an empty list.
- Reads go through the `active_facts` view (validity window applied). `durable_facts` additionally
  excludes any fact with a validity bound; only durable facts live on pages.
- A deleted or corrected claim is recorded in `memory_suppressions` so consolidation cannot bring it back.
- `facts.embedding` exists (migration 0005) but no code reads or writes it: retrieval is keyword-only.

## 5.2 Retrieval

`retrieve_context(facts, episodes, query, top_k)` runs every turn; there is no gate model.
`turn/context.rs` passes the first 300 characters of the user message and `retrieval_top_k`
(`JEEVES_RETRIEVAL_TOP_K`, default 4), under a timeout, and skips it on a resumed turn.

- Facts: `Facts::search(query, top_k)`, each rendered `- **subject**: content`, with a
  "temporary claim, valid until ..." suffix when the fact has a `valid_until`.
- Episodes: `Episodes::search(query, 3)`, rendered under a "Historical episode summaries" line as `- (date) summary`.
- An empty result is an empty string and the prompt adds no memory section. An error or timeout also yields an empty string.

## 5.3 Consolidation

`consolidate_if_due(chat, facts, episodes, client, small_model, every_n)`:

1. `chat.claim_consolidation(every_n * 2)` atomically claims either a retry batch whose backoff has
   expired or at least that many fresh `chat_log` rows (up to 4096; `browser-task` rows are never
   claimed). Fewer rows or an already-claimed batch returns 0.
2. Each row becomes `{sourceRef: "chat:<id>@<digest>", role, content}`. Up to 20 known facts (search hits
   for the batch's user messages, then the most recent) are listed with their ids.
3. One small-model call with `SUMMARIZER_PROMPT`, `max_tokens` 4096. The answer is JSON:
   `facts[{subject, content, kind, replaces[]}]`, `states[{subject, content, observation}]`, `episode`.
   `Distillation::validate` rejects more than 128 facts or 32 states, subjects over 256 characters,
   content over 4096, and an episode over 4096.
4. `persist` writes the result. A fact is skipped when its `lifetime` is not `durable`, when
   `temporary_marker` matches (case-insensitive substring from a fixed Chinese/English list such as
   "for now" or "this session"), or when suppressed. Facts go through `add_consolidated_from` (source
   `consolidation`, with source refs), states through `add_transient_from` (time-bounded, with the quoted
   observation), the episode through `add_consolidated` dated today. Each skip is a stderr line.
5. `replaces`: an old id is retired with `Facts::supersede` only when it was among the known facts shown,
   has source `consolidation`, the same subject as the new fact, and no time bound. Retirement ends its
   validity now, keeps the row, and suppresses its wording.
6. `complete_consolidation` clears the claim only after every write succeeded.

Failure (call error, no JSON, wrong shape, or a persist error) calls `mark_consolidation_failed`:
`consolidation_tries` increments and `consolidation_next_at` is pushed out by
`min(60 * 2^min(tries,6), 3600)` seconds; the rows return to unconsolidated and the same batch is
reclaimed once due. `recover_consolidation_claims` resets half-finished claims at exclusive runtime startup.

## 5.4 Write entry and tidying

`store::apply(facts, bundle, Change)` with `Remember`, `Update`, `Delete`, `ForgetSubject`:

1. Fold any hand edit of the subject's page into the facts (`sync_page`).
2. Make the change in `facts`.
3. Rewrite the subject's page from the facts, or remove it when no facts remain, only if the page is
   still ours (`mirror::owned`, decided before the change).

The `memory` tool, the memory RPCs in `dispatch.rs`, and tidying use it. Consolidation writes the table
directly and leaves the page to the worker's mirror.

`tidy::after_consolidation` runs after a distillation that wrote facts. For the subjects of the newest
facts (at most 2 per pass) with at least 4 facts and an owned page, it asks the small model
(`max_tokens` 1024) for `{"drop":[{"id","keptBy"}]}`. Only facts with source `consolidation` may go,
`keptBy` must be another fact on the same page, and each id appears in at most one pair. Each accepted
drop is `apply(Change::Delete)` plus `Facts::confirm(keptBy)`. An unusable answer changes nothing.

## 5.5 The OKF bundle

`<home>/memory/` holds one `<slug>.md` per subject, plus `index.md` and `changelog.md`.

- `Page` frontmatter: `type` (one of `fact`, `person`, `project`, `reference`, `concept`; anything else
  becomes `concept`), `title`, `subject`, `summary`, `sources`, `updated`, `digest` (sha256 of the trimmed
  body). Mirrored pages use `type: fact`, `sources: fact:<id>, ...`.
- `Bundle::write` is atomic (tmp file + rename). `page_path` validates the slug (1-64 characters of
  letters, digits, `-`, `_`; `index` and `changelog` reserved, case-insensitively), so a slug cannot escape the directory.
  `rewrite_index` regenerates `index.md` from the pages; `changelog_append` adds one `- <time> <line>` entry.
- Page bullets end with `<!-- fact:<id> kind=<k> source=<s> at=<t> -->`.
- The row's metadata (`FactMeta`: `page_path`, `digest`, `last_confirmed_at`, `confirm_count`) records the
  digest of the page as Jeeves last wrote it. A page whose digest differs was edited by hand.

`mirror.rs`:

- `mirror_facts`: per subject, writes the page from its facts unless the existing page's digest differs
  from the recorded one (then it is counted as skipped and left alone).
- `sync_edited_pages` / `sync_page`: for a hand-edited page Jeeves wrote, a bullet matched by trailer id (or
  by text) whose text changed updates that fact, an unmatched fact is deleted, an unmatched bullet becomes
  a `source=user` fact. A pure list is then rewritten from the facts; a page with prose of its own keeps its text.
- `reindex_from_pages`: bullets whose trailer id is missing from the table are restored under the same id
  (`Facts::restore`); suppressed claims and ids already in use are not restored.
- `recall_catalog`: the catalog injected into prompts, built at read time. It lists entries like `title · slug.md · kind — summary`
  for pages whose `fact:` sources are all still durable and whose body still matches those facts; stale pages are omitted.

`jeeves memory rebuild` runs reindex, sync, mirror.

## 5.6 Memory worker

`jeeves-app-server/src/memory_worker.rs`: one `MemoryWorker` per process, woken by `wake()` after a turn
and by a 60 s heartbeat. `run_once`:

1. If a model is resolved, `consolidate_if_due` with `consolidate_every` (`JEEVES_CONSOLIDATE_EVERY`, default 6);
   new facts are added to a pending counter and `take_pending` reports them on the next turn as
   `consolidationCompleted`.
2. If facts were written, `tidy::after_consolidation`.
3. `reindex_from_pages`, then `sync_edited_pages`.
4. `mirror_facts`, skipped when the digest of all facts and episodes equals the last mirrored one.

`stop()` waits up to 5 s for a run, then aborts it; its unfinished claim is recovered at the next exclusive
startup. Without a worker (tests, embedders) `finalize_turn` calls `consolidate_if_due` inline.

## 5.7 Compaction

The window is `min(history_turns, turns_that_fit(pairs, budget))`: `JEEVES_HISTORY_TURNS` (default 12) is a cap and
the token budget is the gate. `turns_that_fit` fills from the newest turn backwards, always keeping one turn.
The budget derives from the context window times `JEEVES_COMPACT_THRESHOLD` (default 0.8) minus the committed
spend ([06](06-app-server.md)).

- `newly_evicted(pairs, window, covered)`: turns `[covered, len - window)`. `covered` is
  `context_rollups.covered_turns`, so the summary only rolls forward.
- `roll_forward_with`: one call (`max_tokens` 700) over the previous summary plus the new turns, with an optional
  human focus line. On error or an empty reply it uses `fallback_summary`: previous summary plus each turn's
  user text clipped to 120 and assistant text to 160 characters, cut to 600 characters in total. It never returns Err.
- `refresh` stores the result with `save_rollup`, which refuses a summary covering fewer turns than the stored one.
  A failed or refused store is a stderr line.
- `summary_section` renders `Earlier in this conversation:` followed by the summary into the prompt tail.
- After each turn the app-server spawns `precompact` to prepare the next summary off the turn's clock.
- `chat_log` is never rewritten, and a summary is never re-summarized.
- Context overflow: when the provider returns `ContextOverflow`, the turn recompacts with a forced window of
  `FORCED_RETRY_TURNS` (2) and retries once.
- `session/compact` (`memory_center::compact_session`) is the human-triggered compaction: window 2, optional
  `focus`, the main model. It is not reachable by any tool.

## 5.8 Skills

A skill is `<dir>/<name>/SKILL.md`. `frontmatter::split` normalises CRLF and reads `key: value` lines between
two `---` fences (values trimmed, matching quotes removed). `parse_skill_text` requires `name` and `description`
and reads optional `schedule` (a cron line; the scheduler turns skills that have it into jobs), `version`,
`allow-model-invocation` / `allow_model_invocation` (default true; `false`, `no`, `0` disable), and
`dependencies` (comma- or space-separated skill names). Empty values count as absent; unknown keys are ignored.

- Directories: `<home>/skills` plus `JEEVES_SKILL_DIRS` (path-separated). `loaded_skills` (used by export
  and the CLI listing) inserts the home directory last so a local skill wins on a name clash, and skips `_incoming`
  and a `SKILL.md` directly in a root.
- Starter skills: `seed_default_skills` (called at runtime start) writes `summarize-document`, `tidy-folder` (from `jeeves-memory/default-skills/`) unless `<home>/skills/.seeded` exists,
  skipping any name already present, then writes the marker.
- Hot reload: `SkillLoader` compares the modification times of every `SKILL.md` before each use and rescans on change.
- Triggering (`match_message`): lowercase tokens are ASCII alphanumeric runs of length 3 or more, plus adjacent
  Han-character bigrams. A skill fires when its name + description tokens overlap the message tokens in at least 2
  entries; the top 2 by overlap are taken. Skills with `allow-model-invocation: false` or a missing dependency are excluded.
- `hits_capped(message, body_tokens)` is the single entry point per turn. It extracts `$skill-name` references
  (explicit loads, which bypass the invocation flag and dependency check), builds `### name\nbody` sections with each
  body cut at `JEEVES_SKILL_BODY_TOKENS` (default 1200; the tail points to `read_page(kind=skill)`), returns the message with
  references stripped (only the model's copy; the persisted history keeps the original), and a hint line for each
  reference naming no skill. Each chosen skill appends a line to `<home>/skill-hits.jsonl`, which feeds the skill statistics.
- `index_text(max_tokens)`: one line `- name — description` per skill (marked `explicit $name only` or `needs <dep>`
  where relevant), limited by `JEEVES_SKILL_INDEX_TOKENS` (default 200) with an "N more skills not listed" tail.
- `is_slug`: lowercase ASCII letters, digits and `-`, not starting or ending with `-`. It is the one naming rule
  for `create_skill`, `install_from_text`, `install` and the update path.
- `install_from_text`: parse, require a slug name, refuse an existing name, write `<home>/skills/<name>/SKILL.md`.
  `install_from_url` fetches then calls it; `raw_url` rewrites GitHub page URLs to raw URLs.
- `export_skills(home, targets, force)`: per skill, `unchanged` when the destination folder is byte-identical,
  `kept yours` when it differs and `force` is off, otherwise remove and copy (skipping `.DS_Store`, `__pycache__`).
- `install.rs`: `parse_index` reads an index of `{name, url, version}` entries. `needs_update` is true with no
  installed version, false when equal, `offered > installed` when both are dotted numbers, false otherwise.
  `install` requires a slug, a parseable skill whose `name` equals the index name, and for a replacement copies the old
  folder to `.backup/<name>-<timestamp>/`, stages the new file in `.staging/<name>/`, then renames it into place; a
  fresh install goes through `install_from_text`. `update_all` takes the fetch function as an argument and returns
  `Installed`, `UpToDate`, `Ahead` or `Failed` per entry.

## 5.9 How memory reaches the prompt

| Path | Content | Size control |
|---|---|---|
| Retrieval (5.2) | Matching facts and episodes, in the per-turn tail | `JEEVES_RETRIEVAL_TOP_K`, 300-character query |
| Memory catalog | `recall_catalog` lines, one per current page | Whole pages stay out |
| Skill index and hits | `index_text` plus triggered bodies | `JEEVES_SKILL_INDEX_TOKENS`, `JEEVES_SKILL_BODY_TOKENS` |
| Summary | `summary_section` | `max_tokens` 700 per fold |
| On demand | `read_page(kind=memory\|skill, name)` returns a page or skill body | The model asks |

The model-facing tools are `memory` (actions `save`, `forget`, `search`, `list`, `update`, `delete`,
`delete_episode`, `create_skill`; `search` and `list` are the read actions) and `read_page`.

`AGENTS.md` / `CLAUDE.md` project conventions are separate from memory: read from disk by `agents_md.rs`
(32 KB cap), never distilled or retrieved. The context meter (`memory_center::usage_of`) counts six sections:
soul, memory (retrieval text + catalog), skills (index + bodies), tools, history, summary.

## 5.10 Memory center

`memory_center.rs` is read-mostly: `memory/ledger` (turns and entries from the ledger, default 30, clamped 1..200),
`memory/index` and `memory/page` (bundle pages), `context/usage` (the last meter reading per session),
skill statistics, and `session/compact`. It never writes the ledger or the bundle. Wire shapes are in
[protocol](../protocol.md).
