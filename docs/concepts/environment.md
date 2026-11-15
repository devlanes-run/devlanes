---
title: Environment and expressions
description: A unit's variables are layered from the shell that started it, from tools, from devlanes and from the project file, the lane keeps the last shell's environment in memory only, and values in the project file are evaluated for each lane.
kind: concept
---

Each time a service or task of a lane starts, devlanes puts its environment together. It begins with the shell you ran devlanes from, then gains what tool managers and devlanes add, then what the project file and the command line set. Values in the project file can be expressions, which devlanes works out for each lane, so one file gives each lane its own ports, hostnames and database names.

## Layers stack from the shell to the command line

A unit, a service, task or other process of a lane ([Glossary](../reference/glossary.md)), gets seven layers. When two set the same name, the higher one wins. From the bottom up, with the name `devlanes env show --explain` gives each:

1. client: the shell of the last start, restart, run, exec or resume in the lane.
2. contributor: what tool managers and the runtime add. That is mise and direnv, the variables that point Node, Python and the other runtimes at the devlanes CA, and a preset's settings, a preset being what devlanes knows about a dev server such as Vite.
3. devlanes: devlanes' own variables, such as `PORT` and `HOST`.
4. global: `global_env` in the project file, for every unit of the lane.
5. service: the unit's own env.
6. profile: the env of the profiles, named selections of services, that the start selected.
7. override: values given with `--env` on the command line.

![Seven layers of a unit's environment, from the shell that ran the start at the bottom to --env on the command line at the top, each with the name env show gives it; the higher layer wins](environment.svg)

Because devlanes' own variables sit above the shell, a stray `PORT` in your shell cannot move a service. `global_env` sits above them, so do not set the port there: every service would get the same one. [Environment variables](../reference/environment-variables.md) lists what devlanes sets, and on Windows names are uppercased as they come in ([Windows](../reference/windows.md)).

## Each variable shows the layer it came from

In this lane of myapp the shell gave the API its home folder, the Node runtime pointed it at the devlanes CA, and devlanes set its port.

```console title="on Windows 11"
$ devlanes env show api --explain
KEY                  VALUE                             LAYER
HOME                 C:\Users\you                      client
NODE_EXTRA_CA_CERTS  C:\Users\you\.devlanes\ca\ca.pem  contributor
PORT                 20060                             devlanes
```

A command run with `devlanes exec`, which the daemon runs as a unit of the lane, or with `devlanes with`, which starts the lane and runs your command in your terminal, gets every layer except what belongs to one unit: a unit's own env, and the port and host of a service.

## The lane keeps the last shell's environment

A lane holds one client environment at a time, and each of these commands decides what it is:

- `devlanes start` and `devlanes restart` send your shell's environment, and the lane keeps it for what it starts next.
- `devlanes run` and `devlanes exec` do the same.
- `devlanes with` does the same through the start it makes.
- `devlanes reload` sends none and restarts units with the one the lane holds.
- `devlanes lane resume` and `devlanes daemon restart` give your shell's environment to every lane they start.

So a `devlanes run migrate` from a second shell changes what the next reload of that lane uses. A variable you change in your shell reaches the lane only with one of the commands that send it.

## The shell's environment is never written to disk

The daemon keeps that environment in memory and writes it to no file and no log, since it can hold tokens and other secrets. The values are in no file of devlanes and do not outlive the daemon, which is why a lane waits for a start from a shell after a reboot ([The daemon](daemon.md)).

It does not hide them from your own programs: while a lane runs, any process running as you can ask the daemon for its environment ([Security](security.md)).

A daemon that a command starts keeps for itself only a short list of that shell's variables, so that tokens in the shell stay out of a process that serves every later one ([Environment variables](../reference/environment-variables.md)).

## Expressions are evaluated for each lane

A value in the project file can hold an expression written as `${{ }}`, which reads the lane: its number and domain, its services' ports and URLs, its containers, the shell's variables. [Expressions](../reference/expressions.md) lists what it can read.

```yaml devlanes
services:
  worker:
    autostart: ${{ lane.index == 0 }}
  web:
    env:
      VITE_API_URL: "${{ services.api.public_url }}"
```

A value that is one expression and nothing else keeps its type, so the worker's setting is true or false and not the text. A name that does not exist is an error, not an empty string, and `default()` makes a value optional. The file holds single expressions only, with no blocks such as loops.

devlanes evaluates the expressions for each lane when it plans a start, and a running unit keeps the values it started with. When you edit the file, or a value an expression reads, status reports drift, the difference between what runs and what the file now says, until a reload applies it ([Config files and reload](config-and-reload.md)). `devlanes config check` checks the syntax of expressions and the names they use. It cannot check their values, which exist only once a lane plans a start.

## A .env file feeds expressions, not processes

An expression such as `${{ env.STRIPE_KEY }}` takes the variable from the shell, and from the `.env` file next to the project file when the shell has none. It reads nothing else, so it does not see the port devlanes hands out. [Files and folders](../reference/files.md) says which worktree's file a lane reads.

devlanes does not put the contents of `.env` into a process. You name each value a process needs in the project file, so everything a process gets from devlanes is written there and shows in `devlanes env show`.

```yaml devlanes
global_env:
  STRIPE_KEY: ${{ env.STRIPE_KEY }}
  SENTRY_DSN: ${{ env.SENTRY_DSN | default('') }}
```

Without `STRIPE_KEY` in the shell or the file, the start fails; without `SENTRY_DSN`, the value is empty. A service that loads the file itself still can. [Read .env values and pin tool versions](../how-to/projects/env-files-and-tools.md) sets this up.

## Related

- [Services that find each other](../learn/connecting-services.md)
- [Expressions](../reference/expressions.md)
- [Environment variables](../reference/environment-variables.md)
- [Config files and reload](config-and-reload.md)
