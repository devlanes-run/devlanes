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

`devlanes daemon pair` on the box prints a ticket and a code, each valid once for 10 minutes; without a terminal it prints only the code, and with `--ticket` or `--ephemeral` only the ticket. When it prints a code with `no ticket yet` or `No ticket`, run `devlanes daemon pair --ticket` for the ticket; it waits longer. Both are secrets: show them to the user, never put them in files or commits.

### By ticket, without SSH

1. On the box: `devlanes daemon install` (on Linux after the linger step above), so the box stays reachable after the shell ends, then `devlanes daemon pair`.
2. On the user's machine: `devlanes remote connect <ticket> --background`, with the whole ticket (it starts with `dlp1`). The output names the box, `my-vps` here, and later connects use `devlanes remote connect my-vps`. `--name NAME` chooses another name at pairing.

Run `remote connect <ticket>` only with a ticket the user gave you. Through `devlanes mcp` the server asks the user first; pass `--yes` only when the user agreed in the chat.

The connection needs a direct path between the machines. `no direct connection to the box formed` means the networks allow none: use SSH or Tailscale instead. `devlanes remote disconnect my-vps --forget` deletes the pairing. `remote.relays: []` in both user configs, and `devlanes daemon restart` on both, keeps machines on one network off the relay; a new ticket is then needed when the box's IP address changes.

### By code, over SSH or a command

1. On the box: `devlanes daemon pair`, and give the user the code.
2. On the user's machine: `devlanes remote connect my-vps --code CODE`, where `my-vps` is the name the user reaches the box by: an SSH host from `~/.ssh/config`, or a `remote.boxes` name in the user config.

`remote connect` stays attached to show what SSH asks, such as a passphrase or a new host key. You cannot answer those, so when the user asks you to connect, run `devlanes remote connect my-vps --code CODE --background`, which returns once connected. When SSH needs an answer, the connection waits in `needs_auth`: ask the user to run `devlanes remote connect my-vps` in their own terminal. An SSH key in an agent avoids the prompts on later reconnects.

With SSH access from the user's machine, `ssh my-vps '~/.devlanes/bin/devlanes daemon pair'` prints the code there directly, and `ssh my-vps '~/.devlanes/bin/devlanes daemon pair --ticket'` the ticket.

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
| `reconnecting` in `remote ls` | The box is unreachable, and devlanes retries on its own with growing waits | When the user says the box is back, `devlanes remote connect my-vps --background` tries again at once. |
| `needs_auth` in `remote ls`, `box my-vps needs you to sign in again` | SSH asks for a password, passphrase or host key | The user runs `devlanes remote connect my-vps` in a terminal. |
| `needs_pairing`, `box my-vps needs a new pairing code` | The box has no pairing for this machine | A new code from `devlanes daemon pair` on the box, then `devlanes remote connect my-vps --code CODE`. |
| `the pairing code is wrong, expired or already used` | A typo, an old code, or a used one | A new code. |
| `pairing is paused for N minutes after 5 wrong codes` | Too many wrong codes | Wait, then a new code. Tickets still work. |
| `the box has no pairing ticket open` | The ticket expired, was used, or the box's daemon restarted | A new ticket from `devlanes daemon pair` on the box. |
| `the pairing ticket is wrong, expired or already used` | The box refused the ticket's secret | A new ticket from `devlanes daemon pair` on the box. |
| `the box is not reachable` with `the box's relay HOST is not one this machine uses` | The box uses a relay missing from this machine's `remote.relays` | Tell the user; add the relay only if they trust it and ask for it. |
| `the box is not reachable` without a relay named | The box's daemon is not running, the ticket is over 10 minutes old, or the box's address changed | Ask the user to start the box's daemon and try again; a new ticket only if this one is over 10 minutes old or the box's address changed. |
| `this is not a box name or an SSH destination, and not a whole pairing ticket either` | The ticket was cut or changed when copied | Copy the whole ticket again. |
| `NAME is already the name of ...` (`box_name_taken`) | Another box or a `remote.boxes` entry has the name | Pair with `--name OTHER`. |
| `is both a box paired by ticket and an entry of remote.boxes`, or `differs only in case from` | A name is both a box paired by ticket and a `remote.boxes` entry, or differs from one only in case | Follow the message: rename the `remote.boxes` entry, or `devlanes remote disconnect NAME --forget` and pair again with `--name`. |
| `NAME is not a box paired by ticket here` (`box_not_paired_by_ticket`) | `--forget` was given a box reached over SSH or a command | `devlanes remote disconnect NAME` without `--forget`. |
| `the pairing ticket is damaged` | Part of the ticket was lost when copying | Copy the whole ticket again. |
| `no direct connection to the box formed` | No direct path between the machines | Connect over SSH or Tailscale instead. |
| `failed`, `box my-vps no longer accepts this machine` | The box revoked this machine's ticket pairing | A new ticket from `devlanes daemon pair` on the box, then `devlanes remote connect <ticket>`. |
| `failed`, `box my-vps has not been reachable for 24 hours` | The box stayed offline; devlanes stopped trying | `devlanes remote connect my-vps` once the box is back. |
| A warning that the releases differ, or a protocol error | The machines run different releases | Install the same release on both, then `devlanes daemon restart` on the box. |
| `the box stops your processes at logout` | Linger is off on the Linux box | `sudo loginctl enable-linger "$USER"` on the box. |
| `port PORT is not mirrored because ...` | The port is taken on the user's machine | Free it there, or split `ports.range` as above. |
| `this machine does not accept connections because remote.allow_connections is false` | The box turns clients away | Only the box's user changes `remote.allow_connections`. |
| DL204, DL205, DL206 pages in the browser | The box is reconnecting, needs a sign-in, or needs a pairing | `devlanes remote ls` names the command. |

[Prepare a box](https://devlanes.run/docs/how-to/remote/prepare-a-box.md), [Connect to a box with a ticket](https://devlanes.run/docs/how-to/remote/connect-by-ticket.md) and [Connect to a box](https://devlanes.run/docs/how-to/remote/connect.md) are the pages for the user. [Remote connection states](https://devlanes.run/docs/reference/remote-states.md) lists every state and warning.
