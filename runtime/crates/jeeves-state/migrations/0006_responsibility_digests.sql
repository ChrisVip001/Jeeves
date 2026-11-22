ALTER TABLE responsibilities ADD COLUMN digest_json TEXT;
DROP VIEW responsibility_snapshots;
CREATE VIEW responsibility_snapshots AS
SELECT id,revision,json_object(
    'purpose',purpose,'sources',json(sources_json),'cron',cron,'expiresMs',expires_ms,
    'budget',json(budget_json),'weeklyReport',json(weekly_report_json),'digest',json(digest_json),'status',status
) AS definition FROM responsibilities;
