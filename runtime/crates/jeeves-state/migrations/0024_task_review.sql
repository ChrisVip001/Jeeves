CREATE TABLE task_review_bases (
    task_id TEXT PRIMARY KEY REFERENCES agent_tasks(id) ON DELETE CASCADE,
    base_commit TEXT NOT NULL
);
CREATE TABLE task_reviews (
    task_id TEXT NOT NULL REFERENCES agent_tasks(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL,
    review_json TEXT NOT NULL,
    PRIMARY KEY (task_id, run_id)
);
