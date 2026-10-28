---
cargo/devlanes: minor
cargo/devlanes-cli: minor
cargo/devlanes-config: minor
cargo/devlanes-daemon: minor
cargo/devlanes-proto: minor
cargo/devlanes-resolve: minor
cargo/devlanes-state: minor
---

Changed projects to be identified by their `project:` name, so one repository can hold several projects: a `devlanes.yaml` in a subdirectory, or another file such as `devlanes.prod.yaml` named once with the new `--file` flag and then with `-p`/`--project` (which also reads `DEVLANES_PROJECT`).

Renaming `project:` now makes a new project; the old one's lanes stay until `devlanes lane gc --yes` cleans them. Another repository can no longer use a name already taken. In a project file below the worktree root, relative `cwd`, `run_script.file` and `sync` paths now start at that file's directory. `lanes.max` comes from the primary worktree's file, and a worktree whose file sets it, `lanes.primary` or shared infra differently gets a warning. See [several projects in one repository](https://github.com/devlanes-run/devlanes/blob/main/docs/guides/services.md).
