---
title: Environment and expressions
description: A unit's variables are layered from the shell that started it, from devlanes and from the project file, and values in the project file are evaluated for each lane.
kind: concept
---

A unit's environment is put together each time it starts. It begins with the shell that asked for the start, then gains what tools and devlanes add, then what the project file and the command line set. Values in the project file can be expressions, which devlanes works out for each lane, so one file gives each lane its own ports, hostnames and database names.

## Layers stack from the shell to the command line

A variable can come from several places, and the higher layer wins when two set the same name. The shell sits at the bottom, so its path, tool versions and credentials reach the service, but anything devlanes or the project file sets replaces them.

![Seven layers of a unit's environment, from the shell that ran the start at the bottom to --env on the command line at the top, each with the name env show gives it; a higher layer wins](environment.svg)

One consequence is that a stray `PORT` in your shell cannot move a service, since devlanes' own variables sit above the shell. Another is that `global_env` can override devlanes' variables, which is why the project file should not set the port there: every service would get the same one. [Environment variables](../reference/environment-variables.md) lists what devlanes sets.

On Windows names are uppercased in every layer, because Windows treats `Path` and `PATH` as one variable.

## Each variable shows the layer it came from

The explain option of the environment command prints the name of the layer next to each variable. In this lane of myapp the shell gave the API its home folder, the Node platform pointed it at the devlanes CA, and devlanes set its port and lane.

```console title="on Windows 11"
$ devlanes env show api --explain
KEY                           VALUE                                                                                                                     LAYER
DEVLANES_LANE                 0                                                                                                                         devlanes
DEVLANES_SERVICE              api                                                                                                                       devlanes
HOME                          C:\Users\you                                                                                                              client
HOST                          127.0.0.1                                                                                                                 devlanes
NODE_EXTRA_CA_CERTS           C:\Users\you\.devlanes\ca\ca.pem                                                                                          contributor
PORT                          20060                                                                                                                     devlanes
```

The client layer is the shell that sent the start. A command run outside a unit, through `devlanes exec` or `devlanes with`, gets the lane's part of this: the shell, devlanes' lane variables and the project's shared entries, without what belongs to one service.

## The shell's environment is never written down

The daemon keeps the environment of the shell that started a lane in memory, and reuses it when the TUI or a reload restarts a unit. It does not write it to disk, because a shell's environment holds tokens and other secrets. The price is that a lane needs one start from a shell after a reboot or a stop of the daemon, which [The daemon](daemon.md) shows.

The daemon itself keeps even less. One that a command starts inherits only a short list of that shell's variables, such as the path, the locale and the SSH agent, so tokens in that shell stay out of a process that serves every later one.

## Expressions are evaluated for each lane

A value in the project file can hold `${{ }}` expressions, which read the lane: its number, name and domain, the ports and URLs of its services, its containers, and the shell's variables. devlanes evaluates them for each lane when it plans a start, and a running unit keeps the values it started with. A later edit to the file, or to something an expression reads, shows on the running lane as a change until a reload applies it ([Config files and reload](config-and-reload.md)).

```yaml devlanes
services:
  worker:
    autostart: ${{ lane.index == 0 }}
  web:
    env:
      VITE_API_URL: "${{ services.api.public_url }}"
```

A value that is one expression and nothing else keeps its type, so the worker's setting is a true or false and not the text. A name that does not exist is an error, not an empty string: a missing variable stops the start with the file and line, where an empty value would have started a service pointed at nothing. The file holds expressions only, with no loops or template blocks, and the config check verifies their syntax and the names they use, while their values exist only per lane. [Expressions](../reference/expressions.md) lists the namespaces, functions and filters.

## A .env file feeds expressions, not processes

An expression reads a variable from the shell that ran devlanes first, then from the `.env` file next to the project file. A new worktree has none, since Git does not copy ignored files, so a lane without one reads the primary worktree's. Expressions read only these two sources, so `env.PORT` is the shell's value and not the port devlanes hands out.

devlanes does not put the contents of `.env` into a process. Each value a process needs is named in the project file, so it shows in the explain output and in what a reload compares.

```yaml devlanes
global_env:
  STRIPE_KEY: ${{ env.STRIPE_KEY }}
  SENTRY_DSN: ${{ env.SENTRY_DSN | default('') }}
```

A missing STRIPE_KEY stops the start, and a missing SENTRY_DSN becomes empty. An app that loads `.env` itself still can; most dotenv loaders leave a variable that is already set alone, so the value from the project file wins. [Read .env values and pin tool versions](../how-to/projects/env-files-and-tools.md) sets this up.

## Related

- [Services that find each other](../learn/connecting-services.md)
- [Expressions](../reference/expressions.md)
- [Environment variables](../reference/environment-variables.md)
- [Config files and reload](config-and-reload.md)
