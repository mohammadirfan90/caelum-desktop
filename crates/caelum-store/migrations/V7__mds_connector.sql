-- MDS connector (F-GND-12/13, CR-016, docs/integration/M10.md §1): the V1 mds_outbox and
-- mds_inbox predate the design and were never used. Outbox records and read-back state are kept
-- as the connector's JSON records; the client identity (certificate and key) is stored in this
-- encrypted database, set by an administrator (audited, CR-016 §10).
DROP TABLE mds_outbox;
DROP TABLE mds_inbox;
CREATE TABLE mds_outbox (
    client_id   TEXT PRIMARY KEY,
    record      TEXT NOT NULL CHECK (json_valid(record))
) STRICT;
CREATE TABLE mds_inbox_seen (
    source_id   TEXT PRIMARY KEY,
    version     TEXT NOT NULL
) STRICT;
CREATE TABLE mds_state (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL
) STRICT;
