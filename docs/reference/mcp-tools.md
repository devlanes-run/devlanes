---
title: MCP tools
description: The tools devlanes mcp offers to a coding agent, and the options of the server.
kind: reference
---

`devlanes mcp` serves devlanes to a coding agent over MCP. [Set up a coding agent](../how-to/agents/set-up.md) registers it with your agent.

## Options

| Option | Effect |
|---|---|
| `--read-only` | Offer only the tools that change nothing. |
| `--path DIR` | Run calls that name no `cwd` in this worktree. |
| `--lane LANE` | Use this lane for calls that name no `cwd` or lane. |

## Tools

Each tool runs the devlanes command of the same name and takes that command's options as arguments, with `_` in place of `-`. Every tool also takes `cwd`, the worktree to run in.

| Tool | Does | Main arguments |
|---|---|---|
| `status` | Show lanes and units with their state and URLs | `lane`, `mine` |
| `start` | Start the lane and wait until it is ready | `units`, `profile`, `timeout` |
| `exec` | Run a dev server, watcher or other long-running command as a unit of the lane | `name`, `command` |
| `logs` | Read the logs of units | `units`, `tail`, `grep`, `since` |
| `stop` | Stop units, or the whole lane | `units`, `mine` |
| `restart` | Restart units, or the whole lane | `units` |
| `reload` | Apply changes to the project file or `.env` | `units` |
| `wait` | Wait until a unit or lane is ready or failed | `target`, `timeout` |
| `with` | Start the lane, run a command such as the tests, then stop what was started | `command`, `keep` |
| `run` | Run a task from the project file | `task` |
| `debug_why` | Explain why a unit is not running or not ready | `unit` |
| `debug_errors` | List failed units and recent errors | `since` |
| `debug_top` | Show CPU and memory of the running units | |
| `url_print` | Print a service's URL | `unit` |
| `url_ls` | List the routed hostnames | |
| `env_show` | Print the environment of the lane or a unit | `unit`, `explain` |
| `lane_ls` | List the lanes | |
| `lane_clean` | Clean a lane and delete its data | `lane`, `dry_run` |
| `daemon_pair` | Print a pairing code for another machine | |
| `doctor` | Check the daemon, Docker, proxy, CA and hostnames | |
| `devlanes` | Run any other devlanes command | `args` |

`lane_clean`, `daemon_pair` and other commands that delete data or reach other machines ask you before they run. Commands that need a terminal, such as `ui` and `setup`, are refused.

Calls that wait, such as `start` and `wait`, stop a little before the agent's own time limit and return what is ready so far; another `wait` call continues.
