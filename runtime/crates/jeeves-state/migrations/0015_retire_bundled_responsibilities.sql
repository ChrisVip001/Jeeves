ALTER TABLE responsibilities ADD COLUMN migration_note TEXT;
ALTER TABLE responsibility_event_samples ADD COLUMN event_id TEXT;

CREATE TABLE responsibility_predecessors (
    task_id TEXT PRIMARY KEY REFERENCES agent_tasks(id),
    responsibility_id TEXT NOT NULL REFERENCES responsibilities(id)
);

INSERT INTO responsibility_predecessors
    SELECT w.task_id,r.id FROM price_watches w JOIN responsibilities r ON r.id='watch-'||w.task_id;

DROP TRIGGER bind_new_price_watch;
DROP TRIGGER revise_price_watch;
DROP TRIGGER close_price_watch;
DROP TRIGGER resume_price_clock;

UPDATE wake_inbox
SET state='reconcile',settled_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
WHERE state='pending' AND EXISTS (
    SELECT 1 FROM responsibilities r WHERE target_task='responsibility:'||r.id AND r.clock_owner!='cron'
);

INSERT INTO responsibility_runs(wake_key,responsibility_id,status,detail)
SELECT w.source_key,r.id,'review','Occurrence held during compatibility cutover; check its status before continuing'
FROM wake_inbox w JOIN responsibilities r ON w.target_task='responsibility:'||r.id
WHERE w.state='reconcile' AND r.clock_owner!='cron'
ON CONFLICT(wake_key) DO NOTHING;

UPDATE responsibility_runs
SET status='review',detail='Execution interrupted during compatibility cutover; verify effects before continuing'
WHERE status IN ('preparing','awaiting','sending') AND wake_key IN (
    SELECT source_key FROM wake_inbox w
    JOIN responsibilities r ON w.target_task='responsibility:'||r.id
    WHERE w.state='reconcile' AND r.clock_owner!='cron'
);

DROP VIEW responsibility_snapshots;
CREATE VIEW responsibility_snapshots AS
SELECT id,revision,json_object(
    'purpose',purpose,'sources',json(sources_json),'cron',cron,'expiresMs',expires_ms,
    'budget',json(budget_json),'researchTaskId',research_task_id,'feishuRecipient',feishu_recipient,
    'digest',json(digest_json),'status',status,'plan',json(plan_json),'migrationNote',migration_note
) AS definition FROM responsibilities;

UPDATE responsibilities
SET plan_json=CASE WHEN clock_owner='morning' AND json_extract(plan_json,'$.trigger.kind')='cron'
               THEN json_object('trigger',json(plan_json->'$.trigger'),
                    'action',json_object('kind','digest','version',1,'config',json_object(
                        'sources',json(sources_json),'options',json_object(
                            'timeZone',json_extract(plan_json,'$.trigger.timeZone'),
                            'calendarWindow','today','layout','list'))),
                    'delivery',json_object('kind','draft'))
               ELSE NULL END,
    status='paused',revision=revision+1,next_due_ms=NULL,
    migration_note=CASE WHEN clock_owner='morning'
        THEN 'Review the source coverage and delivery destination before resuming this imported schedule.'
        ELSE 'Connect a structured read source and review the matching rule before resuming this imported monitor.' END,
    clock_owner='cron'
WHERE clock_owner!='cron' AND status IN ('active','paused');


UPDATE agent_tasks
SET status='suspended',last_stop_reason='The recurring monitor moved to Recurring Work. Review its sources and conditions there.'
WHERE status IN ('pending','running','needs_attention') AND id IN (SELECT task_id FROM responsibility_predecessors);

UPDATE responsibilities
SET plan_json=json_set(plan_json,'$.action.kind','verifiedSummary','$.action.config.lookbackDays',7)
WHERE json_extract(plan_json,'$.action.kind')='researchDigest';

UPDATE responsibilities
SET plan_json=json_set(plan_json,'$.delivery',
    json_object('kind','connector','connector','feishu','recipient',json_extract(plan_json,'$.delivery.recipient')))
WHERE json_extract(plan_json,'$.delivery.kind')='feishu';

UPDATE responsibilities SET next_due_ms=0
WHERE research_task_id IS NOT NULL AND plan_json IS NOT NULL AND next_due_ms IS NULL AND status!='expired';

UPDATE responsibility_reviews
SET state='superseded'
WHERE state='pending' AND (duty_id IN (SELECT id FROM responsibilities WHERE migration_note IS NOT NULL)
   OR json_extract(contract_json,'$.plan.action.kind') IN ('morningBriefing','productQuote','researchDigest'));
