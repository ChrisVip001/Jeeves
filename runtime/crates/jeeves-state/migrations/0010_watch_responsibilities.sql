CREATE VIEW price_watch_definitions AS
SELECT 'watch-'||w.task_id AS id, w.task_id,
       printf('%s %s · < %.2f %s',json_extract(w.quote_json,'$.model'),
              json_extract(w.quote_json,'$.spec'),w.threshold_minor/100.0,
              json_extract(w.quote_json,'$.currency')) AS purpose,
       json_array(json_extract(w.quote_json,'$.url')) AS sources_json,
       w.expires_ms,t.budget_json,w.revision,
       CASE WHEN t.status IN ('pending','running') THEN 'active'
            WHEN t.status IN ('suspended','needs_attention') THEN 'paused' ELSE 'expired' END AS status
FROM price_watches w JOIN agent_tasks t ON t.id=w.task_id;

INSERT INTO responsibilities(id,purpose,sources_json,cron,expires_ms,budget_json,revision,status,enabled_ms,last_slot,clock_owner)
SELECT v.id,v.purpose,v.sources_json,'0 * * * *',v.expires_ms,v.budget_json,v.revision,
       CASE WHEN v.status='active' AND t.spent_tools>(SELECT count(*) FROM watch_observations o WHERE o.task_id=v.task_id)
            THEN 'paused' ELSE v.status END,
       unixepoch(t.created_at)*1000,(SELECT max(checked_ms)/60000 FROM watch_observations o WHERE o.task_id=v.task_id),'price-watch'
FROM price_watch_definitions v JOIN agent_tasks t ON t.id=v.task_id;

CREATE TRIGGER bind_new_price_watch AFTER INSERT ON price_watches BEGIN
    INSERT INTO responsibilities(id,purpose,sources_json,cron,expires_ms,budget_json,revision,status,enabled_ms,clock_owner)
    SELECT id,purpose,sources_json,'0 * * * *',expires_ms,budget_json,revision,status,NEW.next_check_ms,'price-watch'
    FROM price_watch_definitions WHERE task_id=NEW.task_id;
END;
CREATE TRIGGER revise_price_watch AFTER UPDATE OF revision ON price_watches BEGIN
    UPDATE responsibilities SET revision=revision+1,purpose=(SELECT purpose FROM price_watch_definitions WHERE task_id=NEW.task_id)
    WHERE id='watch-'||NEW.task_id AND clock_owner='price-watch';
END;
CREATE TRIGGER close_price_watch AFTER UPDATE OF status ON agent_tasks
WHEN NEW.status<>OLD.status AND NEW.status IN ('done','failed','cancelled','suspended','needs_attention') BEGIN
    UPDATE responsibilities SET status=CASE WHEN NEW.status IN ('suspended','needs_attention') THEN 'paused' ELSE 'expired' END,revision=revision+1
    WHERE id='watch-'||NEW.id AND clock_owner='price-watch' AND status<>'expired';
END;
CREATE TRIGGER resume_price_clock AFTER UPDATE OF status ON responsibilities
WHEN NEW.clock_owner='price-watch' AND OLD.status='paused' AND NEW.status='active' BEGIN
    UPDATE price_watches SET next_check_ms=max(next_check_ms,NEW.enabled_ms+3600000)
    WHERE 'watch-'||task_id=NEW.id;
END;
