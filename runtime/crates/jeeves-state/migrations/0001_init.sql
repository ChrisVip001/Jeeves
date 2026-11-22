-- Jeeves state.db schema — single baseline.
-- This is the final schema produced by the former 0001..0025 incremental migrations,
-- captured from a freshly migrated database. Historical data migrations (dedup/backfill/
-- column drops) are intentionally absent: they only mattered to databases created before
-- the squash. See AGENTS.md for the compatibility policy on pre-existing state.db files.

CREATE TABLE agent_tasks (
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
, completion_notify_requested INTEGER NOT NULL DEFAULT 0, completion_notify_target TEXT, parent_task_id TEXT REFERENCES agent_tasks(id), integrated_commit TEXT);
CREATE TABLE approval_log (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    turn_id      TEXT NOT NULL,
    request_id   TEXT NOT NULL,
    tool         TEXT NOT NULL,
    args_preview TEXT NOT NULL,
    reason       TEXT NOT NULL,
    outcome      TEXT NOT NULL,
    decided_at   TEXT NOT NULL DEFAULT (datetime('now'))
, actor TEXT NOT NULL DEFAULT '');

CREATE TABLE browser_sessions (
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
CREATE TABLE chat_log (
    id           INTEGER PRIMARY KEY,
    role         TEXT NOT NULL,        -- 'user' | 'assistant'
    content      TEXT NOT NULL,
    consolidated INTEGER NOT NULL DEFAULT 0,
    session_id   TEXT NOT NULL DEFAULT 'default',
    source       TEXT NOT NULL DEFAULT 'cli',  -- cli / voice / telegram / dashboard
    meta         TEXT,                 -- 一轮的遥测 JSON（gate/graph/iterations/…）
    created_at   TEXT NOT NULL DEFAULT (datetime('now'))
, consolidation_tries INTEGER NOT NULL DEFAULT 0, consolidation_next_at TEXT, task_id TEXT, consolidation_batch TEXT, consolidation_claimed_at TEXT, notice_key TEXT);
CREATE TABLE context_rollups (
    session_id    TEXT PRIMARY KEY,
    covered_turns INTEGER NOT NULL,
    summary       TEXT NOT NULL,
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE episodes (
    id          INTEGER PRIMARY KEY,
    happened_at TEXT NOT NULL,         -- ISO 8601 日期
    summary     TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE VIRTUAL TABLE episodes_fts USING fts5(
    summary, content='episodes', content_rowid='id', tokenize='trigram'
)
/* episodes_fts(summary) */;
CREATE TABLE facts (
    id         INTEGER PRIMARY KEY,
    subject    TEXT NOT NULL,          -- 这条事实关于谁/什么，如 'alex'
    content    TEXT NOT NULL,          -- 事实本身
    source     TEXT NOT NULL DEFAULT 'user',  -- 'user' 直接告诉它 / 'consolidation' 提炼
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
, embedding TEXT, kind TEXT NOT NULL DEFAULT 'fact', page_path TEXT, digest TEXT, last_confirmed_at TEXT, confirm_count INTEGER NOT NULL DEFAULT 0);
CREATE VIRTUAL TABLE facts_fts USING fts5(
    subject, content, content='facts', content_rowid='id', tokenize='trigram'
)
/* facts_fts(subject,content) */;
CREATE TABLE interaction_receipts (
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
CREATE TABLE job_completions (
    job_id TEXT PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    notification_enqueued INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE jobs (
    id          TEXT PRIMARY KEY,
    owner       TEXT NOT NULL,
    command     TEXT NOT NULL,
    status      TEXT NOT NULL,
    exit_code   INTEGER,
    log_path    TEXT,
    started_at  TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at TEXT
, run_id TEXT, task_id TEXT, notify_requested INTEGER NOT NULL DEFAULT 0, notify_target TEXT);
CREATE TABLE ledger (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    turn_id    TEXT NOT NULL,
    kind       TEXT NOT NULL,                 -- user | assistant | tool_use | tool_result | image | notice
    payload    TEXT NOT NULL,                 -- 结构化 JSON；转存时是 {ptr,digest,bytes,preview}
    digest     TEXT NOT NULL,                 -- sha256(canonical payload)
    fidelity   TEXT NOT NULL DEFAULT 'exact', -- exact | folded
    at         TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE ledger_turns (
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
CREATE TABLE memory_episode_contexts (
    context_digest TEXT PRIMARY KEY,
    source_refs_json TEXT NOT NULL,
    episode_id INTEGER REFERENCES episodes(id) ON DELETE SET NULL
);
CREATE TABLE memory_observations (
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    context_digest TEXT NOT NULL,
    source_refs_json TEXT NOT NULL,
    observed_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(fact_id, context_digest)
);
CREATE TABLE memory_suppressions (
    key TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK(scope IN ('fact','subject')),
    reason TEXT NOT NULL CHECK(reason IN ('deleted','corrected','forgotten_subject')),
    replacement_id INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE memory_transient_assessments (
    fact_id INTEGER NOT NULL,
    context_digest TEXT NOT NULL,
    assessment_json TEXT NOT NULL,
    PRIMARY KEY(fact_id,context_digest),
    FOREIGN KEY(fact_id,context_digest) REFERENCES memory_observations(fact_id,context_digest) ON DELETE CASCADE
);
CREATE TABLE memory_validity (
    fact_id INTEGER PRIMARY KEY REFERENCES facts(id) ON DELETE CASCADE,
    valid_from TEXT,
    valid_until TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')), origin TEXT NOT NULL DEFAULT 'human'
    CHECK(origin IN ('human','consolidation')),
    CHECK(valid_from IS NULL OR julianday(valid_from) IS NOT NULL),
    CHECK(valid_until IS NULL OR julianday(valid_until) IS NOT NULL),
    CHECK(valid_from IS NULL OR valid_until IS NULL OR julianday(valid_from)<julianday(valid_until))
);
CREATE TABLE run_round_accounts (
    run_id TEXT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    round_id TEXT NOT NULL,
    tokens_in INTEGER NOT NULL DEFAULT 0,
    tokens_out INTEGER NOT NULL DEFAULT 0,
    tools_ok INTEGER NOT NULL DEFAULT 0,
    tools_failed INTEGER NOT NULL DEFAULT 0,
    wall_ms INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, round_id)
);
CREATE TABLE schedule_completions (
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
CREATE TABLE schedule_jobs (
    name TEXT PRIMARY KEY,
    cron TEXT NOT NULL,
    prompt TEXT NOT NULL,
    delivery TEXT NOT NULL DEFAULT 'outbox',
    catch_up INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE TABLE schedule_state (
    name TEXT PRIMARY KEY,
    last_fired_minute TEXT,
    claimed_minute TEXT,
    -- 1 = 至少一次（错过的补跑一次）；0 = 至多一次（直接放弃，如实告知 stderr）。
    catch_up INTEGER NOT NULL DEFAULT 1,
    -- outbox（默认，写文件）| notify（文件 + 通知器）| quiet（只落盘日志）。
    delivery TEXT NOT NULL DEFAULT 'outbox',
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE TABLE session_context (
    session_id TEXT PRIMARY KEY,
    observed_input_tokens INTEGER NOT NULL,
    estimated_input_tokens INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE session_permissions (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    mode TEXT NOT NULL CHECK(mode IN ('readOnly', 'standard', 'fullAccess')),
    revision INTEGER NOT NULL CHECK(revision > 0 AND revision <= 2147483647)
);
CREATE TABLE session_queue (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    message    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
, status TEXT NOT NULL DEFAULT 'pending');
CREATE TABLE sessions (
    id         TEXT PRIMARY KEY,
    -- 人起的名字；NULL 时读取侧回退「第一条用户消息」，与 0018 之前的行为一致。
    title      TEXT,
    -- 本会话的工作目录：它同时也是这个会话唯一的可写范围根。NULL = 还没选。
    cwd        TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_at    TEXT
);
CREATE TABLE task_artifacts (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    run_id  TEXT,
    path    TEXT NOT NULL,
    bytes   INTEGER NOT NULL DEFAULT 0,
    at      TEXT NOT NULL DEFAULT (datetime('now')), written_text TEXT,
    UNIQUE(task_id, path)
);
CREATE TABLE task_completions (
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
, verification_id TEXT REFERENCES task_verifications(id), summary TEXT);
CREATE TABLE task_dispatch (
    task_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE,
    turn_id TEXT NOT NULL UNIQUE,
    message TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','claimed','finished','interrupted')),
    created_at TEXT NOT NULL DEFAULT(datetime('now')),
    finished_at TEXT
, attachments_json TEXT NOT NULL DEFAULT '[]', origin_message_id TEXT);
CREATE TABLE task_events (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id  TEXT NOT NULL,
    run_id   TEXT,
    seq      INTEGER NOT NULL DEFAULT 0,
    kind     TEXT NOT NULL,
    detail   TEXT,
    at       TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE task_input_conditions (
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
CREATE TABLE task_input_events (
    source TEXT NOT NULL,
    event_id TEXT NOT NULL,
    condition_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    received_at TEXT NOT NULL DEFAULT(datetime('now')),
    PRIMARY KEY(source,event_id)
);
CREATE TABLE task_input_waits (
    task_id TEXT PRIMARY KEY,
    condition_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'waiting' CHECK(state IN ('waiting','claimed','cancelled')),
    created_at TEXT NOT NULL DEFAULT(datetime('now'))
);
CREATE TABLE task_job_wait_members (
    task_id TEXT NOT NULL REFERENCES task_job_waits(task_id) ON DELETE CASCADE,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, job_id)
);
CREATE TABLE task_job_waits (
    task_id TEXT PRIMARY KEY REFERENCES agent_tasks(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES task_runs(id) ON DELETE CASCADE,
    turn_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'waiting',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE task_pause_fences (
    task_id TEXT PRIMARY KEY REFERENCES agent_tasks(id) ON DELETE CASCADE,
    paused_at TEXT NOT NULL DEFAULT(datetime('now'))
);
CREATE TABLE task_plan_snapshots (
    task_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version > 0),
    steps_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (task_id, version)
);
CREATE TABLE task_runs (
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
CREATE TABLE task_sources (
    task_id     TEXT PRIMARY KEY,
    channel     TEXT NOT NULL,
    chat_id     TEXT NOT NULL,
    message_id  TEXT NOT NULL,
    sender_id   TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE task_steps (
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
CREATE TABLE task_verifications (
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
CREATE TABLE tool_operations (
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
CREATE TABLE turn_checkpoints (
    turn_id      TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL,
    user_message TEXT NOT NULL,
    messages_json TEXT NOT NULL,
    iterations   INTEGER NOT NULL DEFAULT 0,
    usage_json   TEXT NOT NULL DEFAULT '{}',
    status       TEXT NOT NULL DEFAULT 'open',
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
, run_id TEXT, step_seq INTEGER NOT NULL DEFAULT 0, plan_version INTEGER, round_id TEXT, round_no INTEGER CHECK(round_no > 0));
CREATE TABLE usage_ledger (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    TEXT NOT NULL,
    provider      TEXT NOT NULL,
    model         TEXT NOT NULL,
    input_tokens  INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE VIEW active_facts AS SELECT f.* FROM facts f LEFT JOIN memory_validity v ON v.fact_id=f.id
WHERE (v.valid_from IS NULL OR julianday(v.valid_from)<=julianday('now'))
AND (v.valid_until IS NULL OR julianday(v.valid_until)>julianday('now'))
/* active_facts(id,subject,content,source,created_at,embedding,kind,page_path,digest,last_confirmed_at,confirm_count) */;
CREATE VIEW durable_facts AS SELECT f.* FROM active_facts f LEFT JOIN memory_validity v ON v.fact_id=f.id
WHERE v.valid_from IS NULL AND v.valid_until IS NULL
/* durable_facts(id,subject,content,source,created_at,embedding,kind,page_path,digest,last_confirmed_at,confirm_count) */;
CREATE INDEX browser_owner ON browser_sessions(owner_session_id);
CREATE INDEX chat_log_consolidated_idx ON chat_log (consolidated);
CREATE INDEX chat_log_consolidation_batches ON chat_log(consolidated,consolidation_batch,consolidation_next_at);
CREATE INDEX chat_log_session_idx ON chat_log (session_id);
CREATE INDEX chat_log_task_idx ON chat_log (task_id);
CREATE UNIQUE INDEX facts_subject_content_unique
    ON facts (subject, lower(trim(content)));
CREATE INDEX idx_agent_tasks_session_created ON agent_tasks(session_id, created_at DESC);
CREATE INDEX idx_agent_tasks_status ON agent_tasks(status, created_at ASC);
CREATE INDEX idx_approval_log_turn ON approval_log(turn_id, decided_at DESC);
CREATE INDEX idx_jobs_owner_started ON jobs(owner, started_at DESC);
CREATE INDEX idx_task_artifacts_task ON task_artifacts(task_id, id);
CREATE INDEX idx_task_events_task ON task_events(task_id, id);
CREATE INDEX idx_task_runs_status ON task_runs(status, started_at ASC);
CREATE INDEX idx_task_runs_task ON task_runs(task_id, started_at DESC);
CREATE INDEX idx_task_runs_turn ON task_runs(turn_id);
CREATE UNIQUE INDEX idx_task_steps_key ON task_steps(idempotency_key)
    WHERE idempotency_key IS NOT NULL;
CREATE INDEX idx_task_steps_run ON task_steps(run_id, seq);
CREATE INDEX idx_task_steps_task ON task_steps(task_id, id);
CREATE INDEX idx_turn_checkpoints_status ON turn_checkpoints(status, updated_at DESC);
CREATE INDEX idx_usage_model ON usage_ledger(model, created_at DESC);
CREATE INDEX idx_usage_session ON usage_ledger(session_id, created_at DESC);
CREATE INDEX jobs_run ON jobs(run_id);
CREATE INDEX jobs_task ON jobs(task_id);
CREATE INDEX ledger_session_idx ON ledger (session_id, seq);
CREATE INDEX ledger_turn_idx ON ledger (session_id, turn_id, seq);
CREATE INDEX ledger_turns_pending ON ledger_turns (consolidated, at);
CREATE INDEX schedule_completions_pending ON schedule_completions(notify_requested,enqueued_at);
CREATE INDEX session_queue_fifo ON session_queue (session_id, id);
CREATE UNIQUE INDEX session_queue_one_claim ON session_queue(session_id) WHERE status = 'claimed';
CREATE INDEX sessions_recent ON sessions(last_at DESC);
CREATE INDEX task_completions_pending ON task_completions(notify_requested,enqueued_at);
CREATE INDEX task_dispatch_pending ON task_dispatch(state, created_at);
CREATE INDEX task_input_waits_pending ON task_input_waits(state,created_at);
CREATE INDEX task_verifications_latest ON task_verifications(task_id,run_id,measured_at);
CREATE INDEX tool_operations_task ON tool_operations(task_id,started_at);
CREATE TRIGGER episodes_ad AFTER DELETE ON episodes BEGIN
    INSERT INTO episodes_fts(episodes_fts, rowid, summary) VALUES ('delete', old.id, old.summary);
END;
CREATE TRIGGER episodes_ai AFTER INSERT ON episodes BEGIN
    INSERT INTO episodes_fts(rowid, summary) VALUES (new.id, new.summary);
END;
CREATE TRIGGER facts_ad AFTER DELETE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, content) VALUES ('delete', old.id, old.subject, old.content);
END;
CREATE TRIGGER facts_ai AFTER INSERT ON facts BEGIN
    INSERT INTO facts_fts(rowid, subject, content) VALUES (new.id, new.subject, new.content);
END;
CREATE TRIGGER facts_au AFTER UPDATE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, content) VALUES ('delete', old.id, old.subject, old.content);
    INSERT INTO facts_fts(rowid, subject, content) VALUES (new.id, new.subject, new.content);
END;
CREATE TRIGGER task_completion_committed AFTER UPDATE OF status ON agent_tasks
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
CREATE TRIGGER task_completion_unverified_goal AFTER INSERT ON task_completions
WHEN NEW.status='done' AND EXISTS(SELECT 1 FROM agent_tasks WHERE id=NEW.task_id AND acceptance_json='[]')
BEGIN
    UPDATE task_completions SET status='produced_unverified',verification_id=NULL WHERE id=NEW.id;
END;
CREATE TRIGGER task_completion_verification AFTER INSERT ON task_completions
WHEN NEW.status='done'
BEGIN
    UPDATE task_completions SET verification_id=(SELECT id FROM task_verifications
        WHERE task_id=NEW.task_id AND run_id=NEW.run_id
        AND acceptance_json=(SELECT acceptance_json FROM agent_tasks WHERE id=NEW.task_id)
        AND checks_json=(SELECT checks_json FROM agent_tasks WHERE id=NEW.task_id) ORDER BY rowid DESC LIMIT 1)
        WHERE id=NEW.id;
END;
CREATE INDEX idx_task_events_task_seq ON task_events(task_id, seq);
CREATE TABLE action_grants (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL,
    resource TEXT NOT NULL,
    action TEXT NOT NULL,
    destination TEXT NOT NULL,
    content_hash TEXT,
    scope TEXT NOT NULL CHECK (scope IN ('once', 'session', 'day', 'always')),
    expires_at INTEGER,
    revoked_version INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_action_grants_lookup ON action_grants(account, resource, action);
CREATE TABLE pending_approvals (
    turn_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    args_preview TEXT NOT NULL,
    reason TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (turn_id, request_id)
);
CREATE TABLE receipts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    run_id TEXT,
    kind TEXT NOT NULL,
    ref TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('claimed', 'confirmed', 'failed')),
    evidence_path TEXT,
    observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX idx_receipts_task ON receipts(task_id, kind);
CREATE TABLE wake_inbox (
    source_key TEXT PRIMARY KEY,
    target_task TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'delivered', 'reconcile')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    settled_at TEXT
);
CREATE INDEX idx_wake_inbox_state ON wake_inbox(state);
CREATE TABLE task_edit_checks (
    task_id TEXT PRIMARY KEY,
    checks_json TEXT NOT NULL
);
CREATE TABLE task_leases (
    task_id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE TABLE browser_holds (
    task_id TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    origins_json TEXT NOT NULL DEFAULT '[]',
    since TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE connector_wait_cursor (
    condition_id TEXT PRIMARY KEY,
    account_digest TEXT NOT NULL,
    uid_validity INTEGER NOT NULL,
    first_uid INTEGER NOT NULL
);
CREATE TABLE task_git_checkpoint_policies (
    task_id TEXT PRIMARY KEY,
    paths_json TEXT NOT NULL
);
CREATE TABLE task_git_checkpoint_boundaries (
    task_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    boundary_digest TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    PRIMARY KEY (task_id, turn_id, boundary_digest)
);
CREATE TABLE responsibilities (
    id TEXT PRIMARY KEY NOT NULL,
    purpose TEXT NOT NULL,
    sources_json TEXT NOT NULL,
    cron TEXT NOT NULL,
    expires_ms INTEGER NOT NULL,
    budget_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'paused', 'expired')),
    enabled_ms INTEGER NOT NULL,
    last_slot INTEGER
, weekly_report_json TEXT);
CREATE INDEX responsibilities_expiry ON responsibilities(expires_ms) WHERE status != 'expired';
CREATE TABLE weekly_reports (
    wake_key TEXT PRIMARY KEY REFERENCES wake_inbox(source_key),
    responsibility_id TEXT NOT NULL REFERENCES responsibilities(id),
    status TEXT NOT NULL CHECK(status IN ('preparing','draft','awaiting','sending','confirmed','review','denied')),
    body TEXT,
    detail TEXT NOT NULL DEFAULT '',
    tools_used INTEGER NOT NULL DEFAULT 0,
    wall_ms INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX weekly_reports_by_duty ON weekly_reports(responsibility_id);
CREATE INDEX task_children_by_parent ON agent_tasks(parent_task_id) WHERE parent_task_id IS NOT NULL;
CREATE TABLE device_grants (
    device_id TEXT PRIMARY KEY NOT NULL,
    grant_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation BETWEEN 1 AND 2147483647),
    capabilities TEXT NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0,
    last_sequence INTEGER NOT NULL DEFAULT 0
, push_sequence INTEGER);
CREATE TABLE executor_control (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    stopped BOOLEAN NOT NULL,
    generation INTEGER NOT NULL CHECK (generation BETWEEN 0 AND 2147483647)
);
INSERT INTO executor_control VALUES (1, FALSE, 0);
CREATE TABLE browser_downloads (
    session_id TEXT NOT NULL,
    download_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES agent_tasks(id),
    name TEXT NOT NULL,
    body BLOB,
    PRIMARY KEY (session_id, download_id)
);
CREATE TABLE desktop_receipts (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES agent_tasks(id),
    action TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('unknown','confirmed','rejected'))
);
CREATE INDEX desktop_task_receipts ON desktop_receipts(task_id);
CREATE TABLE desktop_model (id INTEGER PRIMARY KEY CHECK(id=1), profile_id TEXT);
INSERT INTO desktop_model VALUES(1,NULL);
CREATE TABLE approval_push_attempts (
  grant_id TEXT NOT NULL, task_id TEXT NOT NULL, turn_id TEXT NOT NULL, request_id TEXT NOT NULL, version INTEGER NOT NULL,
  expires_at INTEGER NOT NULL, PRIMARY KEY (grant_id,turn_id,request_id,version)
);
CREATE INDEX approval_push_expiry ON approval_push_attempts(expires_at);
CREATE UNIQUE INDEX chat_task_notice ON chat_log(session_id,source,task_id,notice_key)
WHERE notice_key IS NOT NULL;
CREATE TRIGGER main_task_completion AFTER INSERT ON task_completions
WHEN NEW.session_id='jeeves-main' AND NEW.status IN ('done','produced_unverified','failed','cancelled')
BEGIN
    INSERT INTO chat_log(role,session_id,source,task_id,notice_key,content)
    VALUES('assistant',NEW.session_id,'task_completion',NEW.task_id,CAST(NEW.id AS TEXT),
        NEW.title || ' · ' || NEW.task_id || char(10) || NEW.status || ' · ' || NEW.reason);
END;
CREATE TRIGGER main_task_decision AFTER INSERT ON pending_approvals
BEGIN
    INSERT OR IGNORE INTO chat_log(role,session_id,source,task_id,notice_key,content)
    SELECT 'assistant',t.session_id,'task_decision',t.id,NEW.request_id||':'||NEW.version,
        t.title || ' · ' || t.id || char(10) || NEW.args_preview || char(10) ||
        '批准 / Approve: approve ' || NEW.request_id || ' v' || NEW.version || char(10) ||
        '拒绝 / Deny: deny ' || NEW.request_id || ' v' || NEW.version
    FROM task_runs r JOIN agent_tasks t ON t.id=r.task_id
    WHERE r.turn_id=NEW.turn_id AND t.session_id='jeeves-main';
END;
CREATE TRIGGER main_task_decision_restored AFTER UPDATE OF version ON pending_approvals
WHEN NEW.version <> OLD.version
BEGIN
    INSERT INTO chat_log(role,session_id,source,task_id,notice_key,content)
    SELECT 'assistant',session_id,'task_decision',task_id,NEW.request_id||':'||NEW.version,
        NEW.args_preview || char(10) || 'Approve: approve ' || NEW.request_id || ' v' || NEW.version ||
        char(10) || 'Deny: deny ' || NEW.request_id || ' v' || NEW.version
    FROM chat_log WHERE source='task_decision' AND session_id='jeeves-main'
        AND notice_key=OLD.request_id||':'||OLD.version;
END;
CREATE TABLE IF NOT EXISTS "mail_actions" (
    id TEXT PRIMARY KEY,
    task_id TEXT REFERENCES agent_tasks(id) ON DELETE CASCADE,
    session_id TEXT,
    account TEXT NOT NULL,
    envelope TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','started','confirmed','rejected','unknown','draft','unapproved')),
    CHECK ((task_id IS NULL) <> (session_id IS NULL))
);
CREATE INDEX mail_actions_task ON mail_actions(task_id);
CREATE INDEX mail_actions_open_session ON mail_actions(session_id,account)
    WHERE state IN ('queued','started','unknown') AND session_id IS NOT NULL;
CREATE INDEX task_delivery_latest ON task_completions(task_id, id DESC);
CREATE TABLE task_review_bases (
    task_id TEXT PRIMARY KEY REFERENCES agent_tasks(id) ON DELETE CASCADE,
    base_commit TEXT NOT NULL
);
CREATE TABLE task_reviews (
    task_id TEXT NOT NULL REFERENCES agent_tasks(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL,
    review_json TEXT NOT NULL,
    PRIMARY KEY (task_id, run_id)
);
