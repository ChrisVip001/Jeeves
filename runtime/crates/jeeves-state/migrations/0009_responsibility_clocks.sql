ALTER TABLE responsibilities ADD COLUMN clock_owner TEXT NOT NULL DEFAULT 'cron';
CREATE INDEX responsibility_clock ON responsibilities(clock_owner,status);
