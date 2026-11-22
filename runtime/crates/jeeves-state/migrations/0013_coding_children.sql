-- A child is an ordinary durable task. Existing tasks have no parent.
ALTER TABLE agent_tasks ADD COLUMN parent_task_id TEXT REFERENCES agent_tasks(id);
ALTER TABLE agent_tasks ADD COLUMN integrated_commit TEXT;
CREATE INDEX task_children_by_parent ON agent_tasks(parent_task_id) WHERE parent_task_id IS NOT NULL;
