-- M8 clinical records (CR-019): encounter vitals snapshot and creation time, checklist runs by
-- crew member and starter, the medication and medical-kit log (F-CLN-08), attachment owner.
ALTER TABLE encounter ADD COLUMN vitals TEXT CHECK (vitals IS NULL OR json_valid(vitals));
ALTER TABLE encounter ADD COLUMN created_at_tai INTEGER;
ALTER TABLE checklist_run ADD COLUMN crew_member_id TEXT REFERENCES crew_member (id);
ALTER TABLE checklist_run ADD COLUMN started_by TEXT REFERENCES user (id);
ALTER TABLE attachment ADD COLUMN crew_member_id TEXT REFERENCES crew_member (id);
ALTER TABLE attachment ADD COLUMN filename TEXT;
ALTER TABLE attachment ADD COLUMN created_at_tai INTEGER;
CREATE TABLE medication_entry (
    id              TEXT PRIMARY KEY,
    crew_member_id  TEXT REFERENCES crew_member (id),
    kind            TEXT NOT NULL CHECK (kind IN ('dose', 'kit_use', 'kit_restock')),
    item            TEXT NOT NULL,
    amount          REAL,
    unit            TEXT,
    note            TEXT,
    by_user         TEXT NOT NULL REFERENCES user (id),
    at_tai          INTEGER NOT NULL,
    CHECK ((kind = 'dose') = (crew_member_id IS NOT NULL))
) STRICT;
