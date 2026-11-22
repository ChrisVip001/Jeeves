ALTER TABLE responsibilities ADD COLUMN weekly_report_json TEXT;
CREATE TABLE weekly_reports (
    wake_key TEXT PRIMARY KEY REFERENCES wake_inbox(source_key),
    responsibility_id TEXT NOT NULL REFERENCES responsibilities(id),
    status TEXT NOT NULL CHECK(status IN ('preparing','draft','awaiting','sending','confirmed','review','denied')),
    body TEXT,
    detail TEXT NOT NULL DEFAULT '',
    tools_used INTEGER NOT NULL DEFAULT 0,
    wall_ms INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX weekly_reports_by_duty ON weekly_reports(responsibility_id);
