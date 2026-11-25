CREATE TABLE control_leases (
    lease_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    owner_session_id TEXT,
    device TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    status TEXT NOT NULL,
    reason TEXT,
    expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_control_leases_task ON control_leases(task_id);
CREATE INDEX idx_control_leases_owner ON control_leases(owner_session_id);
