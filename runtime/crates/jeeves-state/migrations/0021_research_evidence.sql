-- Durable private evidence, not writable through workspace tools.
CREATE TABLE research_evidence (
    task_id TEXT PRIMARY KEY REFERENCES agent_tasks(id) ON DELETE CASCADE,
    state_json TEXT NOT NULL
);
