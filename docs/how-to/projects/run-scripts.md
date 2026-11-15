---
title: Run scripts your project already has
description: Start a service or a task from a script in package.json, a Makefile, a justfile, Cargo.toml, pyproject.toml or deno.json, and know when a plain command fits better.
kind: how-to
---

Use this guide when the commands your project runs are already scripts, targets, recipes or tasks in one of its own files. devlanes then builds the command with the project's tool, and the file stays the one place the command is written.

## Point the service at the script

Name the file, relative to the project file, and the script in it. The command runs in that file's folder, so a script that works from a terminal there works the same under devlanes.

```yaml devlanes
services:
  api:
    run_script:
      file: apps/api/package.json
      name: dev
```

A start looks the name up in the file first, and a misspelt name fails with the list of names the file has. A Makefile or justfile the check cannot read through, such as one that includes others, is left to make or just at run time. Tasks take the same two keys.

## Match the file kind

The file name picks the tool, and a file of another name fails the start.

::::tabs
:::tab[package.json]
devlanes runs `<manager> run <name>` with the package manager the project declares, read from its `packageManager` field and otherwise from its lockfile. Without either it falls back to npm, so add the field when you use pnpm, yarn or bun.
:::
:::tab[Makefile]
devlanes runs `make <target>` in the Makefile's folder, and picks the file with a flag when make would read another one.

make cannot pass extra arguments to a target, so a service with a preset cannot use one. Put that command in a package.json script or a justfile recipe instead.
:::
:::tab[justfile]
devlanes runs `just <recipe>`. A preset adds its flags after the recipe name, so the recipe needs a last parameter that takes any number of arguments:

```just
dev *ARGS:
    vite {{ARGS}}
```

A recipe with required parameters fails, since devlanes has nothing to fill them with. Give them defaults.
:::
:::tab[Cargo.toml]
The name is a binary, and devlanes runs `cargo run --bin <name>`. At a workspace root it finds the member that builds that binary and adds `-p` with its name.

There is no way to add `--release` or features here. Use a plain command for those.
:::
:::tab[pyproject.toml]
The name is an entry under `[project.scripts]` or Poetry's scripts table. devlanes runs it with uv when the project has a uv lockfile or uv settings, and with Poetry when it has a Poetry lockfile.

With neither, the start fails and suggests `uv lock` or `poetry lock`. uv installs these entries only for a packaged project, one with a build system. A dev server started by a tool, such as uvicorn, is a plain command.
:::
:::tab[deno.json]
devlanes runs `deno task <name>`, and `deno.jsonc` works the same way. Comments and trailing commas are fine in both.

Point the service at the Deno file even when a `package.json` sits next to it, because that one always goes to the package manager.
:::
::::

## Use a plain command for the rest

A command that is not a script in one of these files goes in `run`. That covers a tool called with its own flags, a pipeline, a script of several lines, a release build and a file no runner reads, such as a requirements file or a Taskfile.

```yaml devlanes
services:
  api:
    run: uv run uvicorn acme.main:app --host 127.0.0.1 --port ${{ self.port }} --reload
```

devlanes runs that text as written and refuses a preset next to it, so write the preset's flags into the command yourself. [Run Vite and Next.js](vite-and-next.md) lists the flags each preset adds.

## If a profile should run another script

A profile, a named choice of services for a start, can swap the script name for every service that uses it, such as the build's start script in place of the dev server. [Share settings and choose which services start](templates-and-profiles.md) shows it, and [Run dev servers and a production build side by side](dev-and-prod.md) uses it for a second project.

## If only you need a different script

Your local file can change the script name and keep the file from the project file. [Keep personal changes out of Git](local-overrides.md) covers the local file.

## Related

- [Add devlanes to a project](add-devlanes.md)
- [Run Node, Bun, Deno, Python and Rust services](runtimes.md)
- [`services.<name>.run_script`](/docs/reference/config/project/services/)
- [`tasks.<name>.run_script`](/docs/reference/config/project/tasks/)
