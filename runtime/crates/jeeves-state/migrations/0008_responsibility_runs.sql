ALTER TABLE responsibilities ADD COLUMN research_task_id TEXT;
ALTER TABLE responsibilities ADD COLUMN feishu_recipient TEXT CHECK ((research_task_id IS NULL) = (feishu_recipient IS NULL));
UPDATE responsibilities SET research_task_id=json_extract(weekly_report_json,'$.taskId'),
    feishu_recipient=json_extract(weekly_report_json,'$.recipient') WHERE weekly_report_json IS NOT NULL;
DROP VIEW responsibility_snapshots;
CREATE VIEW responsibility_snapshots AS
SELECT id,revision,json_object(
    'purpose',purpose,'sources',json(sources_json),'cron',cron,'expiresMs',expires_ms,
    'budget',json(budget_json),'researchTaskId',research_task_id,'feishuRecipient',feishu_recipient,
    'digest',json(digest_json),'status',status
) AS definition FROM responsibilities;
ALTER TABLE responsibilities DROP COLUMN weekly_report_json;
ALTER TABLE weekly_reports RENAME TO responsibility_runs;
