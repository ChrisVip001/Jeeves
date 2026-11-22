CREATE TABLE conversation_handoffs (
    task_id TEXT PRIMARY KEY REFERENCES agent_tasks(id),
    session_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    instruction TEXT NOT NULL,
    context TEXT NOT NULL,
    evidence_json TEXT NOT NULL,
    operation_id TEXT REFERENCES tool_operations(id),
    UNIQUE(session_id,message_id,instruction)
);
CREATE TABLE price_watches (
    task_id TEXT PRIMARY KEY REFERENCES conversation_handoffs(task_id),
    operation_id TEXT NOT NULL REFERENCES tool_operations(id),
    quote_json TEXT NOT NULL,
    threshold_minor INTEGER NOT NULL CHECK(threshold_minor>0),
    expires_ms INTEGER NOT NULL,
    next_check_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE watch_observations (
    task_id TEXT NOT NULL REFERENCES price_watches(task_id),
    checked_ms INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    threshold_minor INTEGER NOT NULL,
    evidence TEXT NOT NULL,
    PRIMARY KEY(task_id,checked_ms,revision)
);
DROP TRIGGER main_task_completion;
CREATE TRIGGER main_task_completion AFTER INSERT ON task_completions
WHEN NEW.session_id='jeeves-main' AND NEW.status IN ('done','produced_unverified','failed','cancelled')
AND NOT (NEW.status='failed' AND EXISTS(SELECT 1 FROM price_watches WHERE task_id=NEW.task_id))
BEGIN
    INSERT INTO chat_log(role,session_id,source,task_id,notice_key,content)
    VALUES('assistant',NEW.session_id,'task_completion',NEW.task_id,CAST(NEW.id AS TEXT),
        NEW.title || ' · ' || NEW.task_id || char(10) || NEW.status || ' · ' || NEW.reason);
END;
CREATE TABLE task_amendments (
    task_id TEXT NOT NULL REFERENCES agent_tasks(id),
    message_id TEXT NOT NULL,
    instruction TEXT NOT NULL,
    revision INTEGER,
    threshold_minor INTEGER,
    operation_id TEXT REFERENCES tool_operations(id),
    UNIQUE(task_id,revision),
    PRIMARY KEY(task_id,message_id)
);
