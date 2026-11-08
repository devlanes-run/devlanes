-- How a task run ended, as the daemon names a unit's end (`exited`, `failed`,
-- `stopped`); NULL while it runs. Before this column only the exit code was
-- kept, so older rows get the word that code implies: 0 exited, another code
-- failed, none stopped (a signal, a stop or an abandoned run). A daemon-side
-- failure also left no code, so those rows read stopped too.
ALTER TABLE task_runs ADD COLUMN state TEXT;
UPDATE task_runs SET state = CASE
    WHEN exit_code = 0 THEN 'exited'
    WHEN exit_code IS NOT NULL THEN 'failed'
    ELSE 'stopped'
END
WHERE ended_at IS NOT NULL;
