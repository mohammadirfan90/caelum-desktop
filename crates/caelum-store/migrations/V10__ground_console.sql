-- Ground console profile (F-GND-08; CR-025): what a ground installation receives from the
-- downlink, and the private medical conference record kept by the flight surgeon.
CREATE TABLE ground_summary (
    id              TEXT PRIMARY KEY,
    crew_member_id  TEXT NOT NULL,
    parameter       TEXT NOT NULL,
    window_start    INTEGER NOT NULL,
    window_end      INTEGER NOT NULL,
    quality         TEXT NOT NULL CHECK (quality IN ('good', 'degraded', 'poor', 'unusable')),
    min_value       REAL,
    mean_value      REAL,
    max_value       REAL,
    state           TEXT,
    count           INTEGER NOT NULL CHECK (count >= 1),
    received_at     INTEGER NOT NULL,
    UNIQUE (crew_member_id, parameter, window_end)
) STRICT;
CREATE INDEX ground_summary_by_crew ON ground_summary (crew_member_id, parameter, window_end);
CREATE TABLE ground_ops (
    seq_key         TEXT PRIMARY KEY,          -- apid:seq:packet time, de-duplicates resends
    kind            TEXT NOT NULL CHECK (kind IN ('ops_alarm', 'crew_status', 'instance_status')),
    at_tai          INTEGER NOT NULL,
    message         TEXT NOT NULL CHECK (json_valid(message))
) STRICT;
CREATE INDEX ground_ops_by_time ON ground_ops (kind, at_tai);
CREATE TABLE ground_file (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    kind            TEXT NOT NULL,
    content         BLOB NOT NULL,
    received_at     INTEGER NOT NULL
) STRICT;
CREATE TABLE pmc_record (
    id              TEXT PRIMARY KEY,
    crew_member_id  TEXT NOT NULL,
    author          TEXT NOT NULL,
    held_at         INTEGER NOT NULL,
    notes           TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    signed_at       INTEGER
) STRICT;
-- A signed private medical conference record is immutable (F-CLN-05 rule applied to PMC).
CREATE TRIGGER pmc_record_no_update_after_sign BEFORE UPDATE ON pmc_record
WHEN OLD.signed_at IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'a signed private medical conference record is immutable'); END;
