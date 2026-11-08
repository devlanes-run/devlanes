-- A clean now deletes the lane row, so a lane that comes back is a new lane with
-- a new id, data folder and history. Rows earlier daemons kept as cleaned go,
-- with their run history, setup records and task history by cascade.
DELETE FROM lanes WHERE cleaned_at IS NOT NULL;
-- '#lane=' is the daemon's ALIAS_MARK: extra lanes (`lane new --name`) whose lane
-- row is gone. An extra lane's worktree row now exists only with its lane or
-- while it waits for one.
DELETE FROM worktrees
WHERE instr(identity, '#lane=') > 0
  AND id NOT IN (SELECT worktree_id FROM lanes);
DROP INDEX lanes_active_index;
CREATE UNIQUE INDEX lanes_index ON lanes(project_id, lane_index);
ALTER TABLE lanes DROP COLUMN cleaned_at;
-- Lane data folders a clean could not delete (a file held open on Windows),
-- which `lane gc` and the state maintenance pass delete later.
CREATE TABLE data_deletes (
    path BLOB PRIMARY KEY NOT NULL,
    since BIGINT NOT NULL
);
