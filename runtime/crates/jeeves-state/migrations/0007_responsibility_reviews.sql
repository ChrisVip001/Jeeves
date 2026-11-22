CREATE TABLE responsibility_reviews (
    id TEXT PRIMARY KEY,
    duty_id TEXT NOT NULL,
    expected_revision INTEGER NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('create','amend','pause','resume')),
    request TEXT NOT NULL,
    contract_json TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','accepted','superseded','dismissed'))
);
CREATE INDEX responsibility_reviews_pending ON responsibility_reviews(duty_id) WHERE state='pending';
