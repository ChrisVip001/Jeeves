CREATE TABLE task_dependencies (
    task_id TEXT NOT NULL REFERENCES agent_tasks(id),
    prerequisite_id TEXT NOT NULL REFERENCES agent_tasks(id),
    first_completion_id INTEGER NOT NULL,
    PRIMARY KEY (task_id, prerequisite_id),
    CHECK (task_id <> prerequisite_id)
);

CREATE INDEX task_dependencies_by_prerequisite ON task_dependencies(prerequisite_id);

CREATE VIEW pending_task_dependencies AS
SELECT d.task_id, d.prerequisite_id
FROM task_dependencies d
WHERE NOT EXISTS (
    SELECT 1 FROM task_completions c
    WHERE c.task_id=d.prerequisite_id AND c.id>=d.first_completion_id
      AND c.status='done' AND c.verification_id IS NOT NULL
);

CREATE VIEW ready_task_dispatches AS
SELECT d.task_id, d.rowid AS dispatch_order
FROM task_dispatch d JOIN agent_tasks t ON t.id=d.task_id
WHERE d.state='pending' AND t.status='pending'
  AND NOT EXISTS (SELECT 1 FROM task_pause_fences p WHERE p.task_id=t.id)
  AND NOT EXISTS (SELECT 1 FROM pending_task_dependencies w WHERE w.task_id=t.id);
