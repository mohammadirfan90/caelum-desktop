-- Fail-over pair (F-SYS-04, R-OPS-02; CR-017, docs/integration/M11.md): the signed, hash-chained
-- event log the primary appends and the standby receives, entries quarantined after a fenced
-- primary's unacknowledged writes (kept for review, never re-applied), and small state values
-- (highest fence seen, crew-LAN identity).
CREATE TABLE cluster_log (
    seq          INTEGER PRIMARY KEY CHECK (seq >= 1),
    fence_epoch  INTEGER NOT NULL,
    fence_node   TEXT NOT NULL,
    kind         TEXT NOT NULL,
    at_tai       INTEGER NOT NULL,
    payload      BLOB NOT NULL,
    prev_hash    BLOB NOT NULL CHECK (length(prev_hash) = 32),
    signature    BLOB NOT NULL CHECK (length(signature) = 64)
) STRICT;
CREATE TABLE cluster_quarantine (
    seq          INTEGER NOT NULL,
    fence_epoch  INTEGER NOT NULL,
    fence_node   TEXT NOT NULL,
    kind         TEXT NOT NULL,
    at_tai       INTEGER NOT NULL,
    payload      BLOB NOT NULL,
    prev_hash    BLOB NOT NULL,
    signature    BLOB NOT NULL,
    diverted_at  INTEGER NOT NULL,
    PRIMARY KEY (fence_epoch, fence_node, seq)
) STRICT;
CREATE TABLE cluster_state (
    key          TEXT PRIMARY KEY,
    value        TEXT NOT NULL
) STRICT;
