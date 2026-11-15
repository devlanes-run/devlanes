---
title: Remote mirroring
description: Your machine connects to a box and serves the box's lanes on the same hostnames and ports, over one session that carries traffic and no commands.
kind: concept
---

A box is a machine that runs devlanes for its own lanes. When your machine connects to it, the box's lanes appear on your machine under the hostnames and port numbers the box gave them. You start and stop those lanes on the box itself, and the connection carries what they serve, not what controls them.

## Two daemons share one session

Each machine runs its own daemon. The daemon on your machine owns the connection: it opens the session, listens on the box's ports, adds the box's hostnames to its proxy and writes the folders the box syncs. The daemon on the box runs the box's lanes as it would with nobody connected.

![A request from the browser on your machine goes through your proxy and a mirrored port, over one session by SSH or a command, to the devlanes tunnel on the box and the lane's service there; environments, secrets, logs, commands and either CA never cross](remote-mirroring.svg)

On the box, SSH starts one devlanes tunnel per connection, and the tunnel talks to the box daemon the way a local command would. A box without SSH, such as a container or a WSL distribution, is reached by a command that starts the same tunnel. Because the session belongs to your daemon and not to a terminal, it outlives the terminal that opened it and comes back after your daemon restarts. Both ends must speak the same protocol version, and devlanes warns when their releases differ, so run the same release on the box and on your machine; [Prepare a box](../how-to/remote/prepare-a-box.md) covers installing one.

## The session carries traffic, not control

The box sends a description of its lanes and of their units that are not hidden, then the bytes of each connection you make to them and the files of each synced folder; your machine sends who it is, its pairing token and a report of what it mirrors. A synced folder goes one way, from the box to your machine, so something the box builds, such as a browser extension you load unpacked, appears on your machine as it changes.

Environments, values from env files, secrets, logs, terminal output, commands and either machine's CA do not cross. The session has no request that runs a daemon command, so your machine cannot start a unit on the box, read its logs or run a command there through it. To change a lane, you work on the box, over SSH or in an editor's remote terminal. [Remote connection states](../reference/remote-states.md) lists everything each side sends, and [Security](security.md) says what each side checks.

## A box keeps its hostnames and ports

The box's services got their URLs when they started, and those URLs end up in their config, in CORS lists and in cookies. Your machine passes them through unchanged: each port listens on the loopback addresses under the same number, and each hostname joins your proxy, which serves it with your CA and sends the request on to the box. A browser on your machine trusts the box's hostnames once your machine trusts its own CA.

Renaming the hostnames or moving the ports on your machine would break each URL the box already handed out. When the box serves HTTPS on another port than your machine does, its URLs carry that port, so your machine listens on it as well for that box's hostnames. Box hostnames are accepted only below `localhost` or the top-level domain your machine resolves itself, so a box cannot make your CA sign a name of its choosing.

A port your machine already uses, or one inside a local lane's block of ports, stays local and is reported instead of mirrored. For you that means a URL or a client that names that port number reaches your own program, or nothing, instead of the box. The box's hostnames still work, because your proxy reaches the box without that port.

## One box serves a lane at a time

Two boxes made from one image run the same lane with the same hostnames and ports, and a box lane can claim the hostnames of a lane on your machine. A hostname could be routed by name to either one, but a port number on your machine leads to exactly one place. So one side serves the lane whole, ports included, and the other is skipped whole rather than renamed, since a renamed hostname would break the origins and cookies the box chose.

A lane running on your machine wins, then the box lane you chose, then a stopped local lane, then the box that connected first. A lane you stopped here therefore keeps its hostnames until you choose the box's lane with `devlanes remote use`. Switching moves the lane's hostnames and ports to the other box in one step, and ends the connections to the old one once their answers are through. [Work with several boxes](../how-to/remote/several-boxes.md) covers choosing and switching.

## A dropped session keeps its ports and routes

Your daemon notices within seconds when the box stops answering, and keeps reconnecting, waiting longer between tries up to a limit. Meanwhile the mirrored ports stay bound and the hostnames keep their routes, so no other program on your machine takes them over. Connections that were open through the lost session, an open WebSocket among them, end with it.

A request to one of the box's hostnames waits a few seconds for the session to come back, then gets a page that asks you to reload in a moment. A connection made straight to a mirrored port waits too, then is closed. Once the session is back, the box's current state replaces what your machine knew and synced folders are compared again. A reconnect that needs you, to sign in or to pair again, waits for you instead of retrying; [Remote connection states](../reference/remote-states.md) lists those states and the times involved.

## Related

- [Lanes on another machine](../learn/remote.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Choose what a box shares](../how-to/remote/what-a-box-shares.md)
- [Work with several boxes](../how-to/remote/several-boxes.md)
