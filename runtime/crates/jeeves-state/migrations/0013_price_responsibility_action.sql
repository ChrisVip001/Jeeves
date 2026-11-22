CREATE VIEW price_responsibility_plans AS
SELECT 'watch-'||task_id AS duty_id,
       json_object('trigger',json_object('kind','every','intervalSeconds',3600,'anchor',strftime('%Y-%m-%dT%H:%M:%fZ',next_check_ms/1000.0,'unixepoch')),
         'action',json_object('kind','productQuote','version',1,'config',json('{}')),
         'delivery',json_object('kind','chat')) AS plan
FROM price_watches;

UPDATE responsibilities SET plan_json=(SELECT plan FROM price_responsibility_plans WHERE duty_id=id)
WHERE clock_owner='price-watch' AND plan_json IS NULL;

DROP TRIGGER bind_new_price_watch;
CREATE TRIGGER bind_new_price_watch AFTER INSERT ON price_watches BEGIN
    INSERT INTO responsibilities(id,purpose,sources_json,cron,expires_ms,budget_json,revision,status,enabled_ms,clock_owner,plan_json)
    SELECT id,purpose,sources_json,'0 * * * *',expires_ms,budget_json,revision,status,NEW.next_check_ms,'price-watch',p.plan
    FROM price_watch_definitions JOIN price_responsibility_plans p ON p.duty_id=id WHERE task_id=NEW.task_id;
END;
