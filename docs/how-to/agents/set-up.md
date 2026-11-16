---
title: Set up a coding agent
description: Install the devlanes skill so a coding agent starts, tests and stops lanes, and register the MCP server for agent hosts that need it.
kind: how-to
---

Set this up when a coding agent works in a repository that runs on devlanes. Your agent can do it for you:

```prompt
Install the devlanes skill:
npx skills add devlanes-run/devlanes --skill devlanes

Then check that your shell can run devlanes in this repository.
If it cannot, register the devlanes MCP server with this agent.
Tell me which steps I have to do myself, such as restarting you.
```

## Install the skill

The skill teaches the agent to run dev servers and watchers through the daemon, so they outlive its shell commands, and to test and stop what it started.

```console
$ npx skills add devlanes-run/devlanes --skill devlanes
```

## Register the MCP server

The MCP server offers devlanes commands to the agent as tools. Register it when the agent host has no shell, or when its sandbox keeps the shell away from the daemon.

::::tabs{sync="agent"}
:::tab[Claude Code]
```console
$ claude mcp add --scope user devlanes -- devlanes mcp
```
:::
:::tab[Codex]
```console
$ codex mcp add devlanes -- devlanes mcp
```
:::
:::tab[OpenCode]
```console
$ opencode mcp add devlanes --global -- devlanes mcp
```
:::
:::tab[Cursor]
```json title="~/.cursor/mcp.json"
{
  "mcpServers": {
    "devlanes": { "type": "stdio", "command": "devlanes", "args": ["mcp"] }
  }
}
```
:::
::::

Each registers the server for every project. `devlanes mcp --help` prints the lines for other hosts.

## If the agent should only read

Add `--read-only` after `mcp`, and the agent gets the tools for status, logs and errors only:

```console
$ claude mcp add --scope user devlanes -- devlanes mcp --read-only
```

## If the agent's shell is sandboxed

[Use devlanes from an agent sandbox](sandboxes.md) says which way in works.

## Related

- [Use devlanes from an agent sandbox](sandboxes.md)
- [Run tests against a lane](../ci/tests.md)
- [A coding agent runs the lane](../../learn/coding-agents.md)
- [MCP tools](../../reference/mcp-tools.md)
