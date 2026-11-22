ALTER TABLE tool_operations ADD COLUMN output_preview TEXT;

CREATE TABLE model_requests (
    id INTEGER PRIMARY KEY,
    session_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    task_id TEXT REFERENCES agent_tasks(id),
    run_id TEXT REFERENCES task_runs(id),
    round_id TEXT,
    purpose TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    termination TEXT NOT NULL DEFAULT 'in_flight',
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read INTEGER,
    cache_creation INTEGER,
    charged INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX model_requests_owner ON model_requests(run_id,round_id);

CREATE INDEX model_requests_pending ON model_requests(task_id) WHERE charged=0 AND input_tokens IS NOT NULL;

CREATE TRIGGER attach_request_to_new_run AFTER INSERT ON task_runs
WHEN EXISTS (SELECT 1 FROM model_requests WHERE turn_id=NEW.turn_id AND task_id=NEW.task_id AND run_id IS NULL)
BEGIN
    UPDATE task_runs SET tokens_in=tokens_in+(
        SELECT COALESCE(SUM(input_tokens),0) FROM model_requests
        WHERE turn_id=NEW.turn_id AND task_id=NEW.task_id AND run_id IS NULL AND charged=1
    ), tokens_out=tokens_out+(
        SELECT COALESCE(SUM(output_tokens),0) FROM model_requests
        WHERE turn_id=NEW.turn_id AND task_id=NEW.task_id AND run_id IS NULL AND charged=1
    ) WHERE id=NEW.id;
    UPDATE model_requests SET run_id=NEW.id
        WHERE turn_id=NEW.turn_id AND task_id=NEW.task_id AND run_id IS NULL;
END;

CREATE TRIGGER enforce_request_token_budget BEFORE INSERT ON model_requests
WHEN EXISTS (
    SELECT 1 FROM agent_tasks t WHERE t.id=NEW.task_id AND t.spent_tokens >= json_extract(t.budget_json,'$.tokens')
)
BEGIN
    SELECT RAISE(ABORT,'Task token budget exhausted before model request');
END;

CREATE TRIGGER charge_model_request AFTER UPDATE OF charged ON model_requests
WHEN OLD.charged=0 AND NEW.charged=1 AND NEW.task_id IS NOT NULL AND NEW.input_tokens IS NOT NULL
BEGIN
    UPDATE task_runs SET tokens_in=tokens_in+NEW.input_tokens,
        tokens_out=tokens_out+NEW.output_tokens WHERE id=NEW.run_id;
    UPDATE agent_tasks SET spent_tokens=spent_tokens+NEW.input_tokens+NEW.output_tokens
        WHERE id=NEW.task_id;
    INSERT INTO run_round_accounts(run_id,round_id,tokens_in,tokens_out)
        SELECT NEW.run_id,NEW.round_id,NEW.input_tokens,NEW.output_tokens WHERE NEW.round_id IS NOT NULL
        ON CONFLICT(run_id,round_id) DO UPDATE SET tokens_in=tokens_in+excluded.tokens_in,
            tokens_out=tokens_out+excluded.tokens_out;
END;
