-- expand only: a human takeover (browser status 'human') that was in progress when the Runtime
-- stopped. The browser process dies with the Runtime; the hold does not, so the task is not
-- resumed behind the human's back and the page is re-observed after they open a fresh browser.
CREATE TABLE IF NOT EXISTS browser_holds (
    task_id TEXT PRIMARY KEY,
    url TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    origins_json TEXT NOT NULL DEFAULT '[]',
    since TEXT NOT NULL DEFAULT (datetime('now'))
);
