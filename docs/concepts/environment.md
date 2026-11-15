---
title: Environment and expressions
description: A unit's variables are layered from the shell that started it, from tools, from devlanes and from the project file, and values in the project file are evaluated for each lane.
kind: concept
---

A unit's environment is put together each time it starts. It begins with the shell that asked for the start, then gains what tool managers and devlanes add, then what the project file and the command line set. Values in the project file can be expressions, which devlanes works out for each lane, so one file gives each lane its own ports, hostnames and database names.

## Layers stack from the shell to the command line

When two layers set the same name, the higher one wins. From the bottom up, with the name `devlanes env show --explain` gives each layer:

1. client: the shell that sent the start, restart, run or resume.
2. contributor: what tool managers and the runtime add. That is mise and direnv, the variables that point Node, Python and the other runtimes at the devlanes CA, and a preset's settings.
3. devlanes: devlanes' own variables, such as `PORT` and `HOST`.
4. global: `global_env` in the project file, for every unit of the lane.
5. service: the unit's own env.
6. profile: the env of the profiles the start selected.
7. override: values given with `--env` on the command line.

![Seven layers of a unit's environment, from the shell that ran the start at the bottom to --env on the command line at the top, each with the name env show gives it; the higher layer wins](environment.svg)

Because devlanes' own variables sit above the shell, a stray `PORT` in your shell cannot move a service. And because `global_env` sits above them, setting the port there would give every service the same one. [Environment variables](../reference/environment-variables.md) lists what devlanes sets, and on Windows names are uppercased as they come in ([Windows](../reference/windows.md)).

## Each variable shows the layer it came from

In this lane of myapp the shell gave the API its home folder, the Node runtime pointed it at the devlanes CA, and devlanes set its port.

```console title="on Windows 11"
$ devlanes env show api --explain
KEY                  VALUE                             LAYER
HOME                 C:\Users\you                      client
NODE_EXTRA_CA_CERTS  C:\Users\you\.devlanes\ca\ca.pem  contributor
PORT                 20060                             devlanes
```

A command you run outside a unit, with `devlanes exec` or `devlanes with`, gets every layer except what belongs to one unit: a unit's own env, and the port and host of a service.

## The shell's environment is never written to disk

The daemon keeps the environment of the shell that started a lane in memory, and treats its values as secrets: it writes them to no file and no log. A `devlanes reload` reuses that environment, while a `devlanes restart` takes the one of the shell you run it in.

So the values are in no file of devlanes and do not outlive the daemon, which is why a lane waits for a start from a shell after a reboot ([The daemon](daemon.md)). They are not hidden from your own programs: while a lane runs, any process running as you can ask the daemon for its environment ([Security](security.md)).

A daemon that a command starts keeps only a short list of that shell's variables for itself, which [Environment variables](../reference/environment-variables.md) gives.

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

A value that is one expression and nothing else keeps its type, so the worker's setting is true or false and not the text. A name that does not exist is an error, not an empty string, and `default()` makes a value optional. The file holds expressions only, without loops or template blocks.

devlanes evaluates the expressions for each lane when it plans a start, and a running unit keeps the values it started with. When you edit the file, or a value an expression reads, status reports drift, the difference between what runs and what the file now says, until a reload applies it ([Config files and reload](config-and-reload.md)). `devlanes config check` checks the syntax of expressions and the names they use, but their values exist only per lane.

## A .env file feeds expressions, not processes

An expression that reads a variable with `env` takes it from the shell that ran devlanes, and when the shell does not have it, from the `.env` file next to the project file. It reads nothing else there, so it does not see the port devlanes hands out. A new worktree has none, since Git does not copy ignored files, and then the primary worktree's file is read.

devlanes does not put the contents of `.env` into a process. You name each value a process needs in the project file.

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
