-- CR-012 §4: alarm lists and history show each physiological alarm's title from the ruleset in
-- force when it was raised. Technical alarms have none (NULL); the UI names them by rule kind.
ALTER TABLE alarm ADD COLUMN title TEXT;
