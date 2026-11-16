---
title: devlanes daemon
description: Run, stop and install the background daemon, and pair other machines with it.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

```text
devlanes daemon [OPTIONS] <COMMAND>
```

## devlanes daemon start

Start the daemon in the background unless it already runs, then show its status.

```text
devlanes daemon start [OPTIONS]
```

## devlanes daemon status

Show the daemon's version, pid, uptime and managed process count; never starts it.

```text
devlanes daemon status [OPTIONS]
```

## devlanes daemon stop

Stop the daemon and every process it manages.

```text
devlanes daemon stop [OPTIONS]
```

## devlanes daemon restart

Restart the daemon and start again, with this shell's environment, the services that ran; asks first when processes are running.

```text
devlanes daemon restart [OPTIONS]
```

## devlanes daemon logs

Print the daemon's own log file.

```text
devlanes daemon logs [OPTIONS]
```

- `-f, --follow`: Keep printing new lines.
- `--tail <N>`: Lines to show [default: 100].

## devlanes daemon install

Install a login service so the daemon starts at login and outside agent sandboxes.

```text
devlanes daemon install [OPTIONS]
```

## devlanes daemon uninstall

Remove the login service; a running daemon keeps running, except on macOS, where launchd stops the one it started.

```text
devlanes daemon uninstall [OPTIONS]
```

## devlanes daemon reset-state

Stop everything and empty the state database; lanes get new numbers and ports (needs --yes).

```text
devlanes daemon reset-state [OPTIONS]
```

## devlanes daemon pair

Print a one-time code that pairs another machine with this one.

On a terminal it also prints how long the code is valid and the command to run on the other machine. Without a terminal, as in `ssh box devlanes daemon pair`, it prints only the code, for scripts.

```text
devlanes daemon pair [OPTIONS]
```

## devlanes daemon clients

List the machines paired or connected to this one.

```text
devlanes daemon clients [OPTIONS]
```

## devlanes daemon revoke

Disconnect a client and delete its pairing.

```text
devlanes daemon revoke [OPTIONS] <CLIENT>
```

- `<CLIENT>`: Client name, id, or a unique start of an id, as devlanes daemon clients shows them.

Every command also takes the [global options](README.md#global-options).
