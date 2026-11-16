---
title: Read .env values and pin tool versions
description: Hand values from a gitignored .env file to the services that need them, and give every lane the tool versions mise or direnv pin.
kind: how-to
---

Use this guide when a project keeps secrets or machine-specific values in a .env file, or pins its tool versions with mise or direnv, and the lanes should use both.

::example{slug="workflows-dotenv"}

## Pass .env values to services

The `.env` file sits next to the project file and stays out of Git. devlanes reads it for expressions and does not put its contents into any process, so name each variable a process needs:

```yaml devlanes
global_env:
  ACME_API_TOKEN: "${{ env.ACME_API_TOKEN }}"
  SENTRY_DSN: "${{ env.SENTRY_DSN | default('') }}"
```

Under `global_env` a value reaches every service and task of the lane. A variable missing from both the file and your shell stops the start, unless the expression gives a default as the second line does. A new worktree reads the primary worktree's `.env`, as [Files and folders](../../reference/files.md#files-of-a-project) describes.

## Apply an edit

```console title="~/dev/myapp"
$ devlanes reload
```

The reload restarts the services whose values changed.

## Pin tool versions with mise or direnv

With a mise config such as `mise.toml`, or an allowed `.envrc`, in the worktree root, each service and task gets the tools and variables it sets. A config the tool does not trust yet is skipped with a note, so trust it and restart the services:

```console title="~/dev/myapp"
$ mise trust
$ devlanes restart
```

For direnv, run `direnv allow` in place of `mise trust`.

## If the app loads .env itself

A service that loads `.env` itself keeps working. Most dotenv loaders leave a variable alone when it is already set, so the value the project file passes wins.

## Related

- [Environment and expressions](../../concepts/environment.md)
- [Keep personal changes out of Git](local-overrides.md)
- [Run Node, Bun, Deno, Python and Rust services](runtimes.md)
- [Expressions](../../reference/expressions.md)
