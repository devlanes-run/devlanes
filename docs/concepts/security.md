---
title: Security
description: The daemon answers only your own account, a lane separates ports and hostnames but not processes, the CA signs only local names, and pairing decides which machines see a box's lanes.
kind: concept
---

devlanes runs as your user. Its daemon answers processes of your account and no other, and each service and task runs with your permissions. Lanes keep ports, hostnames and data apart, but they do not keep code apart, and that shapes what you can trust devlanes with.

## Only your account reaches the daemon

The daemon listens on a socket, a named pipe on Windows, that only your account can open, and it turns away callers from other accounts. There is no token or password: the account check is the whole of it.

![Who is checked where: processes running as you reach the daemon's socket or pipe and the CA key, which admit your account only; a sandbox's edge decides whether a process inside it gets through; SSH or a command decides who reaches a box, and the pairing check decides which machines see the box daemon's lanes](security.svg)

So any process running as you can do what you can through the daemon: start and stop services, run commands in a lane, read logs and a lane's environment, your shell's secrets included. A dev server, or a dependency's install script, runs as you too and can do the same. A sandboxed process that is allowed to reach the daemon gets the daemon's rights, outside its sandbox; [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md) covers that case.

## A lane is not a sandbox

A lane has its own ports, hostnames, data folder and cookie site, so two lanes do not collide. Its processes still belong to the same user as every other lane's: a service in one lane can read another lane's files and connect to its ports. Code you do not trust belongs in a container or a separate account, not in a lane.

## The CA signs only local names

devlanes creates one CA for your user, with its key in `ca-key.pem` in the `ca` folder of the devlanes home, readable by your account only. Any process running as you can read that key and use it to make certificates your browsers trust. The CA is limited to local names, so a client that checks that limit refuses a certificate it signs for any name outside these:

- `localhost`
- `test`
- `internal`
- `home.arpa`
- the top-level domain in `hostnames.tld`, as set when the CA was created

In such a client, trusting the CA does not let it vouch for a real website. [Hostnames and HTTPS](hostnames-and-https.md) describes how the CA gets trusted.

## Pairing decides which machines see a box

A box is a machine whose lanes are mirrored to yours. SSH, or the command that reaches a box, decides who gets onto the box at all. Pairing decides which of the machines that get in see its lanes: the first time your machine connects, you give it a one-time code from the box, and it trades the code for a token it presents on each later connection. Revoking a machine on the box deletes its pairing and ends its connections.

Under ordinary SSH access, pairing is not a boundary. A machine that can open a shell on the box as you can hand itself a code, and so can an agent working there. Pairing protects the box's lanes only when a machine's SSH key on the box is restricted to running the devlanes tunnel.

The token is a bearer secret: any process running as you on the paired machine can copy it and connect as that machine. Revoking a machine leaves its SSH access as it was, so remove its key from the box as well. [Connect to a box](../how-to/remote/connect.md) has the steps.

A box's services see your connections as coming from the box itself. A service there that trusts local connections without a password, such as an admin page or a database, trusts every machine the box serves.

## The daemon looks for new releases

The daemon reads the latest version from the registry devlanes was installed from. The request carries nothing that identifies your machine or your projects. [Update and uninstall](../how-to/update.md) says how to turn the check off.

## Related

- [Use devlanes from an agent sandbox](../how-to/agents/sandboxes.md)
- [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Remote mirroring](remote-mirroring.md)
