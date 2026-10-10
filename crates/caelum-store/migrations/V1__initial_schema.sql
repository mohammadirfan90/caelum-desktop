-- CAELUM schema v1: every table of docs/05_DATA_MODEL.md §2 (F-SYS-02, R-SEC-01, R-SEC-03).
-- Conventions: IDs are UUID text; times are TAI nanoseconds since 1958-01-01 (INTEGER, `*_tai`);
-- JSON columns are TEXT checked with json_valid; STRICT tables enforce column types.

CREATE TABLE crew_member (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    role        TEXT NOT NULL,
    mission     TEXT NOT NULL,
    active      INTEGER NOT NULL CHECK (active IN (0, 1)),
    photo_hash  TEXT
) STRICT;

CREATE TABLE user (
    id              TEXT PRIMARY KEY,
    username        TEXT NOT NULL UNIQUE,
    role            TEXT NOT NULL CHECK (role IN ('astronaut', 'cmo', 'flight_surgeon', 'bme',
                                                  'mcc', 'administrator', 'device_gateway')),
    crew_member_id  TEXT REFERENCES crew_member (id),
    credential      TEXT NOT NULL,          -- argon2 PHC string; never a plain secret
    key_id          TEXT,
    disabled        INTEGER NOT NULL DEFAULT 0 CHECK (disabled IN (0, 1))
) STRICT;

CREATE TABLE device (
    id               TEXT PRIMARY KEY,
    adapter          TEXT NOT NULL,
    model            TEXT NOT NULL,
    vendor           TEXT NOT NULL,
    serial           TEXT NOT NULL,
    firmware         TEXT,
    accuracy_class   TEXT,
    calibration_due  INTEGER,
    crew_member_id   TEXT REFERENCES crew_member (id),
    status           TEXT NOT NULL,
    last_seen_tai    INTEGER,
    clock_offset_ms  REAL,
    UNIQUE (vendor, model, serial)
) STRICT;

CREATE TABLE observation (
    id                 TEXT PRIMARY KEY,
    crew_member_id     TEXT NOT NULL REFERENCES crew_member (id),
    device_id          TEXT REFERENCES device (id),
    parameter          TEXT NOT NULL,
    value              REAL,
    value_text         TEXT,                -- enumerated parameters
    unit               TEXT NOT NULL,
    measured_at_tai    INTEGER NOT NULL,
    received_at_tai    INTEGER NOT NULL,
    quality            TEXT NOT NULL CHECK (quality IN ('good', 'degraded', 'poor', 'unusable')),
    classification     TEXT NOT NULL CHECK (classification IN ('measured', 'derived', 'entered',
                                                               'imported', 'simulated')),
    measurement_mode   TEXT NOT NULL CHECK (measurement_mode IN ('continuous', 'spot')),
    algorithm_version  TEXT,
    context            TEXT CHECK (context IS NULL OR json_valid(context)),
    CHECK ((value IS NULL) <> (value_text IS NULL))
) STRICT;
CREATE INDEX observation_by_crew_parameter_time
    ON observation (crew_member_id, parameter, measured_at_tai);

CREATE TABLE waveform_chunk (
    id              TEXT PRIMARY KEY,
    crew_member_id  TEXT NOT NULL REFERENCES crew_member (id),
    device_id       TEXT REFERENCES device (id),
    signal          TEXT NOT NULL,
    start_tai       INTEGER NOT NULL,
    sample_rate     REAL NOT NULL CHECK (sample_rate > 0),
    encoding        TEXT NOT NULL,
    data            BLOB NOT NULL,
    quality         TEXT NOT NULL CHECK (quality IN ('good', 'degraded', 'poor', 'unusable'))
) STRICT;
CREATE INDEX waveform_by_crew_signal_time ON waveform_chunk (crew_member_id, signal, start_tai);

CREATE TABLE baseline (
    crew_member_id  TEXT NOT NULL REFERENCES crew_member (id),
    parameter       TEXT NOT NULL,
    median          REAL NOT NULL,
    mad             REAL NOT NULL,
    n               INTEGER NOT NULL,
    window          TEXT NOT NULL,
    status          TEXT NOT NULL,
    method_version  TEXT NOT NULL,
    PRIMARY KEY (crew_member_id, parameter)
) STRICT;

