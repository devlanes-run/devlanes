-- Whether a stop asked for the run's end: 1 when it did, 0 when the run ended
-- on its own. Rows from earlier daemons stay NULL, since nothing recorded it.
ALTER TABLE unit_runs ADD COLUMN ended_by_request INTEGER;
