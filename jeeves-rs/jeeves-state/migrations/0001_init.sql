-- Jeeves state.db schema (squashed baseline).
-- Created by folding the original 0001..0050 incremental migrations into one
-- final-schema snapshot. All statements are idempotent (IF NOT EXISTS).
-- Historical data migrations (dedup/backfill) are intentionally omitted: they
-- only ever applied to pre-existing rows during the original rollout.

-- table: agent_tasks
CREATE TABLE IF NOT EXISTS agent_tasks (
    id               TEXT PRIMARY KEY,
    -- NULL = 这个任务不挂在任何会话上（任务独立存在）。
    session_id       TEXT,
    title            TEXT NOT NULL,
    -- 目标条件：有它才有多轮循环；NULL = 单轮任务。
    goal_condition   TEXT,
    -- 验收条件：`[{"kind":"fileExists","path":"report.md"}, …]`，空数组 = 没有客观条件。
    acceptance_json  TEXT NOT NULL DEFAULT '[]',
    -- 最近一次验收的逐条结果：`[{"label":…,"passed":false,"detail":"report.md 不存在"}]`。
    -- 存在行上而不是只写事件流水：任务卡要一次读全，不必再查事件表。
    checks_json      TEXT NOT NULL DEFAULT '[]',
    -- 预算：`{"wallMs":…,"tokens":…,"tools":…}`，缺的键 = 这一维不设上限。
    budget_json      TEXT NOT NULL DEFAULT '{}',
    -- 已花费：每轮结束累加；人看得到「用了多少」，判定也读它。
    spent_wall_ms    INTEGER NOT NULL DEFAULT 0,
    spent_tokens     INTEGER NOT NULL DEFAULT 0,
    spent_tools      INTEGER NOT NULL DEFAULT 0,
    -- 任务自己的工作目录：验收检查与写入都以它为边界；NULL = 没选目录，路径检查一律拒绝。
    cwd              TEXT,
    status           TEXT NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending', 'running', 'suspended', 'needs_attention',
                                       'done', 'failed', 'cancelled')),
    rounds_used      INTEGER NOT NULL DEFAULT 0,
    max_rounds       INTEGER NOT NULL DEFAULT 5,
    tier             TEXT,
    current_turn_id  TEXT,
    suspend_reason   TEXT,
    -- 上一次停下的原因（验收未通过 / 超预算 / 判断器故障…），任务卡脚注直接读它。
    last_stop_reason TEXT,
    created_at       TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at       TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at      TEXT
, completion_notify_requested INTEGER NOT NULL DEFAULT 0, completion_notify_target TEXT);

-- table: approval_log
CREATE TABLE IF NOT EXISTS approval_log (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id      TEXT NOT NULL,
    request_id   TEXT NOT NULL,
    tool         TEXT NOT NULL,
    args_preview TEXT NOT NULL,
    reason       TEXT NOT NULL,
    outcome      TEXT NOT NULL,
    decided_at   TEXT NOT NULL DEFAULT (datetime('now'))
, actor TEXT NOT NULL DEFAULT '');

