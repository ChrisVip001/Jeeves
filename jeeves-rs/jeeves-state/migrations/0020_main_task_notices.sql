-- The notice belongs to the business transition. A rolled-back transition leaves no chat row.
ALTER TABLE chat_log ADD COLUMN notice_key TEXT;
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

-- A restarted request carries a new version. Preserve the old review as history and
-- post the current decision command so the owner can answer from the same chat.
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
