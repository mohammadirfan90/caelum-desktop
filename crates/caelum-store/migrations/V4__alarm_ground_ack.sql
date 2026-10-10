-- ROLE_MATRIX.md §5.4: the ground flight surgeon acknowledges only the ground escalation stage
-- ("seen by ground"); it never acknowledges or silences the alarm onboard (R-ALM-03, SR-007).
ALTER TABLE alarm ADD COLUMN ground_acknowledged_by TEXT REFERENCES user (id);
ALTER TABLE alarm ADD COLUMN ground_acknowledged_at_tai INTEGER;
