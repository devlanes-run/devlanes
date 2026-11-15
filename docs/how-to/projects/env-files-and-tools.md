---
title: Read .env values and pin tool versions
description: Hand values from a gitignored .env file to the services that need them, and give every lane the tool versions mise or direnv pin.
kind: how-to
---

Use this guide when a project keeps secrets or machine-specific values in a .env file, or pins its Node, Python or Rust versions with mise or direnv, and the lanes should use both.

::example{slug="workflows-dotenv"}

## Keep the values in .env

The file sits next to the project file and stays out of Git. A lane whose worktree has none, such as a new one, reads the primary worktree's, as [Files and folders](../../reference/files.md#files-of-a-project) describes, so one file serves every lane of the project on this machine.

```text title=".env"
ACME_API_TOKEN=acme_dev_7f3a9c2e51
SENTRY_DSN=
```

## Pass a value on to a service

devlanes reads the file for expressions and does not put its contents into any process, which [Environment and expressions](../../concepts/environment.md#a-env-file-feeds-expressions-not-processes) explains. Name each variable a process needs and read its value with an expression. Under `global_env` it reaches every service and task of the lane, and a service's own `env` does the same for that service alone.

```yaml devlanes
global_env:
  ACME_API_TOKEN: "${{ env.ACME_API_TOKEN }}"
  SENTRY_DSN: "${{ env.SENTRY_DSN | default('') }}"
```

A variable that neither the file nor your shell has stops the start, unless the expression gives a default as the second line does. The same name in the shell that runs the start wins over the file, and an edit to the file shows in `devlanes status` until `devlanes reload` applies it.

## Pin tool versions with mise

With a mise config in the worktree root, such as `mise.toml` or `.tool-versions`, devlanes asks mise for its environment when it starts the lane. Each service and task then finds the pinned tools first on its path and gets the variables the config sets.

```toml title="mise.toml"
[tools]
node = "24"

[env]
API_MODE = "local"
```

A config mise does not trust yet is skipped, and the start goes on with a note that tells you to run `mise trust` in that folder.

## Load an .envrc with direnv

An `.envrc` in the worktree root works the same way, through direnv. Variables it sets are added, and variables it unsets are removed from what the services and tasks get. A blocked file is skipped with a note, and devlanes does not allow it for you:

```console title="~/dev/myapp"
$ direnv allow
```

Both tools look only at the worktree root, not at a service's own folder. When the tool is not installed, the note says so and the start goes on, while any other error from the tool fails the service with the tool's message. The tools cannot change a service's port, and `global_env` or a service's `env` override them, as [Environment and expressions](../../concepts/environment.md#layers-stack-from-the-shell-to-the-command-line) explains.

## Apply a trust or an allow

devlanes asks mise and direnv again each time it starts a service, and does not watch their files, so a trust or an allow changes nothing that runs and `devlanes status` shows no change. Restart the services with `devlanes restart`, or name the ones that need the new values.

To read the notes, run `devlanes env show --explain` with the service's name; they follow its variables.

## If the app loads .env itself

A service that loads `.env` itself keeps working. Most dotenv loaders leave a variable alone when it is already set, so a value the project file passes wins over the same name in the file.

## If a container needs a value

devlanes hands Compose the `.env` next to the project file for the variables in the Compose file, and values under [`infra.env`](/docs/reference/config/project/infra/) go to Compose the same way. Neither reaches a service or task. [Share one database server between lanes](../infra/shared-database.md) uses that.

## Related

- [Keep personal changes out of Git](local-overrides.md)
- [Run Node, Bun, Deno, Python and Rust services](runtimes.md)
- [Environment variables](../../reference/environment-variables.md)
- [Expressions](../../reference/expressions.md)
