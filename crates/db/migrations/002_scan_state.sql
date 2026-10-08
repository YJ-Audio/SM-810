-- A quick-hash collision needs two sample rows with the same quick hash.
-- Keep the agreed initial schema intact and relax identity only after verification.
CREATE TABLE samples_next (
    id INTEGER PRIMARY KEY,
    size INTEGER NOT NULL,
    quick_hash BLOB NOT NULL,
    full_hash BLOB,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
INSERT INTO samples_next SELECT * FROM samples;
DROP TABLE samples;
ALTER TABLE samples_next RENAME TO samples;
CREATE INDEX samples_full_hash ON samples(full_hash) WHERE full_hash IS NOT NULL;
CREATE INDEX samples_quick ON samples(size, quick_hash);
CREATE UNIQUE INDEX samples_pending_identity ON samples(size, quick_hash) WHERE full_hash IS NULL;
CREATE UNIQUE INDEX samples_verified_identity ON samples(size, quick_hash, full_hash) WHERE full_hash IS NOT NULL;

-- A failed or interrupted traversal must never make unvisited files disappear.
CREATE TABLE root_state (
    root_id INTEGER PRIMARY KEY REFERENCES roots(id) ON DELETE CASCADE,
    next_generation INTEGER NOT NULL DEFAULT 0,
    complete_generation INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'new' CHECK (status IN ('new', 'scanning', 'online', 'offline', 'partial')),
    error TEXT
);
CREATE INDEX files_root_generation ON files(root_id, last_seen_scan);
