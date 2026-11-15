---
title: Security
description: The daemon answers only your own account, a lane separates ports and hostnames but not processes, the CA signs only local names, and pairing decides which machines see a box's lanes.
kind: concept
---

devlanes runs as your user. Its daemon answers only processes of the account that runs it, and a box hands a connected machine only what its lanes serve. Lanes keep ports, hostnames and data apart, but they do not keep code apart: each unit runs with your permissions.

## Only your account reaches the daemon

On macOS and Linux the daemon listens on a socket in a directory only you can open. On each connection it compares the peer's user with its own and drops anyone else without an answer, and the client checks the daemon the same way before it sends anything. On Windows the endpoint is a named pipe whose access list names your account alone. The pipe refuses clients from other machines, and its integrity label also refuses your own low-integrity processes, which is how most sandboxed programs run.

The endpoint has no token. The user check already limits it to your account, and a token file that only you can read would be readable by exactly the same processes. [Files and folders](../reference/files.md) lists where the endpoint and the rest of the devlanes home live.

## A process running as you can do what you can

Any process of your account that reaches the daemon can use each of its commands. It can start and stop units, run a command in a lane, type into a unit's terminal, read logs and a lane's environment, including the variables of the shell that started each unit, stop the daemon, and on a box hand out pairing codes. Units run as you too, so a dev server or a dependency's install script can do all of this.

Each request carries a label saying whether a person or an agent sent it. The client sets that label and the daemon records it without checking it, so it tells you who probably acted and proves nothing.

A sandboxed process that is allowed to reach a daemon started outside its sandbox gets that daemon's rights, and what it asks for runs outside the sandbox. A command that looks sandboxed does not start a daemon of its own, and [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md) covers that case. On Windows, a daemon started from an elevated terminal runs each unit as administrator, and `devlanes doctor` warns about it.

## A lane is not a sandbox

A lane has its own ports, hostnames, data folder and cookie site, so two lanes do not collide. They are still processes of one user on one machine. A service in one lane can read the files of another lane's worktree, connect to its ports and talk to the daemon like any other process of yours.

Sharing your account is what lets a lane use your tools, caches and SSH keys as they are, and keeping code apart would take a container or another account. Code you do not trust belongs in a container or a separate account, not in a lane.

## The CA signs only local names

devlanes creates one CA per user, in the devlanes home, with a key that only you can read. The daemon refuses to load a key that other accounts can read or that is a link. The CA is valid for 10 years and name-constrained, so a client that checks name constraints refuses a certificate it signs for any name outside these:

- `localhost`
- `test`
- `internal`
- `home.arpa`
- the top-level domain in `hostnames.tld`, as set when the CA was created

The proxy signs a certificate during the handshake, for the name the browser asked for, and only when a route has that name. The daemon does not write to a trust store; `devlanes setup` does that in your terminal, as [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md) describes. Anyone who can read the key can make certificates your browsers trust for those names. Choosing mkcert's root in its place gives up the name constraints, since mkcert's root has none.

## Pairing decides which machines see a box

SSH, or the command that reaches a box, decides who gets onto the box at all. Pairing decides which of the machines that get in see the box's lanes. A machine's first connection needs a one-time code from the box, which it trades for a token it presents on each later connection. These rules hold for codes and tokens:

- A code has 8 characters and 40 random bits, works once, expires after 10 minutes and exists only in the box daemon's memory.
- A wrong code voids each outstanding code, so a guesser gets one try per code.
- After 5 wrong codes within 10 minutes, across all of the box's connections, the box accepts and hands out no code until the oldest of those is 10 minutes old.
- A token has 256 random bits. The box stores only its hash and compares hashes and codes in constant time.
- The box keeps up to 8 pairings per machine and drops the oldest beyond that.
- A machine's id and host name are what it says they are, and neither counts as a credential.

Until the box accepts its token or code, a machine gets no lanes, no version and no details about the box. Revoking a machine on the box deletes its pairings and ends its connections at once. [Connect to a box](../how-to/remote/connect.md) has the steps.

## Pairing does not replace SSH

Anyone with a shell on the box as you can hand out a code, and so can an agent working there. With ordinary SSH access, a machine that can run the tunnel can also run that command, so pairing adds protection only when the machine's key allows nothing but the tunnel. A key restricted that way in the box's authorized keys, used through a box command, is the case where pairing keeps the box's lanes from a machine that gets in.

A token is a bearer secret. Any process running as you on the paired machine can copy it with the machine's id and connect from anywhere SSH lets it reach the box. Revoking a machine leaves its SSH access as it was, so remove its key as well.

Someone who reaches the tunnel without a code can still send wrong ones, which voids your codes and can pause pairing for 10 minutes. With pairing turned off, every machine that SSH lets in gets the box's lanes and their traffic. An invalid value in the box's [`remote`](/docs/reference/config/user/remote/) settings counts as the safe one: connections off, pairing on, and no daemon started by a connection.

## A box learns who connects

Anyone who runs commands on the box as you, agents included, can see each connected machine's id, host name, SSH address and connection time. From the report each machine sends about what it mirrors, they also see the names of that machine's local lanes and the name and PID of each program there that holds one of the box's ports. Paired machines stay listed with their names and when they last connected. [Remote mirroring](remote-mirroring.md) lists what else crosses the connection.

A connection that a box service gets from your machine arrives from the box's own loopback address. A service that trusts loopback without asking for a password, such as an admin page, a debugger port or a local database, trusts each machine the box serves. On your machine the mirrored ports and the proxy listen on loopback, so other users of your machine can connect to them, as they can to your local services.

## Related

- [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md)
- [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Choose what a box shares](../how-to/remote/what-a-box-shares.md)
