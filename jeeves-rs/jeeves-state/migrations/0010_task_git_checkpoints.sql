-- Human opt-in to selected paths in a task's private Git worktree.
CREATE TABLE IF NOT EXISTS task_git_checkpoint_policies (
    task_id TEXT PRIMARY KEY,
    paths_json TEXT NOT NULL
);
-- Each opted-in execution boundary maps to one commit, including unchanged boundaries.
CREATE TABLE IF NOT EXISTS task_git_checkpoint_boundaries (
    task_id TEXT NOT NULL,
    turn_id TEXT NOT NULL,
    boundary_digest TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    PRIMARY KEY (task_id, turn_id, boundary_digest)
);
