-- expand only: one lease per running task dispatch. A worker renews it while its turn runs; a
-- lease that stops being renewed is swept exactly once (the row is deleted by the sweep).
CREATE TABLE IF NOT EXISTS task_leases (
    task_id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
