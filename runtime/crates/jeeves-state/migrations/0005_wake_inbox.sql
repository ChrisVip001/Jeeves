-- expand only: one new table. Every wake (scheduled firing, external input) is recorded under a
-- source key that is unique, so the same source delivers once.
CREATE TABLE IF NOT EXISTS wake_inbox (
    source_key TEXT PRIMARY KEY,
    target_task TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'delivered', 'reconcile')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    settled_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_wake_inbox_state ON wake_inbox(state);
