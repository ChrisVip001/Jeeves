-- `task_completion_unverified_goal` in migrations/0001_init.sql keys off the task contract
-- (`acceptance_json='[]'`), while this predicate keys off evidence (`verification_id IS NULL`).
-- They agree only because Verifications::record rejects an empty acceptance contract: a
-- contractless task never acquires a receipt, so its verification_id stays NULL. Keep both
-- halves in step if that rule ever changes.
CASE WHEN c.status='done' AND c.verification_id IS NULL THEN 'produced_unverified' ELSE c.status END
