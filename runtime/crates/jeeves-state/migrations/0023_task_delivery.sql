ALTER TABLE task_completions ADD COLUMN summary TEXT;
CREATE INDEX task_delivery_latest ON task_completions(task_id, id DESC);

WITH notices AS (
    SELECT c.id, m.content,
           c.title || ' · ' || c.task_id || char(10) || c.status || ' · ' || c.reason || char(10,10) AS prefix
    FROM task_completions c JOIN chat_log m
      ON m.task_id=c.task_id AND m.session_id=c.session_id
     AND m.source='task_completion' AND m.notice_key=CAST(c.id AS TEXT)
)
UPDATE task_completions SET summary=substr(n.content,length(n.prefix)+1)
FROM notices n WHERE task_completions.id=n.id AND instr(n.content,n.prefix)=1;
ALTER TABLE task_artifacts ADD COLUMN written_text TEXT;
