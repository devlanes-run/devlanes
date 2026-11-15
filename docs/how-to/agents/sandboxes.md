---
title: Use devlanes from an agent sandbox
description: Reach the devlanes daemon from a coding agent whose sandbox keeps its shell away from it.
kind: how-to
---

Some agent hosts, Codex among them, run each shell command in a sandbox, and devlanes commands from that shell then fail because they cannot start the daemon or cannot reach one that runs. The way out is a daemon started outside the sandbox, or the MCP server, which the host starts outside it.

## Recognize the refusal

A command that finds no daemon starts one, but not from a shell that looks sandboxed. It checks for the variables Codex and macOS app sandboxes set, and for a socket or pipe it may not connect to or a folder it may not create. Then it exits with code 4 and names the way out.

```console title="in a Codex sandbox, on Windows 11"
$ devlanes start
Error: auto-spawn refused in sandbox (CODEX_SANDBOX); run devlanes daemon install outside the sandbox, then retry
hint: devlanes daemon install
```

The part in parentheses says what gave the sandbox away. The agent cannot fix this from inside, so it has to ask you.

## Check from inside the sandbox

The doctor command never starts a daemon, so it runs in the sandbox and reports both sides: whether a daemon answers, and whether this shell is sandboxed.

```console title="in a Codex sandbox, on Windows 11"
$ devlanes doctor
daemon
  ! not running (pipe \\.\pipe\devlanes-9ae1a19790fe1d39)
    fix: devlanes daemon start
sandbox
  ! this shell runs inside a sandbox (codex_sandbox)
    fix: devlanes daemon install, so the daemon starts outside the sandbox
```

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
```console title="outside the sandbox"
$ devlanes daemon install
$ devlanes daemon start
```

The first command adds a start at each logon and starts nothing now, so the second starts the daemon for this session. The daemon's pipe refuses low-integrity processes, which is how most sandboxes on Windows run their commands, Codex's included. For those, use the MCP server below. [Windows](../../reference/windows.md) covers terminals that end the daemon when they close.
:::
::::

## Give the agent the MCP server

Codex starts its MCP servers outside its sandbox, so each tool call runs a devlanes command that reaches the daemon, and starts one when none runs. Register it once:

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
