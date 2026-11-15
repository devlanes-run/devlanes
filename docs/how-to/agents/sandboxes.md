---
title: Use devlanes from an agent sandbox
description: Reach the devlanes daemon from a coding agent whose sandbox keeps its shell away from it.
kind: how-to
---

Some agent hosts, Codex among them, run each shell command in a sandbox, and devlanes commands from that shell then fail because they may not start the daemon or reach one that runs. Depending on the OS and the sandbox, the way out is a daemon started outside the sandbox or the MCP server.

## Recognize the refusal

A command that cannot reach a daemon starts one, unless the shell looks sandboxed: a sandbox variable is set, or the daemon's socket, pipe or folder is off limits. Then it exits with code 4 and names the way out. On macOS, Codex sets `CODEX_SANDBOX` in the shells of its sandbox, and the refusal names that variable:

```console title="with CODEX_SANDBOX set and no daemon running"
$ devlanes start --no-ui
error: auto-spawn refused in sandbox (CODEX_SANDBOX); run devlanes daemon install outside the sandbox, then retry
hint: devlanes daemon install
```

The part in parentheses says what gave the sandbox away. When it is the socket, the pipe or the daemon's folder rather than a variable, it says which one was refused. The agent cannot fix this from inside, so it has to ask you.

## Check from inside the sandbox

The doctor command never starts a daemon, so it runs in the sandbox too. It reports whether a daemon answers and, when none does, whether this shell is sandboxed. When a daemon answers, its sandbox line describes the daemon instead, and reads "not sandboxed" for one you started outside.

```console title="with CODEX_SANDBOX set and no daemon running, on Windows 11"
$ devlanes doctor
daemon
  ! not running (pipe \\.\pipe\devlanes-999748f97f6ffe2c)
    fix: devlanes daemon start
sandbox
  ! this shell runs inside a sandbox (codex_sandbox)
    fix: devlanes daemon install, so the daemon starts outside the sandbox
```

Inside the sandbox, the sandbox line is the one to act on. Starting the daemon from the sandboxed shell, as the first fix says, is what the sandbox refuses.

## Start the daemon outside the sandbox

Run this in your own terminal, not through the agent. The agent's commands then find a running daemon and stop trying to start one.

::::tabs{sync="os"}
:::tab[macOS and Linux]
```console title="outside the sandbox"
$ devlanes daemon install
```

This registers a login service that starts the daemon now and at each login. Whether the sandboxed shell may then connect to the daemon's socket is up to the sandbox. When it may not, the refusal names the socket, and the MCP server below is the way in.
:::
:::tab[Windows]
The daemon's pipe accepts only your own account at medium integrity or above. Depending on its mode, Codex's Windows sandbox runs commands in a Windows execution container, as separate sandbox users or under a restricted token, and in testing its shell could not connect to the daemon however the daemon was started. For Codex, skip this step and register the MCP server below.

A sandbox that runs commands as your own account at medium integrity can reach a daemon you start in your terminal:

```console title="outside the sandbox"
$ devlanes daemon start
```

`devlanes daemon install` adds a start at each logon and starts nothing now. [Windows](../../reference/windows.md#terminals-and-the-daemon) covers terminals that end the daemon when they close.
:::
::::

## Give the agent the MCP server

The agent host starts the MCP server, and each tool call runs a devlanes command from it, which reaches the daemon and starts one when none runs. That works when the host starts its MCP servers outside the sandbox, as Codex did on Windows in testing:

```console
$ codex mcp add devlanes -- devlanes mcp
```

[Set up a coding agent](set-up.md) has the lines for other hosts. Before you rely on one, check that it starts MCP servers outside its sandbox, since a server inside it fails the same way the shell does.

:::caution
Either way, what the agent starts through the daemon runs outside the sandbox, with your account's rights. A service, task or command it starts there can read and write what you can. [Security](../../concepts/security.md) covers what that exposes.
:::

## Related

- [Set up a coding agent](set-up.md)
- [Security](../../concepts/security.md)
- [The daemon](../../concepts/daemon.md)
- [Windows](../../reference/windows.md)
