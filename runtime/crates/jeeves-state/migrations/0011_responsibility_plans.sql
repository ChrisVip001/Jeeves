ALTER TABLE responsibilities ADD COLUMN plan_json TEXT CHECK(plan_json IS NULL OR json_valid(plan_json));
ALTER TABLE responsibilities ADD COLUMN next_due_ms INTEGER;
ALTER TABLE wake_inbox ADD COLUMN due_ms INTEGER;
ALTER TABLE wake_inbox ADD COLUMN event_json TEXT;
ALTER TABLE responsibility_runs ADD COLUMN lease_until_ms INTEGER;
ALTER TABLE responsibility_runs ADD COLUMN progress_ms INTEGER;
ALTER TABLE responsibility_runs ADD COLUMN outcome TEXT;
CREATE INDEX responsibility_due ON responsibilities(next_due_ms) WHERE plan_json IS NOT NULL AND status='active';

DROP VIEW responsibility_snapshots;
CREATE VIEW responsibility_snapshots AS
SELECT id,revision,json_object(
    'purpose',purpose,'sources',json(sources_json),'cron',cron,'expiresMs',expires_ms,
    'budget',json(budget_json),'researchTaskId',research_task_id,'feishuRecipient',feishu_recipient,
    'digest',json(digest_json),'status',status,'plan',json(plan_json)
) AS definition FROM responsibilities;

CREATE TABLE responsibility_event_positions (
    source TEXT NOT NULL,
    event_type TEXT NOT NULL,
    event_id TEXT NOT NULL,
    cursor TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    PRIMARY KEY(source,event_type,event_id)
);
CREATE TABLE responsibility_event_cursors (
    source TEXT NOT NULL,
    event_type TEXT NOT NULL,
    cursor TEXT NOT NULL,
    PRIMARY KEY(source,event_type)
);
