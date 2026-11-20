-- expand only: a new index. The global cursor is task_events.id (already the primary key,
-- monotonic); per-task order is seq, so reads filtered by task need (task_id, seq).
CREATE INDEX IF NOT EXISTS idx_task_events_task_seq ON task_events(task_id, seq);