CREATE TABLE alarm (
    id                TEXT PRIMARY KEY,
    crew_member_id    TEXT REFERENCES crew_member (id),
    kind              TEXT NOT NULL CHECK (kind IN ('physiological', 'technical')),
    category          TEXT NOT NULL CHECK (category IN ('CARDIO', 'RESPIRATORY', 'THERMAL',
                                                        'ENVIRONMENT', 'TECHNICAL')),
    rule_id           TEXT NOT NULL,
    ruleset_version   TEXT NOT NULL,
    priority          TEXT NOT NULL CHECK (priority IN ('critical', 'warning', 'observe',
                                                        'advisory')),
    state             TEXT NOT NULL,
    confirming_readings TEXT CHECK (confirming_readings IS NULL OR json_valid(confirming_readings)),
    latched           INTEGER NOT NULL CHECK (latched IN (0, 1)),
    trigger_value     REAL,
    why               TEXT NOT NULL CHECK (json_valid(why)),
    action            TEXT,
    created_at_tai    INTEGER NOT NULL,
    acknowledged_by   TEXT REFERENCES user (id),
    acknowledged_at_tai INTEGER,
    silenced_until_tai INTEGER,
    resolved_at_tai   INTEGER,
    escalation_stage  INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE alarm_notification (
    alarm_id      TEXT NOT NULL REFERENCES alarm (id),
    audience      TEXT NOT NULL,
    stage         INTEGER NOT NULL,
    at_tai        INTEGER NOT NULL,
    delivered_to  TEXT,
    PRIMARY KEY (alarm_id, stage)
) STRICT;

CREATE TABLE encounter (
    id              TEXT PRIMARY KEY,
    crew_member_id  TEXT NOT NULL REFERENCES crew_member (id),
    author          TEXT NOT NULL REFERENCES user (id),
    subjective      TEXT,
    objective       TEXT,
    assessment      TEXT,
    plan            TEXT,
    signed_at_tai   INTEGER,
    attachments     TEXT CHECK (attachments IS NULL OR json_valid(attachments))
) STRICT;

CREATE TABLE attachment (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('dicom', 'ecg', 'file')),
    content_hash  TEXT NOT NULL,
    metadata      TEXT NOT NULL CHECK (json_valid(metadata))
) STRICT;

CREATE TABLE checklist_run (
    id                TEXT PRIMARY KEY,
    protocol_id       TEXT NOT NULL,
    protocol_version  TEXT NOT NULL,
    alarm_id          TEXT REFERENCES alarm (id),
    steps             TEXT NOT NULL CHECK (json_valid(steps)),
    started_at_tai    INTEGER NOT NULL,
    completed_at_tai  INTEGER
) STRICT;

CREATE TABLE checkin (
    id              TEXT PRIMARY KEY,
    crew_member_id  TEXT NOT NULL REFERENCES crew_member (id),
    answers         TEXT NOT NULL CHECK (json_valid(answers)),
    privacy_tier    TEXT NOT NULL CHECK (privacy_tier IN ('private', 'medical', 'status')),
    at_tai          INTEGER NOT NULL
) STRICT;

CREATE TABLE model_output (
    id              TEXT PRIMARY KEY,
    model_id        TEXT NOT NULL,
    model_hash      TEXT NOT NULL,
    crew_member_id  TEXT NOT NULL REFERENCES crew_member (id),
    output          TEXT NOT NULL CHECK (json_valid(output)),
    explanation     TEXT NOT NULL CHECK (json_valid(explanation)),
    at_tai          INTEGER NOT NULL
) STRICT;

CREATE TABLE downlink_item (
    id              TEXT PRIMARY KEY,
    seq             INTEGER NOT NULL UNIQUE,
    channel         TEXT NOT NULL CHECK (channel IN ('OPS', 'MEDICAL', 'RESEARCH')),
    priority        TEXT NOT NULL CHECK (priority IN ('CRITICAL', 'ROUTINE', 'RESEARCH')),
    payload         BLOB NOT NULL,          -- signed
    status          TEXT NOT NULL,
    created_at_tai  INTEGER NOT NULL,
    sent_at_tai     INTEGER,
    ack_at_tai      INTEGER
) STRICT;

CREATE TABLE uplink_item (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    signature       TEXT NOT NULL,
    verified_by     TEXT,
    applied_at_tai  INTEGER
) STRICT;

CREATE TABLE audit_entry (
    seq        INTEGER PRIMARY KEY,         -- chain order
    id         TEXT NOT NULL UNIQUE,
    at_tai     INTEGER NOT NULL,
    actor      TEXT NOT NULL,
    role       TEXT NOT NULL,
    action     TEXT NOT NULL,
    target     TEXT,
    details    TEXT NOT NULL CHECK (json_valid(details)),
    prev_hash  TEXT NOT NULL,
    hash       TEXT NOT NULL,
    signature  TEXT NOT NULL
) STRICT;

-- The audit log is append-only (docs/03 §4: never deleted; R-SEC-03).
CREATE TRIGGER audit_entry_no_update BEFORE UPDATE ON audit_entry
BEGIN SELECT RAISE(ABORT, 'audit log is append-only'); END;
CREATE TRIGGER audit_entry_no_delete BEFORE DELETE ON audit_entry
BEGIN SELECT RAISE(ABORT, 'audit log is append-only'); END;

CREATE TABLE config_bundle (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    version         TEXT NOT NULL,
    signature       TEXT NOT NULL,
    applied_at_tai  INTEGER
) STRICT;

CREATE TABLE mds_outbox (
    id              TEXT PRIMARY KEY,
    resource_type   TEXT NOT NULL,
    client_id       TEXT NOT NULL UNIQUE,   -- idempotency key
    payload         TEXT NOT NULL CHECK (json_valid(payload)),
    state           TEXT NOT NULL CHECK (state IN ('pending', 'sent', 'acked', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT,
    created_at_tai  INTEGER NOT NULL,
    acked_at_tai    INTEGER
) STRICT;

CREATE TABLE mds_inbox (
    id              TEXT PRIMARY KEY,
    source_id       TEXT NOT NULL,
    resource_type   TEXT NOT NULL,
    received_at_tai INTEGER NOT NULL,
    ingested        INTEGER NOT NULL DEFAULT 0 CHECK (ingested IN (0, 1)),
    dedup_key       TEXT NOT NULL UNIQUE    -- SR-016
) STRICT;
