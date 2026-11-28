---
title: Remote connection states
description: The states and warnings devlanes remote ls shows for a box, and what to do about each.
kind: reference
---

`devlanes remote ls` lists each box this machine knows, with the state of its connection, what it mirrors and any warnings. [Remote mirroring](../concepts/remote-mirroring.md) explains the connection.

## States

| State | Meaning | What to do |
|---|---|---|
| `connecting` | Your machine is reaching the box. | Wait. |
| `connected` | The box's lanes are mirrored here. | Nothing. |
| `reconnecting` | The connection dropped, after sleep or a network change, and devlanes tries again on its own. The box's ports and hostnames stay in place meanwhile. | Wait, or run `devlanes remote connect BOX` to try again now. `error` says why it dropped. |
| `needs_auth` | Reconnecting needs you: SSH asks for a password, a passphrase or a host key. | Run `devlanes remote connect BOX` in a terminal. |
| `needs_pairing` | The box no longer accepts this machine, or no pairing code was given. | Run `devlanes daemon pair` on the box, then `devlanes remote connect BOX --code CODE` here, or `devlanes remote connect <ticket>` for a box paired by ticket. |
| `failed` | Trying again cannot help, for example the two machines run releases too far apart to talk, or the box turns connections away. `error` says why. | Fix the cause, then run `devlanes remote connect BOX`. |
| `failed`, box paired by ticket | The box revoked this machine, or it stayed unreachable for 24 hours, an hour for a box from `--ephemeral`, and devlanes stopped trying. A box from `--ephemeral` is then forgotten. | After a revoke, or for a forgotten box, run `devlanes daemon pair` on the box and `devlanes remote connect <ticket>` here. Otherwise run `devlanes remote connect BOX` once the box is back. |
| `disconnected` | The box is known but not connected, as after `devlanes remote disconnect`. | Run `devlanes remote connect BOX` to connect again. |

A pairing code or ticket from `devlanes daemon pair` works once and expires after 10 minutes. [Connect to a box](../how-to/remote/connect.md) and [Connect to a box with a ticket](../how-to/remote/connect-by-ticket.md) have the steps.

A box paired by ticket that has no direct path to this machine stays `reconnecting` with that reason in `error`; devlanes never sends its traffic through a relay.

## Warnings

| Warning | What to do |
|---|---|
| `the box runs devlanes VERSION and this machine VERSION. Install the same release on both.` | Update the machine that is behind ([Update and uninstall](../how-to/update.md)). |
| ``the box stops your processes at logout. Run `sudo loginctl enable-linger "$USER"` there.`` | Run that command on the box. |
| ``the box daemon stops when this SSH session ends. Run `devlanes daemon install` on the box`` | Run that command on the box. |
| `port PORT is not mirrored because ...` | Something on this machine already uses the port, as the message names. Stop it, or keep the service on the box ([Choose what a box shares](../how-to/remote/what-a-box-shares.md)). |
| `this machine cannot use the box's HTTPS port PORT (...). Open its hostnames on port PORT.` | Open the box's hostnames on the port the message names. |
| `box BOX's ports.range ... overlaps box OTHER ...` | Give each box its own [`ports.range`](/docs/reference/config/user/ports/). |

A lane listed under `skipped_lanes` is not mirrored because another lane serves the same hostnames; its note gives the command that switches to it ([Work with several boxes](../how-to/remote/several-boxes.md)).
