ALTER TABLE task_completions DROP COLUMN spec_hash;
ALTER TABLE lane_setup DROP COLUMN spec_hash;
ALTER TABLE projects DROP COLUMN config_file;
DROP INDEX projects_name;
CREATE INDEX projects_name ON projects(name);

-- Earlier daemons key projects by repository, so only the oldest project of each
-- repository survives; the cascade removes the other projects' lanes and ports.
DELETE FROM projects WHERE id NOT IN (
    SELECT MIN(id) FROM projects GROUP BY COALESCE(common_dir, root_path)
);
-- Same integer pass as the up script, so no row collides with another's name.
UPDATE projects SET identity = -id;
UPDATE projects SET identity = COALESCE(common_dir, root_path);
