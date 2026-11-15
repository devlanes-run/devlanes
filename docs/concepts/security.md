---
title: Security
description: The daemon answers only your own account, a lane separates ports and hostnames but not processes, the CA signs only local names, pairing decides which machines see a box's lanes, and devlanes goes on the network by itself only for a release check and one lookup at each daemon start.
kind: concept
---

devlanes runs as your user. Its daemon answers processes of your account and no other, and a box, a machine whose lanes are mirrored to yours, hands your machine what its lanes serve and a description of them, nothing that controls them. Lanes keep ports, hostnames and data apart, but each service and task runs with your permissions.

## Only your account reaches the daemon

On macOS and Linux the daemon listens on a socket in a folder only you can open. It compares each caller's user with its own and drops anyone else without an answer, and the client checks the daemon the same way before it sends anything. On Windows the endpoint is a named pipe whose access list names your account alone; [Windows](../reference/windows.md) has what else it refuses. There is no token or password: the account check is the whole of it.

![Who is checked where: processes running as you reach the daemon's socket or pipe and the CA key, which admit your account only; a sandbox's edge decides whether a process inside it gets through; SSH or a command decides who reaches a box, and the pairing check decides which machines see the box daemon's lanes](security.svg)

## A process running as you can do what you can

Any process of your account that reaches the daemon can do what you can:

- start and stop services and tasks, and run a command in a lane
- type into a terminal attached to a running service
- read logs and a lane's environment, your shell's secrets included
- stop the daemon
- on a box, hand out the one-time codes that let another machine see its lanes

Services and tasks run as you too, so a dev server or a dependency's install script can do all of this, and can read the CA's key.

A sandboxed process that is allowed to reach a daemon started outside its sandbox gets that daemon's rights, and what it asks for runs outside the sandbox. devlanes refuses to start a daemon from inside a sandbox it recognises; [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md) covers that case.

## A lane is not a sandbox

A lane has its own ports, hostnames, [data folder](../reference/glossary.md) and cookie site, but its processes belong to the same user as every other lane's. A service in one lane can read the files of another lane's worktree, connect to its ports and talk to the daemon like any other process of yours. Code you do not trust belongs in a container or a separate account, not in a lane.

## The CA signs only local names

devlanes creates one CA for your user, with its key in `ca-key.pem` in the `ca` folder of the devlanes home, readable by your account only. Any process running as you can read that key and use it to make certificates your browsers trust. The CA is name-constrained, so a client that checks name constraints refuses a certificate it signs for any name outside these:

- `localhost`
- `test`
- `internal`
- `home.arpa`
- the top-level domain in `hostnames.tld`, as set when the CA was created

[Hostnames and HTTPS](hostnames-and-https.md) describes when the proxy signs a certificate and how the CA gets trusted.

## Pairing decides which machines see a box

SSH, or the command that reaches a box, decides who gets onto the box at all. Pairing decides which of the machines that get in see the box's lanes: the first time your machine connects, you give it a one-time code from the box, and it trades the code for a token it presents on each later connection.

A code works once and expires soon after the box shows it. A wrong code voids each outstanding code, so a guesser gets one try per code, and repeated wrong codes pause pairing. The box stores only a hash of each token, and a machine's id and name are its own claims, not credentials.

Until the box accepts its token or code, a machine sees no lanes and nothing about the box beyond whether it takes connections and, when the two differ, its protocol version. Revoking a machine on the box deletes its pairings and ends its connections at once. [Connect to a box](../how-to/remote/connect.md) has the steps, and [Remote connection states](../reference/remote-states.md) the numbers.

## Under ordinary SSH access, pairing is not a boundary

A machine that can open a shell on the box as you can hand itself a code, and so can an agent working there. Pairing protects the box's lanes only when a machine's SSH key on the box is restricted to running the tunnel.

A token is a bearer secret. Any process running as you on the paired machine can copy it, with the machine's id, and connect from anywhere SSH lets it reach the box. Revoking a machine leaves its SSH access as it was, so remove its key as well.

Someone who reaches the tunnel without a code can still send wrong ones, which voids your codes and pauses pairing. With pairing turned off, every machine that SSH lets in gets the box's lanes and their traffic. A value in the box's [`remote`](/docs/reference/config/user/remote/) settings that devlanes cannot read counts as the safe value of that setting.

## A box learns who connects

Anyone who runs commands on the box as you, agents included, can see which machines are connected and what each one fails to mirror, as [Remote connection states](../reference/remote-states.md) lists. A connection that a box service gets from your machine arrives from the box's own loopback address, so a service that trusts loopback without a password, such as an admin page or a local database, trusts each machine the box serves. On your machine the mirrored ports and the proxy listen on loopback too, so other users of your machine can reach them, as they can your local services.

## devlanes goes on the network for two things

Shortly after it starts, then every few hours and when a TUI opens, the daemon asks the registry devlanes was installed from for the latest version, and reads a newer release's changelog from GitHub. The requests carry no identifier of your machine or your projects. Nothing else is downloaded unless [`updates.mode`](/docs/reference/config/user/updates/) asks for patch releases to be installed in the background.

The same setting turns the checks off, and so do `DEVLANES_NO_UPDATE_CHECK=1` and a CI environment, each read when the daemon starts: a daemon already running goes on checking until it restarts. At each start, unless `hostnames.hosts_file` turns the hosts file off, the daemon also looks up one random name under localhost to see whether your system answers such names itself; where it does not, the query goes on to your DNS server. Other traffic goes where you sent it: to the boxes you connected to, which your daemon reconnects after a restart, and to an update you run.

## Related

- [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md)
- [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Remote mirroring](remote-mirroring.md)
