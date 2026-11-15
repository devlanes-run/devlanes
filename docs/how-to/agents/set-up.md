---
title: Set up a coding agent
description: Install the devlanes skill so a coding agent starts, tests and stops lanes, and register the MCP server for agent hosts that need it.
kind: how-to
---

Set this up when a coding agent works in a repository that runs on devlanes, so the agent hands its dev servers to the daemon instead of leaving them in the background of a shell that is gone a moment later. Your agent can install the skill and check the result itself.

```prompt
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then run `devlanes doctor` and tell me whether your shell can reach the devlanes daemon.
```

## Install the skill

The skill is a set of instructions your agent reads when a task touches a repository with a project file.

```console
$ npx skills add devlanes-run/devlanes --skill devlanes
```

The command asks which of your agents to install it for, and installs it into the current project; `-g` installs it for every project. It needs Node.js 22.20 or later.

With the skill, the agent starts the project's services through the daemon and runs its other long-running commands, such as a watcher, the same way, so they keep running and keep their logs after its shell command returns. It reads logs and failures, runs tests against the lane, stops only what its own session started, and can write or fix a project file. devlanes recognizes the agent from the variables its host sets and answers it in a shorter format, which [States and exit codes](../../reference/states-and-exit-codes.md) describes. [A coding agent runs the lane](../../learn/coding-agents.md) shows such a session.

## Register the MCP server

`devlanes mcp` is an MCP server: a process the agent host starts, which offers devlanes commands to the agent as tools. An agent whose shell reaches the daemon does not need it. Register it when the host has no shell, when the host's sandbox keeps the shell away from the daemon, or when the host shows MCP Apps, which turn results such as a status into a card.

::::tabs{sync="agent"}
:::tab[Claude Code]
```console
$ claude mcp add devlanes -- devlanes mcp
```

This registers it for the current project. `claude mcp add --scope user devlanes -- devlanes mcp` registers it for every project.
:::
:::tab[Codex]
```console
$ codex mcp add devlanes -- devlanes mcp
```

Codex starts MCP servers outside its sandbox, so the tools reach the daemon where the shell cannot. [Use devlanes from an agent sandbox](sandboxes.md) covers that case.
:::
:::tab[OpenCode]
```json title="opencode.json"
{
  "mcp": {
    "devlanes": { "type": "local", "command": ["devlanes", "mcp"] }
  }
}
```

Put it in the project's `opencode.json`, or in `~/.config/opencode/opencode.json` for every project.
:::
:::tab[Cursor]
```json title=".cursor/mcp.json"
{
  "mcpServers": {
    "devlanes": { "command": "devlanes", "args": ["mcp"] }
  }
}
```

Put it in the project's `.cursor/mcp.json`, or in `~/.cursor/mcp.json` for every project.
:::
::::

The server connects to nothing until the agent calls a tool, and each call runs one devlanes command, which starts the daemon when it is not running. `devlanes mcp --help` prints the lines for other hosts, among them VS Code, Claude Desktop and Goose. [MCP tools](../../reference/mcp-tools.md) lists the tools and what each host's time limit does to long calls.

## If the agent should only read

Flags go after `mcp`. With `--read-only` the agent gets the tools for status, logs and errors and none that start, stop or change anything.

```console
$ claude mcp add devlanes -- devlanes mcp --read-only
```

## If calls land in the wrong project

A tool call runs in the directory the agent passes, else in the project directory the host reports, else where the host started the server. Hosts with no project directory, such as Claude Desktop, need `--path` with an absolute path to the worktree.

## If services started through MCP cannot find your tools

A lane that no shell started gets its environment from the server. On macOS and Linux the server reads your login shell for it. On Windows it reads a PowerShell profile only when the server's environment sets `DEVLANES_SHELL_ENV`:

```json title=".cursor/mcp.json"
{
  "mcpServers": {
    "devlanes": {
      "command": "devlanes",
      "args": ["mcp"],
      "env": { "DEVLANES_SHELL_ENV": "pwsh" }
    }
  }
}
```

Use `powershell` instead of `pwsh` for Windows PowerShell 5.1.

## If you use Claude Code's status line

`devlanes status --statusline` prints one line for the lane of Claude Code's directory, with failures first. It never starts the daemon.

```json title="~/.claude/settings.json"
{
  "statusLine": { "type": "command", "command": "devlanes status --statusline" }
}
```

## Related

- [Use devlanes from an agent sandbox](sandboxes.md)
- [Run tests against a lane](../ci/tests.md)
- [A coding agent runs the lane](../../learn/coding-agents.md)
- [MCP tools](../../reference/mcp-tools.md)
