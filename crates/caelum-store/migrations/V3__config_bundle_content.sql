-- SR-009: keep the content of each verified configuration bundle (e.g. the alarm ruleset), so a
-- later start can fall back to the last verified one when a new bundle fails verification. The
-- content is re-verified against its signature before any use.
ALTER TABLE config_bundle ADD COLUMN content TEXT;
