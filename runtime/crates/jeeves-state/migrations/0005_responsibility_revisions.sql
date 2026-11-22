ALTER TABLE responsibilities ADD COLUMN revision INTEGER NOT NULL DEFAULT 1 CHECK(revision BETWEEN 1 AND 2147483647);
ALTER TABLE wake_inbox ADD COLUMN responsibility_revision INTEGER;
CREATE TABLE responsibility_revisions (
    responsibility_id TEXT NOT NULL REFERENCES responsibilities(id),
    revision INTEGER NOT NULL,
    definition TEXT NOT NULL,
    PRIMARY KEY(responsibility_id,revision)
);
CREATE VIEW responsibility_snapshots AS
SELECT id,revision,json_object(
    'purpose',purpose,'sources',json(sources_json),'cron',cron,'expiresMs',expires_ms,
    'budget',json(budget_json),'weeklyReport',json(weekly_report_json),'status',status
) AS definition FROM responsibilities;
INSERT INTO responsibility_revisions SELECT * FROM responsibility_snapshots;
UPDATE wake_inbox SET responsibility_revision=1 WHERE EXISTS
(SELECT 1 FROM responsibilities WHERE target_task='responsibility:' || id AND source_key LIKE 'responsibility:%');
CREATE TRIGGER record_responsibility_creation AFTER INSERT ON responsibilities BEGIN
    INSERT INTO responsibility_revisions SELECT * FROM responsibility_snapshots WHERE id=NEW.id;
END;
CREATE TRIGGER record_responsibility_revision AFTER UPDATE OF revision ON responsibilities BEGIN
    INSERT INTO responsibility_revisions SELECT * FROM responsibility_snapshots WHERE id=NEW.id;
END;
