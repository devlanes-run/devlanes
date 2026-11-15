---
title: Remote mirroring
description: Your machine connects to a box and serves the box's lanes on the same hostnames and ports, over one session that carries traffic and no commands.
kind: concept
---

A box is a machine that runs devlanes for its own lanes. When your machine connects to it, the box's lanes appear on your machine under the hostnames and port numbers the box gave them, and their build folders can follow. You start and stop those lanes on the box itself; the connection carries what the lanes serve and nothing that controls them.

## Two daemons share one session

Each machine runs its own daemon. The daemon on your machine owns the connection: it opens the SSH session, listens on the box's ports, adds the box's hostnames to its proxy and writes the folders the box syncs. The daemon on the box runs the box's lanes as it would with nobody connected.

![Your machine and a box joined by one SSH session: your daemon holds the proxy, mirrored ports and synced folders; on the box a devlanes tunnel talks to the box daemon, which runs the lanes](remote-mirroring.svg)

On the box, SSH starts one devlanes tunnel per connection, and the tunnel talks to the box daemon the way a local command would. Because the session belongs to your daemon and not to a terminal, it survives the terminal that opened it and comes back after your daemon restarts. A box without SSH, such as a container or a WSL distribution, is reached by a command that starts the same tunnel. Both ends must speak the same protocol version, and devlanes warns when the releases differ, so run the same release on the box and on your machine; [Prepare a box](../how-to/remote/prepare-a-box.md) covers installing one.

## The session carries traffic, not control

The box sends its version, the port its proxy serves HTTPS on and its port range, then a description of its lanes: projects, lane numbers and domains, and for each unit that is not hidden its name, kind, state, ports and hostnames. After that come the bytes of each connection you make to one of those ports, and the files of each synced folder. Your machine sends its version, its client id and host name, its pairing token, the files it already holds in each synced folder, and a report of what it mirrors.

Environments, values from env files, secrets, logs, terminal output, commands and either machine's CA do not cross. The session has no request that runs a daemon command, so your machine cannot start a unit on the box, read its logs or run a command there through it. That keeps one owner for each lane. Anything that changes a lane happens on the box, over SSH or in an editor's remote terminal, and what each side checks on the way is in [Security](security.md).

## A box keeps its hostnames and ports

The box's services got their URLs when they started, and those URLs end up in their config, in CORS lists and in cookies. Your machine passes them through unchanged: each port listens on the loopback addresses under the same number, and each hostname joins your proxy, which serves it with your CA and sends the request on to the box as a plain stream. A browser on your machine then trusts the box's hostnames once your machine trusts its own CA.

Renaming the hostnames or moving the ports on your machine would break each URL the box already handed out. When a box can only serve HTTPS on its fallback port, its URLs carry that port, so your machine listens on it as well for that box's hostnames. A port your machine already uses, or one inside a local lane's port block, stays local and is reported instead of mirrored. Box hostnames are accepted only below `localhost` or the top-level domain your machine resolves itself, so a box cannot make your CA sign a name of its choosing.

## One box serves a lane at a time

Two boxes made from one image run the same lane with the same hostnames and ports, and a box lane can claim the hostnames of a lane on your machine. A hostname could be routed by name to either one, but a port number on your machine leads to exactly one place. So one side serves the lane whole, ports included, and the other is skipped whole rather than renamed, since a renamed hostname would break the origins and cookies the box chose.

A lane running on your machine wins. Then comes the box lane you chose, then a stopped local lane, then the box that connected first. Switching to another box moves the lane's hostnames and ports in one step and closes the open connections to the old one. [Work with several boxes](../how-to/remote/several-boxes.md) covers choosing and switching.

## While the connection is down

Your daemon pings the box every 10 seconds and treats 20 seconds of silence as a lost session. It reconnects with a backoff that starts at half a second and grows to 30 seconds. Meanwhile the mirrored ports stay bound and the hostnames keep their routes, so nothing on your machine takes them over.

A request through the proxy to one of the box's hostnames waits up to 15 seconds for the session to come back, then gets a page that says the box is reconnecting. A connection made straight to a mirrored port waits up to 10 seconds, then is closed. Once the session is back, the box's current state replaces what your machine knew and synced folders are compared again. A reconnect that needs you, to sign in or to pair again, waits for you instead of retrying, and [Remote connection states](../reference/remote-states.md) lists those states.

## Related

- [Lanes on another machine](../learn/remote.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Choose what a box shares](../how-to/remote/what-a-box-shares.md)
- [Work with several boxes](../how-to/remote/several-boxes.md)
