-- Projects are identified by name: one repository may hold several projects and
-- a rename makes a new one. Of the rows sharing a name, the most recently
-- started keeps it; the others become `<name>-old<id>`, still a valid project
-- name, so they show as separate projects that `lane gc` can collect. A project
-- that already has that name makes the new one take a random suffix too, since a
-- collision here would stop the upgrade and the daemon with it.
CREATE TEMP TABLE renamed AS
SELECT id, name || '-old' || id AS wanted FROM (
    SELECT id, name, ROW_NUMBER() OVER (
        PARTITION BY name ORDER BY last_start IS NULL, last_start DESC, id DESC
    ) AS rank
    FROM projects
)
WHERE rank > 1;
UPDATE renamed SET wanted = wanted || '-' || lower(hex(randomblob(4)))
WHERE wanted IN (SELECT name FROM projects);
UPDATE projects SET name = (SELECT wanted FROM renamed WHERE renamed.id = projects.id)
WHERE id IN (SELECT id FROM renamed);
DROP TABLE renamed;

-- The UNIQUE identity column now holds the name, which avoids rebuilding a table
-- that four others reference with ON DELETE CASCADE. The integer pass clears the
-- old path identities first, because an integer never equals a blob, so no row
-- can collide with another row's old identity midway.
UPDATE projects SET identity = -id;
UPDATE projects SET identity = CAST(name AS BLOB);
DROP INDEX projects_name;
CREATE UNIQUE INDEX projects_name ON projects(name);

-- File name of the project's config inside config_dir; NULL means devlanes.yaml.
ALTER TABLE projects ADD COLUMN config_file TEXT;

-- Hash of the entry's declaration when it last completed, so an edited entry runs
-- again. Rows from earlier daemons keep NULL, which counts as matching, so an
-- upgrade does not rerun every setup entry and task.
ALTER TABLE lane_setup ADD COLUMN spec_hash TEXT;
ALTER TABLE task_completions ADD COLUMN spec_hash TEXT;
