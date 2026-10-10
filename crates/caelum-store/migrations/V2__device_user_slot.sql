-- CR-008 §4: the user slot a multi-user device (BPS cuff, WSS scale) uses for its assigned crew
-- member; readings from other slots are not filed under that crew member (F-ACQ-09).
ALTER TABLE device ADD COLUMN device_user INTEGER CHECK (device_user IS NULL OR device_user BETWEEN 0 AND 254);
