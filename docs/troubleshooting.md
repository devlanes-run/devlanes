---
title: Troubleshooting
description: Common problems with devlanes, each with its cause and fix.
kind: reference
---

`devlanes debug why UNIT` explains a unit that is not running or not ready, and `devlanes doctor` checks the machine. Text in capitals, such as `UNIT`, stands for the name devlanes prints there.

## Services

| What you see | Cause | Fix |
|---|---|---|
| `UNIT couldn't start: PROGRAM: command not found on PATH` | The program is not on the `PATH` of the shell that started the lane | Start the lane from a shell that finds it ([Bring lanes back after a reboot](how-to/lanes/after-a-reboot.md#if-the-daemon-is-still-running)) |
| `UNIT wasn't ready in time` | The ready check never passed | [Wait until a service is ready](how-to/services/ready-checks.md) |
| `nothing accepts connections on port PORT` | The service listens on another port than the one devlanes gave it | [Give a service several ports and hostnames](how-to/services/ports-and-hostnames.md#if-the-program-insists-on-a-fixed-port) |
| A service crashed, and `status` still shows it `ready` | A watcher such as `node --watch` keeps the process alive | Read `devlanes logs` for the service and run `devlanes restart` on it ([When a service fails](learn/failures.md)) |
| `port PORT is taken by pid N` | Another program holds a port of the lane | Stop that program, then start again |
| `PATH has no value in LANE` | An expression such as `${{ env.API_KEY }}` has no value, often a missing `.env` | [Read .env values and pin tool versions](how-to/projects/env-files-and-tools.md) |
| `global_env.NAME has no value in LANE` on each service, under a profile with `infra: false` | The value reads a container the profile did not start | [Share settings and choose which services start](how-to/projects/templates-and-profiles.md#if-a-profile-should-skip-the-containers) |
| `Docker isn't running or can't be reached` | The project has containers and Docker is down | Start Docker |

## Lanes and the daemon

| What you see | Cause | Fix |
|---|---|---|
| Lanes are `idle` with `needs start from a shell` | The daemon restarted, or ended with the terminal | [Bring lanes back after a reboot](how-to/lanes/after-a-reboot.md) |
| The daemon stops when you close a terminal on Windows | That terminal ends the processes it started | [Windows](reference/windows.md) |
| `devlanes: command not found` in a project whose `package.json` lists devlanes | The project pins devlanes, so the command is not on your `PATH` | Run it as `npx devlanes` ([Install devlanes](install.md#if-a-project-pins-devlanes)) |
| `no devlanes.yaml in DIR or its parents` | The command ran outside a project | [Add devlanes to a project](how-to/projects/add-devlanes.md) |
| `the running daemon is devlanes VERSION ... and cannot serve this CLI` | The daemon runs another release than the CLI | `devlanes daemon restart` ([Update and uninstall](how-to/update.md)) |
| `the port pool has no free block of N ports for another lane` | Old lanes hold every port block | [Remove lanes](how-to/lanes/clean-up.md) |
| `auto-spawn refused in sandbox` | The command runs inside an agent's sandbox | [Use devlanes from an agent sandbox](how-to/agents/sandboxes.md) |

## Hostnames and HTTPS

| What you see | Cause | Fix |
|---|---|---|
| A certificate error, such as `ERR_CERT_AUTHORITY_INVALID` | This browser or tool does not trust the devlanes CA | [Trust the devlanes CA](how-to/hostnames/trust-the-ca.md) |
| HTTPS fails with no page for a hostname | No lane routes that hostname | Check the name with `devlanes url ls` ([Give a service several ports and hostnames](how-to/services/ports-and-hostnames.md#name-the-ports-and-route-hostnames-to-them)) |
| A program cannot resolve a `*.localhost` name | Its resolver does not send `*.localhost` to your machine | [Fix programs that cannot resolve *.localhost](how-to/hostnames/resolve-localhost.md) |
| URLs end in `:8443` or `:8080` | Another program holds port 443 or 80 | [Choose the proxy's ports](how-to/hostnames/proxy-ports.md) |
| [DL201 Service not running](errors/service-idle.md) | The service or its lane is stopped | Start it with the command the page shows |
| [DL203 Service failed](errors/service-failed.md) | The service exited or failed its ready check | `devlanes debug why UNIT` |

## Remote boxes

| What you see | Cause | Fix |
|---|---|---|
| `needs_auth` in `devlanes remote ls` | Reconnecting needs your SSH password, passphrase or host key | `devlanes remote connect BOX` in a terminal |
| `needs_pairing` in `devlanes remote ls` | The box no longer accepts this machine | [Connect to a box](how-to/remote/connect.md) |
| `no direct connection to the box formed` | The networks between the two machines allow no direct path, and devlanes does not relay lane traffic | Connect over SSH ([Connect to a box](how-to/remote/connect.md)) or [Tailscale](how-to/remote/tailscale.md) |
| `the box has no pairing ticket open` | The ticket expired, was used already, or the box's daemon restarted since it was printed | A new ticket from `devlanes daemon pair` on the box |
| `the box's relay HOST is not one this machine uses` | The box uses a relay missing from this machine's `remote.relays` | Add that relay to `remote.relays` and run `devlanes daemon restart` ([Connect to a box with a ticket](how-to/remote/connect-by-ticket.md)) |
| Other states and warnings of `devlanes remote ls` | | [Remote connection states](reference/remote-states.md) |
