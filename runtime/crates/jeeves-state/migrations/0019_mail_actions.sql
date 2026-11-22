-- The immutable envelope is also the recoverable draft if amendment wins the claim.
CREATE TABLE mail_actions (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES agent_tasks(id) ON DELETE CASCADE,
    account TEXT NOT NULL,
    envelope TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','started','confirmed','rejected','unknown','draft','unapproved'))
);
CREATE INDEX mail_actions_task ON mail_actions(task_id);
