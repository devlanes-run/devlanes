-- Deleted lane and worktree rows are not restored.
DROP TABLE data_deletes;
DROP INDEX lanes_index;
ALTER TABLE lanes ADD COLUMN cleaned_at BIGINT;
CREATE UNIQUE INDEX lanes_active_index ON lanes(project_id, lane_index) WHERE cleaned_at IS NULL;
