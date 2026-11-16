---
title: Use devlanes from an agent sandbox
description: Reach the devlanes daemon from a coding agent whose sandbox keeps its shell away from it.
kind: how-to
---

Use this when an agent's devlanes commands fail with `auto-spawn refused in sandbox`: the agent host runs its shell in a sandbox, Codex among them, and devlanes does not start a daemon from there.

## Start the daemon outside the sandbox

On macOS and Linux, run this in your own terminal, not through the agent. It starts the daemon now and at each login, and the agent's commands then reach it.

```console
$ devlanes daemon install
```

If the agent's commands still fail, the sandbox does not let them reach the daemon either. Use the MCP server instead.

## Give the agent the MCP server

The agent host starts the MCP server itself. On Windows, Codex starts it outside its sandbox, so the agent's devlanes tools reach the daemon where its shell cannot.

```console
$ codex mcp add devlanes -- devlanes mcp
```

[Set up a coding agent](set-up.md) has the line for each agent host. A server the host starts inside its sandbox fails the same way the shell does.

:::caution
What the agent starts through the daemon runs outside the sandbox, with your account's rights. [Security](../../concepts/security.md) covers what that exposes.
:::

## Related

- [Set up a coding agent](set-up.md)
- [Security](../../concepts/security.md)
- [The daemon](../../concepts/daemon.md)
