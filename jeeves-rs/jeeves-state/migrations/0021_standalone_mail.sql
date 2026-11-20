-- A chat-only turn can call the same live connector without owning a task.
CREATE TABLE mail_actions_next (
    id TEXT PRIMARY KEY,
    task_id TEXT REFERENCES agent_tasks(id) ON DELETE CASCADE,
    session_id TEXT,
    account TEXT NOT NULL,
    envelope TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','started','confirmed','rejected','unknown','draft','unapproved')),
    CHECK ((task_id IS NULL) <> (session_id IS NULL))
);
INSERT INTO mail_actions_next(id,task_id,account,envelope,state)
    SELECT id,task_id,account,envelope,state FROM mail_actions;
DROP TABLE mail_actions;
ALTER TABLE mail_actions_next RENAME TO mail_actions;
CREATE INDEX mail_actions_task ON mail_actions(task_id);
CREATE INDEX mail_actions_open_session ON mail_actions(session_id,account)
    WHERE state IN ('queued','started','unknown') AND session_id IS NOT NULL;
