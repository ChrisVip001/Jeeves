(SELECT (
    SELECT json_group_array(json_object('requestId', d.request_id, 'tool', d.tool, 'outcome', d.outcome) ORDER BY d.id)
    FROM approval_log d WHERE d.turn_id = r.turn_id
) FROM task_runs r WHERE r.id = c.run_id AND r.task_id = c.task_id AND r.turn_id IS NOT NULL)
