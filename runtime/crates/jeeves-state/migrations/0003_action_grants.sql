-- expand only: two new tables, nothing existing is touched.
-- action_grants: a human's yes, bound to who/what/where (and optionally the exact content).
CREATE TABLE IF NOT EXISTS action_grants (
    id TEXT PRIMARY KEY,
    account TEXT NOT NULL,
    resource TEXT NOT NULL,
    action TEXT NOT NULL,
    destination TEXT NOT NULL,
    content_hash TEXT,
    scope TEXT NOT NULL CHECK (scope IN ('once', 'session', 'day', 'always')),
    expires_at INTEGER,
    revoked_version INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_action_grants_lookup ON action_grants(account, resource, action);

-- pending_approvals: questions waiting for a human, so a restart does not lose them.
CREATE TABLE IF NOT EXISTS pending_approvals (
    turn_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    tool TEXT NOT NULL,
    args_preview TEXT NOT NULL,
    reason TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (turn_id, request_id)
);
