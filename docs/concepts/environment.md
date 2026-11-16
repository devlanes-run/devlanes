---
title: Environment and expressions
description: A unit's variables are layered from the shell that started it, from tools, from devlanes and from the project file, the lane keeps that shell's environment in memory only, and values in the project file are evaluated for each lane.
kind: concept
---

Each time a service or task of a lane starts, devlanes puts its environment together. It begins with the shell you ran devlanes from, then adds what tool managers and devlanes set, then what the project file and the command line set. Values in the project file can be expressions, worked out for each lane, so one file gives each lane its own ports, hostnames and database names.

## Layers stack from the shell to the command line

A unit, a service or task of a lane for example ([Glossary](../reference/glossary.md)), gets seven layers. When two set the same name, the higher one wins. From the bottom up:

1. client: your shell's environment, which the lane keeps as described below. Services and tasks from the project file get it without the variables coding agents set to identify themselves, such as `CLAUDECODE` and `AI_AGENT`, so they run the same whether you or an agent started the lane.
2. contributor: what mise and direnv add, the variables that point Node, Python and the other runtimes at the devlanes CA, and a preset's settings for a dev server such as Vite.
3. devlanes: devlanes' own variables, such as `PORT` and `HOST`.
4. global: `global_env` in the project file, for every unit of the lane.
5. service: the unit's own env.
6. profile: the env of the profiles, named selections of services, that the start selected.
7. override: values given with `--env` on the command line.

Because devlanes' own variables sit above the shell, a `PORT` left in your shell cannot move a service. `global_env` sits above them, so do not set the port there: every service would get the same one. To see where each variable of a unit came from:

```console
$ devlanes env show api --explain
```

[Environment variables](../reference/environment-variables.md) lists what devlanes sets.

## The lane keeps the shell's environment in memory

The lane keeps the environment of the last shell that started, restarted, ran or executed something in it, and uses it for what it starts next. A `devlanes reload` reuses it, while a start or restart from another shell replaces it. After a change in your shell, such as a new `PATH`, restart from that shell so the lane picks it up.

The daemon writes that environment to no file and no log, since it can hold tokens and other secrets. So it does not outlive the daemon: after a reboot each lane waits for one start from a shell ([The daemon](daemon.md)). It is still not hidden from your own programs, since any process running as you can ask the daemon for it ([Security](security.md)).

## Expressions are evaluated for each lane

A value in the project file can hold an expression written as `${{ }}`, which reads the lane: its number and domain, its services' ports and URLs, its containers and the shell's variables.

```yaml devlanes
services:
  worker:
    autostart: ${{ lane.index == 0 }}
  web:
    env:
      VITE_API_URL: "${{ services.api.public_url }}"
```

A value that is one expression keeps its type, so the worker's setting is true or false. A name that does not exist is an error rather than an empty string, and `default()` makes a value optional. [Expressions](../reference/expressions.md) lists what an expression can read.

## A .env file feeds expressions, not processes

An expression such as `${{ env.STRIPE_KEY }}` takes the variable from your shell, or from the `.env` file next to the project file when the shell has none. devlanes does not pass the rest of `.env` to a process: you name each value a process needs, so everything a process gets from devlanes is in the project file.

```yaml devlanes
global_env:
  STRIPE_KEY: ${{ env.STRIPE_KEY }}
  SENTRY_DSN: ${{ env.SENTRY_DSN | default('') }}
```

[Read .env values and pin tool versions](../how-to/projects/env-files-and-tools.md) sets this up.

## Related

- [Services that find each other](../learn/connecting-services.md)
- [Expressions](../reference/expressions.md)
- [Environment variables](../reference/environment-variables.md)
- [Config files and reload](config-and-reload.md)
