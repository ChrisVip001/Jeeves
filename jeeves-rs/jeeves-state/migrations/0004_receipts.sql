-- expand only: one new table. A receipt is what a tool *claimed* it did, and later whether a
-- read-back *confirmed* it. Acceptance can ask for the second.
CREATE TABLE IF NOT EXISTS receipts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL,
    run_id TEXT,
    kind TEXT NOT NULL,
    ref TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('claimed', 'confirmed', 'failed')),
    evidence_path TEXT,
    observed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_receipts_task ON receipts(task_id, kind);
