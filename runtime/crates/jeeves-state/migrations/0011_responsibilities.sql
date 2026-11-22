-- Authorization is distinct from model-managed schedule_jobs. last_slot is a UTC minute.
CREATE TABLE responsibilities (
    id TEXT PRIMARY KEY NOT NULL,
    purpose TEXT NOT NULL,
    sources_json TEXT NOT NULL,
    cron TEXT NOT NULL,
    expires_ms INTEGER NOT NULL,
    budget_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'paused', 'expired')),
    enabled_ms INTEGER NOT NULL,
    last_slot INTEGER
);
CREATE INDEX responsibilities_expiry ON responsibilities(expires_ms) WHERE status != 'expired';
