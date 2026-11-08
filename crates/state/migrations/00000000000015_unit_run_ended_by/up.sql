-- What ended the run: 'requested' (a stop asked for it), 'timed_out' (its own
-- time limit stopped it) or 'itself' (it ended with no stop). NULL when nobody
-- knows: rows from earlier daemons, runs an earlier boot left unfinished and
-- orphans that ended while no daemon watched.
ALTER TABLE unit_runs ADD COLUMN ended_by TEXT;
