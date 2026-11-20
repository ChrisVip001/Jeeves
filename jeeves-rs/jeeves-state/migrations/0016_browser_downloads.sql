-- The operator's accepted bytes survive a crash between publishing and recording the artifact.
CREATE TABLE browser_downloads (
    session_id TEXT NOT NULL,
    download_id TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES agent_tasks(id),
    name TEXT NOT NULL,
    body BLOB,
    PRIMARY KEY (session_id, download_id)
);
