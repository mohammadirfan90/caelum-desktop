-- Replication of the station record to the standby (F-SYS-04, R-OPS-02; CR-017,
-- docs/integration/M11.md item 1). While capture is on (the primary of a configured pair), every
-- insert or update of a replicated table records the row's key in cluster_outbox. The node
-- driver turns each into a signed log entry carrying the whole row, and the standby applies it.
-- Rows are never deleted from these tables (records are append-only or kept inactive).
-- Configuration bundles of kind `cluster` are per node and are not replicated. The primary's
-- audit entries are kept on the standby in audit_replica, apart from the standby's own chain.
CREATE TABLE cluster_outbox (
    seq   INTEGER PRIMARY KEY AUTOINCREMENT,
    tbl   TEXT NOT NULL,
    key   TEXT NOT NULL CHECK (json_valid(key))
) STRICT;
CREATE TABLE audit_replica (
    origin      TEXT NOT NULL,
    seq         INTEGER NOT NULL,
    id          TEXT NOT NULL,
    at_tai      INTEGER NOT NULL,
    actor       TEXT NOT NULL,
    role        TEXT,
    action      TEXT NOT NULL,
    target      TEXT,
    details     TEXT NOT NULL,
    prev_hash   TEXT NOT NULL,
    hash        TEXT NOT NULL,
    signature   TEXT NOT NULL,
    PRIMARY KEY (origin, seq)
) STRICT;
CREATE TRIGGER cluster_capture_crew_member_i AFTER INSERT ON "crew_member"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('crew_member', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_crew_member_u AFTER UPDATE ON "crew_member"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('crew_member', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_user_i AFTER INSERT ON "user"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('user', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_user_u AFTER UPDATE ON "user"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('user', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_device_i AFTER INSERT ON "device"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('device', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_device_u AFTER UPDATE ON "device"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('device', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_config_bundle_i AFTER INSERT ON "config_bundle"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on' AND NEW.kind <> 'cluster'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('config_bundle', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_config_bundle_u AFTER UPDATE ON "config_bundle"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on' AND NEW.kind <> 'cluster'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('config_bundle', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_observation_i AFTER INSERT ON "observation"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('observation', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_observation_u AFTER UPDATE ON "observation"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('observation', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_waveform_chunk_i AFTER INSERT ON "waveform_chunk"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('waveform_chunk', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_waveform_chunk_u AFTER UPDATE ON "waveform_chunk"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('waveform_chunk', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_baseline_i AFTER INSERT ON "baseline"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('baseline', json_array(NEW.crew_member_id, NEW.parameter)); END;
CREATE TRIGGER cluster_capture_baseline_u AFTER UPDATE ON "baseline"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('baseline', json_array(NEW.crew_member_id, NEW.parameter)); END;
CREATE TRIGGER cluster_capture_alarm_i AFTER INSERT ON "alarm"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('alarm', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_alarm_u AFTER UPDATE ON "alarm"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('alarm', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_alarm_notification_i AFTER INSERT ON "alarm_notification"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('alarm_notification', json_array(NEW.alarm_id, NEW.stage)); END;
CREATE TRIGGER cluster_capture_alarm_notification_u AFTER UPDATE ON "alarm_notification"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('alarm_notification', json_array(NEW.alarm_id, NEW.stage)); END;
CREATE TRIGGER cluster_capture_encounter_i AFTER INSERT ON "encounter"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('encounter', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_encounter_u AFTER UPDATE ON "encounter"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('encounter', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_attachment_i AFTER INSERT ON "attachment"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('attachment', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_attachment_u AFTER UPDATE ON "attachment"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('attachment', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_checklist_run_i AFTER INSERT ON "checklist_run"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('checklist_run', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_checklist_run_u AFTER UPDATE ON "checklist_run"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('checklist_run', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_checkin_i AFTER INSERT ON "checkin"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('checkin', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_checkin_u AFTER UPDATE ON "checkin"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('checkin', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_medication_entry_i AFTER INSERT ON "medication_entry"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('medication_entry', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_medication_entry_u AFTER UPDATE ON "medication_entry"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('medication_entry', json_array(NEW.id)); END;
CREATE TRIGGER cluster_capture_audit_entry_i AFTER INSERT ON "audit_entry"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('audit_entry', json_array(NEW.seq)); END;
CREATE TRIGGER cluster_capture_audit_entry_u AFTER UPDATE ON "audit_entry"
WHEN (SELECT value FROM cluster_state WHERE key = 'capture') = 'on'
BEGIN INSERT INTO cluster_outbox (tbl, key) VALUES ('audit_entry', json_array(NEW.seq)); END;
