# Boxes

A box is a machine whose lanes are mirrored to another machine, the user's, where the browser runs. The user's machine listens on the box's ports under the same numbers and serves the box's hostnames through its own proxy, so `https://myapp.localhost` opens on the user's machine while the lane runs on `my-vps`. Read this when a task involves a box: preparing one, pairing, connecting, or acting on its lanes.

The box and the user's machine must run matching releases. Releases on another protocol revision do not connect; releases that differ on the same revision connect with a warning to install the same release on both. devlanes does not update a box; `devlanes --version` on each machine shows the release.

## What you can do from where

| You run on | You can | You cannot |
|---|---|---|
| The box | Everything in [SKILL.md](../SKILL.md) for the box's lanes: start, stop, logs, tests. `devlanes daemon pair`, `devlanes daemon clients`, `devlanes daemon revoke CLIENT`. | Open anything on the user's machine. |
| The user's machine | `devlanes remote ls`, `devlanes url ls`, `devlanes remote sync`; `devlanes remote connect`, `remote disconnect` and `remote use` when the user asks. | Start, stop or restart a box unit, read its logs or run a command on the box through devlanes. The connection carries traffic, the list of lanes, ports and hostnames, and synced folders, nothing that runs a command. |

To act on a box lane from the user's machine, run devlanes on the box over SSH when the user has SSH access set up, as in `ssh my-vps 'cd ~/dev/myapp && ~/.devlanes/bin/devlanes status'`, or ask the user. A box installed with `--dir ~/.devlanes/bin` may not have devlanes on the `PATH` of a non-interactive SSH command; call it by that path. A daemon started by such a command keeps that command's short `PATH`, which can lack Docker or a version manager; `devlanes daemon restart` from an interactive shell on the box fixes it, and asks first when processes run.

## Prepare a box

Run on the box, over SSH:

```sh
curl -fsSL https://devlanes.run/install.sh | sh -s -- --dir ~/.devlanes/bin
```

The user's machine looks for devlanes in `~/.devlanes/bin` when it connects. The box also needs Git, Docker when the project has containers, and on Linux glibc 2.35 or later.

Both machines hand out ports from the same default range, and a port one of the user's lanes already holds cannot be mirrored. Split the range before either machine's lanes have ports, when the user agrees:

```sh
devlanes config set ports.range 25000-32767
```

That on the box, and `devlanes config set ports.range 20000-24999` on the user's machine. The daemon reads it when it starts, so a running daemon needs `devlanes daemon restart`, which restarts its services; ask first. Lanes that already have ports keep them until they are cleaned.

On a Linux box, systemd stops the user's processes, the daemon included, when their last session ends, unless linger is on. This needs `sudo`, so the user runs it unless `sudo` works without a password:

```sh
sudo loginctl enable-linger "$USER"
```

`devlanes daemon install` on the box brings the daemon back after a reboot; on Linux it depends on linger too. Then clone the project on the box and run `devlanes start` in it, as on any machine. Its services must listen on loopback or on all addresses.

## Pair and connect

A box serves only machines it has paired with, unless its user config sets `remote.require_pairing: false`. Never set that unless the user asks.

1. On the box: `devlanes daemon pair`. On a terminal it prints the code, how long it is valid and the command to run on the other machine; without a terminal it prints only the code. A code works once.
2. On the user's machine: `devlanes remote connect my-vps --code CODE`, where `my-vps` is the name the user reaches the box by: an SSH host from `~/.ssh/config`, or a `remote.boxes` name in the user config.

`remote connect` stays attached to show what SSH asks, such as a passphrase or a new host key. You cannot answer those, so when the user asks you to connect, run `devlanes remote connect my-vps --code CODE --background`, which returns once connected. When SSH needs an answer, the connection waits in `needs_auth`: ask the user to run `devlanes remote connect my-vps` in their own terminal. An SSH key in an agent avoids the prompts on later reconnects.

With SSH access from the user's machine, `ssh my-vps '~/.devlanes/bin/devlanes daemon pair'` prints the code there directly.

The connection belongs to the user's daemon. It reconnects after sleep, a network change or a daemon restart, and stays wanted until `devlanes remote disconnect my-vps`.

## Check the mirror

On the user's machine:

- `devlanes remote ls` lists each box with its state, the ports mirrored for it and warnings, such as releases that differ or a Linux box without linger. A port that a program on this machine holds is listed with that program, and is not mirrored.
- `devlanes url ls` in the project's folder lists the box's hostnames next to the local ones.

A lane on the user's machine with the same hostnames keeps them while it runs and while it is stopped. `devlanes remote use my-vps myapp@0` hands them to the box's lane; `devlanes remote use --local myapp@0` gives them back, and so does starting the local lane.

## When you work on the box and the user's browser is elsewhere

The URLs devlanes prints on the box are the box's. Before giving the user a URL, run `devlanes url print web --json`. Its `clients` list has one entry per connected machine that reports to this box:

| Entry | Tell the user |
|---|---|
| `reachable: true` | Its `url`, to open on that machine. |
| `reachable: false` | Its `reason`, such as a port in use there or a lane that machine does not mirror. |
| Empty `clients` | No machine is connected. When `devlanes daemon clients --json` lists the user's machine as paired, ask them to run `devlanes remote connect my-vps` on it. Otherwise run `devlanes daemon pair` and pass the code on with the command from step 2 above. |

A machine that waits in `needs_pairing` needs a new code the same way.

## Problems

| What you see | Cause | Fix |
|---|---|---|
| `needs_auth` in `remote ls`, `box my-vps needs you to sign in again` | SSH asks for a password, passphrase or host key | The user runs `devlanes remote connect my-vps` in a terminal. |
| `needs_pairing`, `box my-vps needs a new pairing code` | The box has no pairing for this machine | A new code from `devlanes daemon pair` on the box, then `devlanes remote connect my-vps --code CODE`. |
| `the pairing code is wrong, expired or already used` | A typo, an old code, or a used one | A new code. |
| `pairing is paused for N minutes after 5 wrong codes` | Too many wrong codes | Wait, then a new code. |
| A warning that the releases differ, or a protocol error | The machines run different releases | Install the same release on both, then `devlanes daemon restart` on the box. |
| `the box stops your processes at logout` | Linger is off on the Linux box | `sudo loginctl enable-linger "$USER"` on the box. |
| `port PORT is not mirrored because ...` | The port is taken on the user's machine | Free it there, or split `ports.range` as above. |
| `this machine does not accept connections because remote.allow_connections is false` | The box turns clients away | Only the box's user changes `remote.allow_connections`. |
| DL204, DL205, DL206 pages in the browser | The box is reconnecting, needs a sign-in, or needs a pairing | `devlanes remote ls` names the command. |

[Prepare a box](https://devlanes.run/docs/how-to/remote/prepare-a-box.md) and [Connect to a box](https://devlanes.run/docs/how-to/remote/connect.md) are the pages for the user. [Remote connection states](https://devlanes.run/docs/reference/remote-states.md) lists every state and warning.
