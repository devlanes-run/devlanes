---
title: Run scripts your project already has
description: Start a service or a task from a script in package.json, a Makefile, a justfile, a Taskfile, Cargo.toml, pyproject.toml, deno.json or go.mod, and know when a plain command fits better.
kind: how-to
---

Use this guide when the commands your project runs are already scripts, targets, recipes or tasks in one of its own files, so the command stays written in one place.

## Point the service at the script

Name the file, relative to the project file, and the script in it. The script runs in that file's folder, the way it does from a terminal there. Tasks take the same two keys.

```yaml devlanes
services:
  api:
    run_script:
      file: apps/api/package.json
      name: dev
```

The file name picks the tool:

- `package.json`: the script, with the package manager the project declares in `packageManager` or its lockfile.
- A Makefile: `make <target>`.
- A justfile: `just <recipe>`.
- A Taskfile, such as `Taskfile.yml`: `task <name>`. The task gets a preset's flags in `{{.CLI_ARGS}}`.
- `Cargo.toml`: `cargo run --bin <name>`, where the name is a binary.
- `pyproject.toml`: an entry under `[project.scripts]`, run with uv or Poetry.
- `deno.json`: `deno task <name>`.
- `go.mod`: `go run ./cmd/<name>`, where the name is a folder under `cmd`. `.` or a name with a `/`, such as `tools/seed`, is a package path from the go.mod folder.

A start checks that the script exists and lists the names the file has when it does not.

## Use a plain command for the rest

A command that is not a script in one of these files goes in `run`, written as you would type it, with an expression where it needs the port.

```yaml devlanes
services:
  api:
    run: uv run uvicorn acme.main:app --host 127.0.0.1 --port ${{ self.port }} --reload
```

## If a profile should run another script

A profile, a named choice of services for a start, can swap the script name for the services that use it, such as a start script in place of the dev server. [Share settings and choose which services start](templates-and-profiles.md) shows it.

## If only you need a different script

Your local file, the `devlanes.local.yaml` that stays out of Git, can change the script name and keep the file. [Keep personal changes out of Git](local-overrides.md) covers it.

## Related

- [Add devlanes to a project](add-devlanes.md)
- [Run Node, Bun, Deno, Python, Rust and Go services](runtimes.md)
- [Run Vite and Next.js](vite-and-next.md)
- [`services.<name>.run_script`](/docs/reference/config/project/services/#services-name-run_script)
