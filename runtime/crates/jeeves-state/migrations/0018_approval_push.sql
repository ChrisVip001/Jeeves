-- A subscription belongs to one grant; its endpoint and encryption keys live in SecretStore.
ALTER TABLE device_grants ADD COLUMN push_sequence INTEGER;

CREATE TABLE approval_push_attempts (
  grant_id TEXT NOT NULL, task_id TEXT NOT NULL, turn_id TEXT NOT NULL, request_id TEXT NOT NULL, version INTEGER NOT NULL,
  expires_at INTEGER NOT NULL, PRIMARY KEY (grant_id,turn_id,request_id,version)
);
CREATE INDEX approval_push_expiry ON approval_push_attempts(expires_at);
