---
title: Set up a coding agent
description: Install the devlanes skill so a coding agent starts, tests and stops lanes, and register the MCP server for agent hosts that need it.
kind: how-to
---

Set this up when a coding agent works in a repository that runs on devlanes, so the agent hands its dev servers to the daemon instead of leaving them in the background of a shell that is gone a moment later. Your agent can do the first steps itself.

```prompt
Install the devlanes skill with `npx skills add devlanes-run/devlanes --skill devlanes`. Then check that your shell can run devlanes in this repository. If it cannot, register the devlanes MCP server with this agent, and tell me which steps I have to do myself, such as restarting you.
```

## Install the skill

The skill is a set of instructions your agent reads when a task touches a repository with a project file.

```console
$ npx skills add devlanes-run/devlanes --skill devlanes
```

The command asks which of your agents to install it for, and installs it into the current project; `-g` installs it for every project. It needs Node.js 22.20 or later. With the skill, the agent runs dev servers and watchers through the daemon, so they outlive its shell commands. [A coding agent runs the lane](../../learn/coding-agents.md) shows such a session.

## Register the MCP server

`devlanes mcp` is an MCP server: a process the agent host starts, which offers devlanes commands to the agent as tools. An agent whose shell reaches the daemon does not need it. Register it when the host has no shell, when the host's sandbox keeps the shell away from the daemon, or when the host shows MCP Apps, which turn results such as a status into a card.

::::tabs{sync="agent"}
:::tab[Claude Code]
```console
$ claude mcp add devlanes -- devlanes mcp
```

This registers it for the current project. For every project:

```console
$ claude mcp add --scope user devlanes -- devlanes mcp
```
:::
:::tab[Codex]
```console
$ codex mcp add devlanes -- devlanes mcp
```

Codex keeps it in `~/.codex/config.toml`, so it applies to every project. When the Codex sandbox keeps the shell away from the daemon, [Use devlanes from an agent sandbox](sandboxes.md) says what the MCP server changes.
:::
:::tab[OpenCode]
```console
$ opencode mcp add devlanes -- devlanes mcp
```

This writes it to the project's configuration. For every project:

```console
$ opencode mcp add devlanes --global -- devlanes mcp
```
:::
:::tab[Cursor]
```json title=".cursor/mcp.json"
{
  "mcpServers": {
    "devlanes": { "type": "stdio", "command": "devlanes", "args": ["mcp"] }
  }
}
```

Put it in the project's `.cursor/mcp.json`, or in `~/.cursor/mcp.json` for every project. Cursor shows MCP Apps.
:::
::::

The server connects to nothing until the agent calls a tool. Each call runs one devlanes command, which starts the daemon as the same command would in a terminal. `devlanes mcp --help` prints the lines for other hosts, among them VS Code, Claude Desktop and Goose, and [MCP tools](../../reference/mcp-tools.md) lists the tools.

## If the agent should only read

Flags go after `mcp`. With `--read-only` the agent gets the tools for status, logs and errors and none that start, stop or change anything.

```console
$ claude mcp add devlanes -- devlanes mcp --read-only
```

## If calls land in the wrong project

A tool call runs in the directory the agent passes, else in the one `--path` names, else in Claude Code's project directory, else where the host started the server. For a host with no project directory, such as Claude Desktop, add `--path` with the worktree's absolute path. Claude Desktop also needs the full path of the command, since it does not read your shell's path:

```json title="claude_desktop_config.json"
{
  "mcpServers": {
    "devlanes": {
      "command": "/Users/you/.local/bin/devlanes",
      "args": ["mcp", "--path", "/Users/you/dev/myapp"]
    }
  }
}
```

## If services started through MCP cannot find your tools on Windows

A lane that no shell started gets its environment from the server. On Windows that is the server's own environment, which lacks what your PowerShell profile adds, such as a Node version manager. Set `DEVLANES_SHELL_ENV` to `pwsh` in the server's environment, and the server reads your profile:

::::tabs{sync="agent"}
:::tab[Claude Code]
```console
PS> claude mcp add --env DEVLANES_SHELL_ENV=pwsh --transport stdio devlanes -- devlanes mcp
```
:::
:::tab[Codex]
```console
PS> codex mcp add devlanes --env DEVLANES_SHELL_ENV=pwsh -- devlanes mcp
```
:::
:::tab[OpenCode]
```console
PS> opencode mcp add devlanes --env DEVLANES_SHELL_ENV=pwsh -- devlanes mcp
```
:::
:::tab[Cursor]
```json title=".cursor/mcp.json"
{
  "mcpServers": {
    "devlanes": {
      "type": "stdio",
      "command": "devlanes",
      "args": ["mcp"],
      "env": { "DEVLANES_SHELL_ENV": "pwsh" }
    }
  }
}
```
:::
::::

For Windows PowerShell 5.1, use `powershell` instead. On macOS and Linux the server reads your login shell by itself. [MCP tools](../../reference/mcp-tools.md#environment-of-a-lane-started-through-mcp) has the details of both.

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
