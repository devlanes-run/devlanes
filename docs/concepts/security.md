---
title: Security
description: The daemon answers only your own account, a lane separates ports and hostnames but not processes, the CA signs only local names, pairing decides which machines see a box's lanes, and apart from the boxes you connect to, devlanes goes on the network only to check for releases.
kind: concept
---

devlanes runs as your user. Its daemon answers processes of your account and no other, and a box hands a connected machine what its lanes serve and a description of them, nothing that controls them. Lanes keep ports, hostnames and data apart, but each unit runs with your permissions.

## Only your account reaches the daemon

On macOS and Linux the daemon listens on a socket in a folder only you can open. It compares each caller's user with its own and drops anyone else without an answer, and the client checks the daemon the same way before it sends anything. On Windows the endpoint is a named pipe whose access list names your account alone. The pipe refuses callers from other machines, and its integrity label also refuses your own low-integrity processes, which is how sandboxed programs usually run.

![Who is checked where: the socket or pipe admits only your account, a sandbox edge decides whether a sandboxed process reaches it, SSH or a command decides who reaches a box, and the pairing check decides which machines see the box daemon's lanes](security.svg)

The endpoint has no token or password; the account check is the whole of it. [Files and folders](../reference/files.md) lists where it lives.

## A process running as you can do what you can

Any process of your account that reaches the daemon can do what you can: start and stop units, run a command in a lane, type into a unit's terminal, read logs and a lane's environment, stop the daemon, and on a box hand out pairing codes. Units run as you too, so a dev server or a dependency's install script can do all of this, and can read the CA's key. The label that marks a request as a person's or an agent's is set by the client and not checked.

A sandboxed process that is allowed to reach a daemon started outside its sandbox gets that daemon's rights, and what it asks for runs outside the sandbox. devlanes refuses to start a daemon from inside a sandbox it recognises; [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md) covers that case.

## A lane is not a sandbox

A lane has its own ports, hostnames, data folder and cookie site, so two lanes do not collide. They are still processes of one user on one machine. A service in one lane can read the files of another lane's worktree, connect to its ports and talk to the daemon like any other process of yours. Code you do not trust belongs in a container or a separate account, not in a lane.

## The CA signs only local names

devlanes creates one CA for your user, with its key in `ca-key.pem` in the `ca` folder of the devlanes home, readable by your account only. Any process running as you can read that key and use it to make certificates your browsers trust for local names. The CA is name-constrained, so a client that checks name constraints refuses a certificate it signs for any name outside these:

- `localhost`
- `test`
- `internal`
- `home.arpa`
- the top-level domain in `hostnames.tld`, as set when the CA was created

The proxy signs a certificate during the handshake, for the name the browser asked for, and only when a route has that name; for any other name the handshake is refused. The daemon does not write to a trust store. `devlanes setup` does that in your terminal, as [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md) describes.

## Pairing decides which machines see a box

SSH, or the command that reaches a box, decides who gets onto the box at all. Pairing decides which of the machines that get in see the box's lanes. Your machine's first connection needs a one-time code from the box, which it trades for a token it presents on each later connection.

A code works once and expires within minutes. A wrong code voids each outstanding code, and repeated wrong codes pause pairing for a while. The box stores only a hash of each token, and keeps one pairing for each name your machine reaches it by. A machine's id and name are what it says they are, so neither counts as a credential.

Until the box accepts its token or code, a machine gets no lanes, no release and no details about the box beyond whether it takes connections and, when the two differ, its protocol version. Revoking a machine on the box deletes its pairings and ends its connections at once. [Connect to a box](../how-to/remote/connect.md) has the steps, and [Remote connection states](../reference/remote-states.md) the numbers.

## Under ordinary SSH access, pairing is not a boundary

A machine that can open a shell on the box as you can hand itself a code, and so can an agent working there. Pairing protects the box's lanes only when a machine's SSH key on the box is restricted to running the tunnel.

A token is a bearer secret. Any process running as you on the paired machine can copy it, with the machine's id, and connect from anywhere SSH lets it reach the box. Revoking a machine leaves its SSH access as it was, so remove its key as well.

Someone who reaches the tunnel without a code can still send wrong ones, which voids your codes and pauses pairing. With pairing turned off, every machine that SSH lets in gets the box's lanes and their traffic. When the box's [`remote`](/docs/reference/config/user/remote/) settings hold a value devlanes cannot read, it takes the safe one: connections off, pairing on, and no daemon started by a connection.

## A box learns who connects

Anyone who runs commands on the box as you, agents included, can see which machines are connected, with each one's name, release and SSH address, and which of the box's ports and lanes each one cannot mirror; [Remote connection states](../reference/remote-states.md) lists all of it.

A connection that a box service gets from your machine arrives from the box's own loopback address. A service that trusts loopback without a password, such as an admin page, a debugger port or a local database, trusts each machine the box serves. On your machine the mirrored ports and the proxy listen on loopback, so other users of your machine can connect to them, as they can to your local services.

## devlanes checks for releases by itself

Shortly after it starts, then every few hours and when a TUI opens, the daemon asks the registry devlanes was installed from for the latest version, and reads a newer release's changelog from GitHub. The requests carry no identifier of your machine or your projects.

Nothing else is downloaded unless [`updates.mode`](/docs/reference/config/user/updates/) asks for patch releases to be installed in the background. The same setting turns the checks off, and so do `DEVLANES_NO_UPDATE_CHECK=1` and a CI environment. Other traffic goes where you sent it: to the boxes you connected to, which your daemon reconnects after a restart, and to an update you run.

## Related

- [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md)
- [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Remote mirroring](remote-mirroring.md)
