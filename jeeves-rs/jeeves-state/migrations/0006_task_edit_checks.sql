-- expand only: per-task checks that run after each edit (`after: edit`). Absent row = none.
CREATE TABLE IF NOT EXISTS task_edit_checks (
    task_id TEXT PRIMARY KEY,
    checks_json TEXT NOT NULL
);