-- table: browser_sessions
CREATE TABLE IF NOT EXISTS browser_sessions (
 id TEXT PRIMARY KEY NOT NULL,
 task_id TEXT NOT NULL UNIQUE REFERENCES agent_tasks(id),
 owner_session_id TEXT NOT NULL,
 generation INTEGER NOT NULL DEFAULT 1,
 status TEXT NOT NULL,
 origins_json TEXT NOT NULL,
 url TEXT NOT NULL,
 title TEXT NOT NULL DEFAULT '',
 observed_at TEXT,
 error TEXT,
 created_at TEXT NOT NULL DEFAULT (datetime('now')),
 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: chat_log
CREATE TABLE IF NOT EXISTS chat_log (
    id           INTEGER PRIMARY KEY,
    role         TEXT NOT NULL,        -- 'user' | 'assistant'
    content      TEXT NOT NULL,
    consolidated INTEGER NOT NULL DEFAULT 0,
    session_id   TEXT NOT NULL DEFAULT 'default',
    source       TEXT NOT NULL DEFAULT 'cli',  -- cli / voice / telegram / dashboard
    meta         TEXT,                 -- 一轮的遥测 JSON（gate/graph/iterations/…）
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
, consolidation_tries INTEGER NOT NULL DEFAULT 0, consolidation_next_at TEXT, task_id TEXT, consolidation_batch TEXT, consolidation_claimed_at TEXT);

-- table: context_rollups
CREATE TABLE IF NOT EXISTS context_rollups (
    session_id    TEXT PRIMARY KEY,
    covered_turns INTEGER NOT NULL,
    summary       TEXT NOT NULL,
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: episodes
CREATE TABLE IF NOT EXISTS episodes (
    id          INTEGER PRIMARY KEY,
    happened_at TEXT NOT NULL,         -- ISO 8601 日期
    summary     TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: episodes_fts
CREATE VIRTUAL TABLE IF NOT EXISTS episodes_fts USING fts5(
    summary, content='episodes', content_rowid='id', tokenize='trigram'
);

-- table: facts
CREATE TABLE IF NOT EXISTS facts (
    id         INTEGER PRIMARY KEY,
    subject    TEXT NOT NULL,          -- 这条事实关于谁/什么，如 'alex'
    content    TEXT NOT NULL,          -- 事实本身
    source     TEXT NOT NULL DEFAULT 'user',  -- 'user' 直接告诉它 / 'consolidation' 提炼
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
, embedding TEXT, kind TEXT NOT NULL DEFAULT 'fact', page_path TEXT, digest TEXT, last_confirmed_at TEXT, confirm_count INTEGER NOT NULL DEFAULT 0);

-- table: facts_fts
CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
    subject, content, content='facts', content_rowid='id', tokenize='trigram'
);

-- table: interaction_receipts
CREATE TABLE IF NOT EXISTS interaction_receipts (
    session_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    input_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('claimed','completed','failed')),
    result_json TEXT,
    events_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(session_id, message_id)
);

-- table: job_completions
CREATE TABLE IF NOT EXISTS job_completions (
    job_id TEXT PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    notification_enqueued INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: jobs
CREATE TABLE IF NOT EXISTS jobs (
    id          TEXT PRIMARY KEY,
    owner       TEXT NOT NULL,
    command     TEXT NOT NULL,
    status      TEXT NOT NULL,
    exit_code   INTEGER,
    log_path    TEXT,
    started_at  TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT
, run_id TEXT, task_id TEXT, notify_requested INTEGER NOT NULL DEFAULT 0, notify_target TEXT);

-- table: ledger
CREATE TABLE IF NOT EXISTS ledger (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    turn_id    TEXT NOT NULL,
    kind       TEXT NOT NULL,                 -- user | assistant | tool_use | tool_result | image | notice
    payload    TEXT NOT NULL,                 -- 结构化 JSON；转存时是 {ptr,digest,bytes,preview}
    digest     TEXT NOT NULL,                 -- sha256(canonical payload)
    fidelity   TEXT NOT NULL DEFAULT 'exact', -- exact | folded
    at         TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: ledger_turns
CREATE TABLE IF NOT EXISTS ledger_turns (
    session_id            TEXT NOT NULL,
    turn_id               TEXT NOT NULL,
    turn_digest           TEXT NOT NULL,        -- 有序行摘要的摘要：轮级篡改证据
    events                INTEGER NOT NULL,
    fidelity              TEXT NOT NULL DEFAULT 'exact',
    preview               TEXT NOT NULL DEFAULT '',
    at                    TEXT NOT NULL DEFAULT (datetime('now')),
    consolidated          INTEGER NOT NULL DEFAULT 0,
    consolidation_tries   INTEGER NOT NULL DEFAULT 0,
    consolidation_next_at TEXT,
    PRIMARY KEY (session_id, turn_id)
);

-- table: memory_episode_contexts
CREATE TABLE IF NOT EXISTS memory_episode_contexts (
    context_digest TEXT PRIMARY KEY,
    source_refs_json TEXT NOT NULL,
    episode_id INTEGER REFERENCES episodes(id) ON DELETE SET NULL
);

-- table: memory_observations
CREATE TABLE IF NOT EXISTS memory_observations (
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    context_digest TEXT NOT NULL,
    source_refs_json TEXT NOT NULL,
    observed_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(fact_id, context_digest)
);

-- table: memory_suppressions
CREATE TABLE IF NOT EXISTS memory_suppressions (
    key TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK(scope IN ('fact','subject')),
    reason TEXT NOT NULL CHECK(reason IN ('deleted','corrected','forgotten_subject')),
    replacement_id INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: memory_transient_assessments
CREATE TABLE IF NOT EXISTS memory_transient_assessments (
    fact_id INTEGER NOT NULL,
    context_digest TEXT NOT NULL,
    assessment_json TEXT NOT NULL,
    PRIMARY KEY(fact_id,context_digest),
    FOREIGN KEY(fact_id,context_digest) REFERENCES memory_observations(fact_id,context_digest) ON DELETE CASCADE
);

-- table: memory_validity
CREATE TABLE IF NOT EXISTS memory_validity (
    fact_id INTEGER PRIMARY KEY REFERENCES facts(id) ON DELETE CASCADE,
    valid_from TEXT,
    valid_until TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')), origin TEXT NOT NULL DEFAULT 'human'
    CHECK(origin IN ('human','consolidation')),
    CHECK(valid_from IS NULL OR julianday(valid_from) IS NOT NULL),
    CHECK(valid_until IS NULL OR julianday(valid_until) IS NOT NULL),
    CHECK(valid_from IS NULL OR valid_until IS NULL OR julianday(valid_from)<julianday(valid_until))
);

-- table: run_round_accounts
CREATE TABLE IF NOT EXISTS run_round_accounts (
    run_id TEXT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    round_id TEXT NOT NULL,
    tokens_in INTEGER NOT NULL DEFAULT 0,
    tokens_out INTEGER NOT NULL DEFAULT 0,
    tools_ok INTEGER NOT NULL DEFAULT 0,
    tools_failed INTEGER NOT NULL DEFAULT 0,
    wall_ms INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, round_id)
);

-- table: schedule_completions
CREATE TABLE IF NOT EXISTS schedule_completions (
    name TEXT NOT NULL,
    minute TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('produced','empty','needs_attention','reviewed')),
    detail TEXT NOT NULL,
    output TEXT NOT NULL,
    preview TEXT NOT NULL,
    output_path TEXT,
    output_written_at TEXT,
    notify_requested INTEGER NOT NULL,
    notify_target TEXT,
    created_at TEXT NOT NULL DEFAULT(datetime('now')),
    enqueued_at TEXT,
    reviewed_at TEXT,
    PRIMARY KEY(name,minute)
);

-- table: schedule_jobs
CREATE TABLE IF NOT EXISTS schedule_jobs (
    name TEXT PRIMARY KEY,
    cron TEXT NOT NULL,
    prompt TEXT NOT NULL,
    delivery TEXT NOT NULL DEFAULT 'outbox',
    catch_up INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- table: schedule_state
CREATE TABLE IF NOT EXISTS schedule_state (
    name TEXT PRIMARY KEY,
    last_fired_minute TEXT,
    claimed_minute TEXT,
    -- 1 = 至少一次（错过的补跑一次）；0 = 至多一次（直接放弃，如实告知 stderr）。
    catch_up INTEGER NOT NULL DEFAULT 1,
    -- outbox（默认，写文件）| notify（文件 + 通知器）| quiet（只落盘日志）。
    delivery TEXT NOT NULL DEFAULT 'outbox',
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- table: session_context
CREATE TABLE IF NOT EXISTS session_context (
    session_id TEXT PRIMARY KEY,
    observed_input_tokens INTEGER NOT NULL,
    estimated_input_tokens INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);

-- table: session_permissions
CREATE TABLE IF NOT EXISTS session_permissions (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK(mode IN ('readOnly', 'standard', 'fullAccess')),
    revision INTEGER NOT NULL CHECK(revision > 0 AND revision <= 2147483647)
);

-- table: session_queue
CREATE TABLE IF NOT EXISTS session_queue (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    message    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
, status TEXT NOT NULL DEFAULT 'pending');

-- table: sessions
CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT PRIMARY KEY,
    -- 人起的名字；NULL 时读取侧回退「第一条用户消息」，与 0018 之前的行为一致。
    title      TEXT,
    -- 本会话的工作目录：它同时也是这个会话唯一的可写范围根。NULL = 还没选。
    cwd        TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_at    TEXT
, forked_from TEXT);

-- table: task_artifacts
CREATE TABLE IF NOT EXISTS task_artifacts (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    run_id  TEXT,
    path    TEXT NOT NULL,
    bytes   INTEGER NOT NULL DEFAULT 0,
    at      TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(task_id, path)
);

-- table: task_completions
CREATE TABLE IF NOT EXISTS task_completions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    run_id TEXT,
    session_id TEXT,
    title TEXT NOT NULL,
    is_goal INTEGER NOT NULL,
    status TEXT NOT NULL,
    reason TEXT NOT NULL,
    notify_requested INTEGER NOT NULL,
    notify_target TEXT,
    created_at TEXT NOT NULL DEFAULT(datetime('now')),
    enqueued_at TEXT
, verification_id TEXT REFERENCES task_verifications(id));

-- table: task_dispatch
CREATE TABLE IF NOT EXISTS task_dispatch (
    task_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE,
    turn_id TEXT NOT NULL UNIQUE,
    message TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','claimed','finished','interrupted')),
    created_at TEXT NOT NULL DEFAULT(datetime('now')),
    finished_at TEXT
, attachments_json TEXT NOT NULL DEFAULT '[]');

-- table: task_events
CREATE TABLE IF NOT EXISTS task_events (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id  TEXT NOT NULL,
    run_id   TEXT,
    seq      INTEGER NOT NULL DEFAULT 0,
    kind     TEXT NOT NULL,
    detail   TEXT,
    at       TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: task_input_conditions
CREATE TABLE IF NOT EXISTS task_input_conditions (
    condition_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL UNIQUE,
    source TEXT NOT NULL,
    event_key TEXT NOT NULL,
    description TEXT NOT NULL,
    expires_at TEXT,
    event_id TEXT,
    payload_json TEXT,
    received_at TEXT,
    consumed_run_id TEXT,
    created_at TEXT NOT NULL DEFAULT(datetime('now'))
);

-- table: task_input_events
CREATE TABLE IF NOT EXISTS task_input_events (
    source TEXT NOT NULL,
    event_id TEXT NOT NULL,
    condition_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    received_at TEXT NOT NULL DEFAULT(datetime('now')),
    PRIMARY KEY(source,event_id)
);

-- table: task_input_waits
CREATE TABLE IF NOT EXISTS task_input_waits (
    task_id TEXT PRIMARY KEY,
    condition_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'waiting' CHECK(state IN ('waiting','claimed','cancelled')),
    created_at TEXT NOT NULL DEFAULT(datetime('now'))
);

-- table: task_job_wait_members
CREATE TABLE IF NOT EXISTS task_job_wait_members (
    task_id TEXT NOT NULL REFERENCES task_job_waits(task_id) ON DELETE CASCADE,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, job_id)
);

-- table: task_job_waits
CREATE TABLE IF NOT EXISTS task_job_waits (
    task_id TEXT PRIMARY KEY REFERENCES agent_tasks(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    turn_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'waiting',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: task_pause_fences
CREATE TABLE IF NOT EXISTS task_pause_fences (
    task_id TEXT PRIMARY KEY REFERENCES agent_tasks(id) ON DELETE CASCADE,
    paused_at TEXT NOT NULL DEFAULT(datetime('now'))
);

-- table: task_plan_snapshots
CREATE TABLE IF NOT EXISTS task_plan_snapshots (
    task_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    steps_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (task_id, version)
);

-- table: task_runs
CREATE TABLE IF NOT EXISTS task_runs (
    id           TEXT PRIMARY KEY,
    task_id      TEXT NOT NULL,
    session_id   TEXT,
    turn_id      TEXT,
    status       TEXT NOT NULL DEFAULT 'running',
    rounds       INTEGER NOT NULL DEFAULT 0,
    tokens_in    INTEGER NOT NULL DEFAULT 0,
    tokens_out   INTEGER NOT NULL DEFAULT 0,
    tools_ok     INTEGER NOT NULL DEFAULT 0,
    tools_failed INTEGER NOT NULL DEFAULT 0,
    stop_reason  TEXT,
    started_at   TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at     TEXT
);

-- table: task_sources
CREATE TABLE IF NOT EXISTS task_sources (
    task_id     TEXT PRIMARY KEY,
    channel     TEXT NOT NULL,
    chat_id     TEXT NOT NULL,
    message_id  TEXT NOT NULL,
    sender_id   TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: task_steps
CREATE TABLE IF NOT EXISTS task_steps (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id         TEXT NOT NULL,
    run_id          TEXT,
    seq             INTEGER NOT NULL,
    kind            TEXT NOT NULL DEFAULT 'tool',
    tool            TEXT,
    input_json      TEXT,
    status          TEXT NOT NULL DEFAULT 'running',
    note            TEXT,
    idempotency_key TEXT,
    output_digest   TEXT,
    started_at      TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at        TEXT
);

-- table: task_verifications
CREATE TABLE IF NOT EXISTS task_verifications (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES agent_tasks(id),
    run_id TEXT NOT NULL REFERENCES task_runs(id),
    round_id TEXT NOT NULL,
    acceptance_json TEXT NOT NULL,
    checks_json TEXT NOT NULL,
    files_json TEXT NOT NULL,
    previous_id TEXT REFERENCES task_verifications(id),
    measured_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- table: tool_operations
CREATE TABLE IF NOT EXISTS tool_operations (
    id TEXT PRIMARY KEY,
    task_id TEXT REFERENCES agent_tasks(id),
    run_id TEXT REFERENCES task_runs(id),
    turn_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    round_id TEXT NOT NULL,
    tool_use_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    input_json TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    origin TEXT NOT NULL DEFAULT 'checkpoint' CHECK(origin IN ('checkpoint','recovered_checkpoint')),
    status TEXT NOT NULL DEFAULT 'intent' CHECK(status IN ('intent','succeeded','failed','reconciled_success','reconciled_failure','not_executed')),
    output_digest TEXT,
    evidence TEXT,
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    settled_at TEXT,
    UNIQUE(turn_id,round_id,tool_use_id)
);

-- table: turn_checkpoints
CREATE TABLE IF NOT EXISTS turn_checkpoints (
    turn_id      TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    user_message TEXT NOT NULL,
    messages_json TEXT NOT NULL,
    iterations   INTEGER NOT NULL DEFAULT 0,
    usage_json   TEXT NOT NULL DEFAULT '{}',
    status       TEXT NOT NULL DEFAULT 'open',
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
, run_id TEXT, step_seq INTEGER NOT NULL DEFAULT 0, plan_version INTEGER, round_id TEXT, round_no INTEGER CHECK(round_no > 0));

-- table: usage_ledger
CREATE TABLE IF NOT EXISTS usage_ledger (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL,
    provider      TEXT NOT NULL,
    model         TEXT NOT NULL,
    input_tokens  INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

-- view: active_facts
CREATE VIEW IF NOT EXISTS active_facts AS SELECT f.* FROM facts f LEFT JOIN memory_validity v ON v.fact_id=f.id
WHERE (v.valid_from IS NULL OR julianday(v.valid_from)<=julianday('now'))
AND (v.valid_until IS NULL OR julianday(v.valid_until)>julianday('now'));

-- view: durable_facts
CREATE VIEW IF NOT EXISTS durable_facts AS SELECT f.* FROM active_facts f LEFT JOIN memory_validity v ON v.fact_id=f.id
WHERE v.valid_from IS NULL AND v.valid_until IS NULL;

-- index: browser_owner
CREATE INDEX IF NOT EXISTS browser_owner ON browser_sessions(owner_session_id);

-- index: chat_log_consolidated_idx
CREATE INDEX IF NOT EXISTS chat_log_consolidated_idx ON chat_log (consolidated);

-- index: chat_log_consolidation_batches
CREATE INDEX IF NOT EXISTS chat_log_consolidation_batches ON chat_log(consolidated,consolidation_batch,consolidation_next_at);

-- index: chat_log_session_idx
CREATE INDEX IF NOT EXISTS chat_log_session_idx ON chat_log (session_id);

-- index: chat_log_task_idx
CREATE INDEX IF NOT EXISTS chat_log_task_idx ON chat_log (task_id);

-- index: facts_subject_content_unique
CREATE UNIQUE INDEX IF NOT EXISTS facts_subject_content_unique
    ON facts (subject, lower(trim(content)));

-- index: idx_agent_tasks_session_created
CREATE INDEX IF NOT EXISTS idx_agent_tasks_session_created ON agent_tasks(session_id, created_at DESC);

-- index: idx_agent_tasks_status
CREATE INDEX IF NOT EXISTS idx_agent_tasks_status ON agent_tasks(status, created_at ASC);

-- index: idx_approval_log_turn
CREATE INDEX IF NOT EXISTS idx_approval_log_turn ON approval_log(turn_id, decided_at DESC);

-- index: idx_jobs_owner_started
CREATE INDEX IF NOT EXISTS idx_jobs_owner_started ON jobs(owner, started_at DESC);

-- index: idx_task_artifacts_task
CREATE INDEX IF NOT EXISTS idx_task_artifacts_task ON task_artifacts(task_id, id);

-- index: idx_task_events_task
CREATE INDEX IF NOT EXISTS idx_task_events_task ON task_events(task_id, id);

-- index: idx_task_runs_status
CREATE INDEX IF NOT EXISTS idx_task_runs_status ON task_runs(status, started_at ASC);

-- index: idx_task_runs_task
CREATE INDEX IF NOT EXISTS idx_task_runs_task ON task_runs(task_id, started_at DESC);

-- index: idx_task_runs_turn
CREATE INDEX IF NOT EXISTS idx_task_runs_turn ON task_runs(turn_id);

-- index: idx_task_steps_key
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_steps_key ON task_steps(idempotency_key)
    WHERE idempotency_key IS NOT NULL;

-- index: idx_task_steps_run
CREATE INDEX IF NOT EXISTS idx_task_steps_run ON task_steps(run_id, seq);

-- index: idx_task_steps_task
CREATE INDEX IF NOT EXISTS idx_task_steps_task ON task_steps(task_id, id);

-- index: idx_turn_checkpoints_status
CREATE INDEX IF NOT EXISTS idx_turn_checkpoints_status ON turn_checkpoints(status, updated_at DESC);

-- index: idx_usage_model
CREATE INDEX IF NOT EXISTS idx_usage_model ON usage_ledger(model, created_at DESC);

-- index: idx_usage_session
CREATE INDEX IF NOT EXISTS idx_usage_session ON usage_ledger(session_id, created_at DESC);

-- index: jobs_run
CREATE INDEX IF NOT EXISTS jobs_run ON jobs(run_id);

-- index: jobs_task
CREATE INDEX IF NOT EXISTS jobs_task ON jobs(task_id);

-- index: ledger_session_idx
CREATE INDEX IF NOT EXISTS ledger_session_idx ON ledger (session_id, seq);

-- index: ledger_turn_idx
CREATE INDEX IF NOT EXISTS ledger_turn_idx ON ledger (session_id, turn_id, seq);

-- index: ledger_turns_pending
CREATE INDEX IF NOT EXISTS ledger_turns_pending ON ledger_turns (consolidated, at);

-- index: schedule_completions_pending
CREATE INDEX IF NOT EXISTS schedule_completions_pending ON schedule_completions(notify_requested,enqueued_at);

-- index: session_queue_fifo
CREATE INDEX IF NOT EXISTS session_queue_fifo ON session_queue (session_id, id);

-- index: session_queue_one_claim
CREATE UNIQUE INDEX IF NOT EXISTS session_queue_one_claim ON session_queue(session_id) WHERE status = 'claimed';

-- index: sessions_recent
CREATE INDEX IF NOT EXISTS sessions_recent ON sessions(last_at DESC);

-- index: task_completions_pending
CREATE INDEX IF NOT EXISTS task_completions_pending ON task_completions(notify_requested,enqueued_at);

-- index: task_dispatch_pending
CREATE INDEX IF NOT EXISTS task_dispatch_pending ON task_dispatch(state, created_at);

-- index: task_input_waits_pending
CREATE INDEX IF NOT EXISTS task_input_waits_pending ON task_input_waits(state,created_at);

-- index: task_verifications_latest
CREATE INDEX IF NOT EXISTS task_verifications_latest ON task_verifications(task_id,run_id,measured_at);

-- index: tool_operations_task
CREATE INDEX IF NOT EXISTS tool_operations_task ON tool_operations(task_id,started_at);

-- trigger: episodes_ad
CREATE TRIGGER IF NOT EXISTS episodes_ad AFTER DELETE ON episodes BEGIN
    INSERT INTO episodes_fts(episodes_fts, rowid, summary) VALUES ('delete', old.id, old.summary);
END;

-- trigger: episodes_ai
CREATE TRIGGER IF NOT EXISTS episodes_ai AFTER INSERT ON episodes BEGIN
    INSERT INTO episodes_fts(rowid, summary) VALUES (new.id, new.summary);
END;

-- trigger: facts_ad
CREATE TRIGGER IF NOT EXISTS facts_ad AFTER DELETE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, content) VALUES ('delete', old.id, old.subject, old.content);
END;

-- trigger: facts_ai
CREATE TRIGGER IF NOT EXISTS facts_ai AFTER INSERT ON facts BEGIN
    INSERT INTO facts_fts(rowid, subject, content) VALUES (new.id, new.subject, new.content);
END;

-- trigger: facts_au
CREATE TRIGGER IF NOT EXISTS facts_au AFTER UPDATE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, content) VALUES ('delete', old.id, old.subject, old.content);
    INSERT INTO facts_fts(rowid, subject, content) VALUES (new.id, new.subject, new.content);
END;

-- trigger: task_completion_committed
CREATE TRIGGER IF NOT EXISTS task_completion_committed AFTER UPDATE OF status ON agent_tasks
WHEN NEW.status<>OLD.status AND NEW.status IN ('done','failed','cancelled','suspended','needs_attention')
BEGIN
    INSERT INTO task_completions(task_id,run_id,session_id,title,is_goal,status,reason,notify_requested,notify_target)
    VALUES(NEW.id,(SELECT CASE WHEN COUNT(*)=1 THEN MIN(id) END FROM task_runs
        WHERE task_id=NEW.id AND (turn_id=COALESCE(OLD.current_turn_id,NEW.current_turn_id)
        OR (turn_id IS NULL AND id=COALESCE(OLD.current_turn_id,NEW.current_turn_id)))),NEW.session_id,NEW.title,
        NEW.goal_condition IS NOT NULL,
        CASE WHEN NEW.status='done' AND NEW.acceptance_json='[]' AND NEW.goal_condition IS NULL THEN 'produced_unverified' ELSE NEW.status END,
        CASE
            WHEN NEW.status='done' THEN COALESCE(NEW.last_stop_reason,'Task output produced; inspect its acceptance evidence')
            WHEN NEW.status='failed' THEN COALESCE(NEW.last_stop_reason,'Task failed; inspect the execution record')
            WHEN NEW.status='cancelled' THEN 'Task cancelled by the user'
            ELSE COALESCE(NEW.suspend_reason,NEW.last_stop_reason,'Task stopped for review')
        END,
        NEW.completion_notify_requested,NEW.completion_notify_target);
END;

-- trigger: task_completion_unverified_goal
CREATE TRIGGER IF NOT EXISTS task_completion_unverified_goal AFTER INSERT ON task_completions
WHEN NEW.status='done' AND EXISTS(SELECT 1 FROM agent_tasks WHERE id=NEW.task_id AND acceptance_json='[]')
BEGIN
    UPDATE task_completions SET status='produced_unverified',verification_id=NULL WHERE id=NEW.id;
END;

-- trigger: task_completion_verification
CREATE TRIGGER IF NOT EXISTS task_completion_verification AFTER INSERT ON task_completions
WHEN NEW.status='done'
BEGIN
    UPDATE task_completions SET verification_id=(SELECT id FROM task_verifications
        WHERE task_id=NEW.task_id AND run_id=NEW.run_id
        AND acceptance_json=(SELECT acceptance_json FROM agent_tasks WHERE id=NEW.task_id)
        AND checks_json=(SELECT checks_json FROM agent_tasks WHERE id=NEW.task_id) ORDER BY rowid DESC LIMIT 1)
        WHERE id=NEW.id;
END;
