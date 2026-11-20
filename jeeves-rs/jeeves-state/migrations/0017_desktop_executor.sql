-- One human action per observation. An interrupted dispatch leaves a visible unknown outcome.
CREATE TABLE desktop_receipts (
    id TEXT PRIMARY KEY NOT NULL,
    task_id TEXT NOT NULL REFERENCES agent_tasks(id),
    action TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('unknown','confirmed','rejected'))
);
CREATE INDEX desktop_task_receipts ON desktop_receipts(task_id);
CREATE TABLE desktop_model (id INTEGER PRIMARY KEY CHECK(id=1), profile_id TEXT);
INSERT INTO desktop_model VALUES(1,NULL);
