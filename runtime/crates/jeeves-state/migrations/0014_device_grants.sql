-- Keep revoked rows so pairing the same public key cannot resurrect an old generation.
CREATE TABLE device_grants (
    device_id TEXT PRIMARY KEY NOT NULL,
    grant_id TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation BETWEEN 1 AND 2147483647),
    capabilities TEXT NOT NULL,
    revoked INTEGER NOT NULL DEFAULT 0,
    last_sequence INTEGER NOT NULL DEFAULT 0
);
