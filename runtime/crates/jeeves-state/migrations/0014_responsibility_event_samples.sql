CREATE TABLE responsibility_event_samples (
    source TEXT NOT NULL,
    event_type TEXT NOT NULL,
    fields TEXT NOT NULL CHECK(json_valid(fields)),
    sampled_ms INTEGER NOT NULL,
    PRIMARY KEY(source,event_type)
);
CREATE INDEX responsibility_event_subscribers ON responsibilities(
    json_extract(plan_json,'$.trigger.source'), json_extract(plan_json,'$.trigger.eventType')
) WHERE status='active' AND json_extract(plan_json,'$.trigger.kind')='event';
CREATE INDEX responsibility_run_live ON responsibility_runs(responsibility_id, lease_until_ms)
    WHERE status IN ('preparing','awaiting','sending');
CREATE UNIQUE INDEX responsibility_event_position_cursor ON responsibility_event_positions(source,event_type,cursor);
CREATE INDEX responsibility_model_requests ON model_requests(created_at) WHERE purpose='responsibility';
