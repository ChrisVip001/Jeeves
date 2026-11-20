-- An opted-in new-mail condition remembers the mailbox generation and starting UID.
-- It is never reset silently after a process restart or a changed mailbox.
CREATE TABLE IF NOT EXISTS connector_wait_cursor (
    condition_id TEXT PRIMARY KEY,
    account_digest TEXT NOT NULL,
    uid_validity INTEGER NOT NULL,
    first_uid INTEGER NOT NULL
);
