---
title: Remote connection states
description: The states and warnings of a connection to a box, its timings, the pairing rules, and what a box and your machine send each other.
kind: reference
---

`devlanes remote ls` lists each box this machine knows, with the state of its connection, what it mirrors and its warnings. `--json` prints the same entries; where its field names differ, the tables give both. [Remote mirroring](../concepts/remote-mirroring.md) explains the connection, and [Security](../concepts/security.md) what pairing protects.

## Fields of a box

| Field | Content |
|---|---|
| `host` | The name the box was connected by. |
| `state` | The connection state, below. |
| `transport` | `ssh`, or `command` for a box reached by a command. |
| `address` | The name the box was last reached by. |
| `version` | The box's release, as `devlanes VERSION`. In `--json`, `box.version`. |
| `daemon_version` | The box's release label from its latest handshake, with `(linger off)` or `(daemon stops with the session)` when those apply. |
| `connected_since` | How long the session has been up. In `--json`, `connected_since_ms`, a time in milliseconds. |
| `last_connected_ms` | When the box last connected, in milliseconds. |
| `https_port` | The port the box's proxy serves HTTPS on. |
| `lanes` | How many lanes the box reports. |
| `ports` | The mirrored ports, under [Mirrored ports](#mirrored-ports). |
| `routes` | The hostnames routed to the box, under [Routes, skipped lanes and hostnames](#routes-skipped-lanes-and-hostnames). |
| `skipped_lanes`, `skipped_hosts`, `chosen` | Under [Routes, skipped lanes and hostnames](#routes-skipped-lanes-and-hostnames). |
| `syncs` | The synced folders, under [Synced folders](#synced-folders). |
| `warnings` | Under [Warnings](#warnings). |
| `box`, `registry`, `proxy`, `suffixes` | In `--json` only: what the box said about itself (`version`, `label`, `https_port`, `ports_range`), its lanes and units with their states, ports and hostnames, this machine's proxy ports for the box, and the name suffixes box hostnames must end in. |

## Connection states

| State | Meaning | What follows it |
|---|---|---|
| `connecting` | An attempt to reach the box runs. `attempt` counts the attempts since the last connection. | `connected`, or one of the states below when the attempt fails. |
| `connected` | A session with the box is up. `server` is the box's release label, with `(linger off)` or `(daemon stops with the session)` when the warnings below apply. | `reconnecting` when the session ends. |
| `reconnecting` | The last attempt or session ended, and the next attempt waits `retry_in_ms`. `error` says why. Mirrored ports stay bound and hostnames stay routed. | The next attempt, on its own. |
| `needs_auth` | Reconnecting needs a person: SSH asked for a password, a passphrase or a host key confirmation with no `devlanes remote connect` attached, or rejected credentials or a host key of a box that connected before. Nothing is retried. Mirrored ports and hostnames stay while the daemon runs. `fix` reads `devlanes remote connect BOX`. | `connecting`, after the command in `fix`. |
| `needs_pairing` | The box serves only paired machines and has no pairing for this one: no code was given, the box refused the code, or it no longer accepts this machine's token. Nothing is retried. Mirrored ports and hostnames stay while the daemon runs. `fix` reads `devlanes daemon pair on BOX, then devlanes remote connect BOX --code <code>`. After too many wrong codes, `pairing_paused` says `the box takes codes again in TIME`; in `--json`, `pairing_retry_at_ms` gives that time in milliseconds. | `connecting`, after the command in `fix`. |
| `failed` | Retrying cannot help: the release on the box speaks another protocol, the box takes no connections ([`remote.allow_connections`](/docs/reference/config/user/remote/) is false there), SSH rejected credentials or a host key of a box that never connected, the box's host name did not resolve or devlanes is missing there before a first connection, or a starting daemon could not reconnect it. `error` says which. Mirrored ports and hostnames are removed. | `connecting`, after `devlanes remote connect BOX`. |
| `stopped` | The connection is being closed by `devlanes remote disconnect`. | `disconnected`. |
| `disconnected` | A box this machine knows with no connection now: one disconnected with `devlanes remote disconnect`, or a wanted box a starting daemon has not opened yet. | `connecting`, after `devlanes remote connect BOX`. |

A box stays wanted from its first connection until `devlanes remote disconnect`. A starting daemon connects every wanted box again in the background, with no terminal attached, so a prompt there leaves it in `needs_auth` or `needs_pairing`.

## Warnings

`warnings` lists the conditions that do not stop the connection:

| Warning | Meaning | Cleared by |
|---|---|---|
| `the box runs devlanes VERSION and this machine VERSION. Install the same release on both.` | The two machines run different releases. | The same release on both, then a reconnect. |
| ``the box stops your processes at logout. Run `sudo loginctl enable-linger "$USER"` there.`` | The Linux box's user has linger off, so a logout stops the daemon there. | Linger on, then a reconnect. |
| ``the box daemon stops when this SSH session ends. Run `devlanes daemon install` on the box`` | The Windows box's daemon was started by this SSH session and ends with it. | `devlanes daemon install` on the box, then a reconnect. |
| `box BOX's ports.range RANGE overlaps box OTHER (RANGE). ...` | Two connected boxes allocate ports from overlapping ranges, so lanes only one of them runs can get the same ports, and only one is then mirrored. | A separate [`ports.range`](/docs/reference/config/user/ports/) on each box. |
| `this machine cannot use the box's HTTPS port PORT (HOLDER). Open its hostnames on port PORT.` | The box's proxy serves HTTPS on another port than this machine's, and this machine cannot listen on that port: a process holds it, or this user may not bind it. The box's hostnames still open on this machine's own HTTPS port. | The port freed. It is tried again every 30 seconds, on a reconnect and on `devlanes remote connect`. |
| `port PORT is not mirrored because it belongs to OWNER here` | The port lies in the port block of a local lane or of a local project's shared containers. | The local block freed, as when its lane is removed. |
| `port PORT is not mirrored because box BOX (LANE:UNIT) uses it here` | A lane of another connected box has the same port. | That lane stopped or no longer mirrored. |
| `port PORT is not mirrored because HOLDER uses it here` | A process on this machine listens on the port. | The process ended. |
| `port PORT is not mirrored because binding it on ADDRESS failed: ERROR` | Listening on the port failed with nothing holding it, as with a port Windows reserves or a low port. | The cause of the error removed. |

## Mirrored ports

Each entry of `ports` has a `state`:

| State | Meaning |
|---|---|
| `bound` | This machine listens on the port, on `127.0.0.1`, `::1` or both. |
| `in_use` | Something on this machine holds the port. |
| `local_lane` | The port lies in a local lane's port block. |
| `failed` | Listening failed and nothing holds the port. |
| `unbound` | The port is not bound. |

`last_error` shows the last connection to the port that did not reach the box's service, until one does:

| Reason | Message |
|---|---|
| `not_running` | `the unit is not running on the box` |
| `not_registered` | `no running unit on the box has this port`. A hidden unit's port gets this too. |
| `dial_failed` | `nothing on the box accepted it (ERROR)` |
| `timeout` | `the box did not answer in time` |
| `interrupted` | `the link dropped while it opened` |
| `session_lost` | `the link to the box was lost: ERROR` |
| `not_connected` | `box BOX is not connected: ERROR`, after the connection stayed down for the whole wait |
| `needs_auth` | `box BOX is waiting for you to sign in again` |
| `needs_pairing` | `box BOX is waiting for a new pairing code` |

## Routes, skipped lanes and hostnames

Each entry of `routes` is a hostname this machine routes to the box, with the `url` that opens it here and a `state`:

| State | Meaning | Proxy answer |
|---|---|---|
| `ready` | The unit serves. | The request is forwarded. |
| `starting` | The unit is starting. | The request waits, as for a local service. |
| `idle` | The unit does not run. | [DL201 Service not running](../errors/service-idle.md) |
| `failed` | The unit failed. | [DL203 Service failed](../errors/service-failed.md) |
| `reconnecting` | The connection to the box is down. | The request waits, then [DL204 Box reconnecting](../errors/box-reconnecting.md). |
| `sign_in_needed` | The connection is in `needs_auth`. | [DL205 Box sign-in needed](../errors/box-sign-in-needed.md) |
| `pairing_needed` | The connection is in `needs_pairing`. | [DL206 Box pairing needed](../errors/box-pairing-needed.md) |

`skipped_lanes` lists the box's lanes that this machine does not mirror because another lane serves the same hostnames. In the list, each has a `note` that names the lane in use and the command that switches. In `--json`, each has `reason`, `cause`, `winner`, `winner_lane` and `can_use` instead, and `cause` says which lane won:

| Cause | Lane in use |
|---|---|
| `local_running` | A lane of this machine that runs, or whose containers run. |
| `box_chosen` | A lane of another box that `devlanes remote use` chose. |
| `local_stopped` | A stopped lane of this machine. |
| `box_earlier` | The same lane of a box that connected earlier. |

| In the list | In `--json` | Content |
|---|---|---|
| `chosen` | `chosen_lanes` | The lanes `devlanes remote use` chose from this box. |
| `local_lanes_not_in_use` | `yielded_local_lanes`, at the top level, each with `lane` and `winner` | This machine's stopped lanes whose hostnames a box's lane serves. The list adds the command that switches back. |
| `skipped_hosts` | `skipped_hosts` | The box's hostnames this machine does not route: hostnames that are not valid, or not strictly below `localhost` or the configured [`hostnames.tld`](/docs/reference/config/user/hostnames/). |

## Synced folders

Each entry of `syncs` is one synced folder of a box unit, with its `lane`, `unit`, `from` on the box, `to` on this machine, counts of files `written`, `removed`, `pruned` and `compared`, the last 16 per-file `errors`, `error`, `updated_ms` and a `state`:

| State | Meaning |
|---|---|
| `comparing` | Your machine sent its file list and the box compares it. |
| `syncing` | The box sends the files that differ. |
| `watching` | The folder matches the box and follows its changes. |
| `interrupted` | The session ended during a pass. The next session starts it again. |
| `stopped` | The box ended the folder's stream, and the folder is no longer followed. |
| `failed` | Following the folder failed; `error` says why. It is tried again after 1 second, doubling up to 60 seconds. |

Your machine follows at most 16 synced folders per box, and the box keeps its own limit too.

## Machines connected to a box

On the box, `devlanes daemon clients` lists each machine that is paired with it or connected now, connected ones first: its name and id, whether it is paired, how many pairings it has, when it paired and was last seen, and for each connection the address it comes from, since when, the machine's devlanes release, and how many ports it mirrors, how many of them it has bound and how many lanes it skips.

## Timings

| What | Time |
|---|---|
| Ping from your machine to the box | Every 10 seconds |
| A ping without an answer ends the session | After 20 seconds |
| Handshake, and the first message of each new connection through the session | 15 seconds |
| SSH connect timeout | 15 seconds |
| One connection attempt, including the time a person takes to answer prompts | 3 minutes. Then the prompt is dropped and the next attempt waits its turn, as after any failed attempt. |
| An SSH or pairing code prompt nobody answers is cancelled | After 2 minutes |
| `devlanes remote connect --background` returns | Once connected or failed, at most after 150 seconds |
| Wait before the next attempt | 0.5 seconds, doubled after each failure up to 30 seconds, back to 0.5 seconds after a connection |
| A request through the proxy to a box that is reconnecting | Waits up to 15 seconds, then gets DL204 |
| A connection to a mirrored port while the connection is down | Waits up to 10 seconds for the next session |
| `last_error` warning for the same port and reason | At most once a minute |
| A box's HTTPS port that could not be opened is tried again | Every 30 seconds |
| A connection to a box that lost its lane is reset | 300 ms after the box last sent data, at most 5 seconds |
| A local start waits for a box's mirror to free a port the lane needs | 3 seconds, then fails |
| A failed synced folder is tried again | After 1 second, doubled up to 60 seconds |

## Pairing

| Rule | Value |
|---|---|
| Pairing off | With [`remote.require_pairing`](/docs/reference/config/user/remote/) set to false on the box, a machine needs no code or token. |
| Connections off | With [`remote.allow_connections`](/docs/reference/config/user/remote/) set to false on the box, every machine is turned away and `devlanes daemon pair` gives no code. |
| Code | 8 characters, shown as `XXXX-XXXX`, from digits and the letters A to Z without I, L, O and U |
| Typing a code | Case, spaces and the dash do not matter. O reads as 0, I and L as 1. |
| Lifetime of a code | 10 minutes, and one use |
| Codes valid at once | 16. Another code drops the oldest. |
| A wrong, expired or used code | Voids every code that is still valid. |
| Wrong codes | After 5 within 10 minutes, the box refuses every code and hands out none until the oldest of the 5 is 10 minutes old. |
| A refused code or token | Logged in the box's daemon log with the machine's id, name and the address it came from. |
| Token | `dl_rt_` and 256 random bits as hex, given to your machine for the code |
| Token on your machine | Kept per box name in the state database of the devlanes home ([Files and folders](files.md)) |
| Token on the box | Only its SHA-256 is stored, in the box's state database |
| Pairings per machine on one box | 8, one per box name the machine connects by. A ninth drops the oldest, which the box logs. |
| Machine id | `dl_cid_` and 256 random bits as hex, kept in the devlanes home across restarts |
| `devlanes daemon revoke CLIENT` | Takes the id, the name, or a start of the id of at least `dl_cid_` and 4 hex digits that matches one machine. Deletes every pairing of that machine and ends its sessions. |
| `devlanes daemon reset-state` on the box | Deletes every pairing, voids every code and ends every session. Wrong codes still count. |

A daemon with a code still valid does not exit when idle. [Connect to a box](../how-to/remote/connect.md) has the steps.

## What a box and your machine send each other

Your machine sends the box:

- Its devlanes release and which parts of the protocol it supports.
- Its machine id and its host name.
- The token the box gave it, or once a pairing code.
- A ping every 10 seconds.
- What it mirrors, after each change: the name it connects to the box by, its proxy's HTTPS port, whether it serves the box's HTTPS port, each mirrored port with its lane, unit, state, address and what holds it, the lanes it skips and why, and the hostnames it does not route.
- A request to connect to a port of the box, then the bytes of that TCP connection both ways.
- For each synced folder, a request to follow it and the list of files it holds: path, size, permission bits, SHA-256 of the contents and modification time.

The box sends your machine:

- Before it accepts the token or a code: whether it takes connections, whether it needs a pairing code and why, and, when the protocols differ, its protocol version.
- Once it accepts them: its devlanes release, whether linger is off or its daemon ends with the SSH session, and which parts of the protocol it supports.
- The port its proxy serves HTTPS on and its [`ports.range`](/docs/reference/config/user/ports/).
- The token when it accepts a code, or the reason it refuses the code.
- Its lanes, each with project name, lane name, number and domain, and every change to them.
- Each unit of those lanes that is not hidden: its kind (service, container, task or external port), name and state, its ports with names and numbers, and its hostnames with the port each routes to.
- An answer to each ping.
- Whether it accepts each connection to a port, and the bytes of that connection.
- For each synced folder: the service, the source folder's path on the box, its name and its ignore patterns, then the files that differ from your machine's copy, and deletions.

Over SSH, the box also sees the address the connection comes from. Neither machine sends the other environments, variables, logs, commands to run, its CA or the units marked hidden.
