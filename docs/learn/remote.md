---
title: Lanes on another machine
description: The lanes of myapp run on a box, and the browser on your machine opens them under the same hostnames and port numbers.
kind: learn
---

myapp does not have to run on the machine in front of you. Its lanes can run on a VPS or in a container, and the browser on your machine still opens them under the hostnames and port numbers they have there, with no port forwarded by hand.

## A box runs its own lanes

A box is a machine with devlanes installed whose lanes another machine mirrors. For myapp it is my-vps, a container that your machine reaches with Docker's exec command. The first devlanes command inside it starts a daemon there, and that daemon runs myapp as a daemon on a laptop would.

```console title="on your machine"
$ docker exec my-vps devlanes start --no-ui
ok install
myapp@0 · main
ok services  2 of 2 ready
  ok  web    https://myapp.localhost
  ok  admin  https://admin.myapp.localhost
```

The install task ran first, as it does in any new lane, then both services became ready. The box runs the same devlanes release as your machine. A box whose release speaks another protocol cannot connect, and [Prepare a box](../how-to/remote/prepare-a-box.md) covers installing the right one.

## One connection joins the two machines

Your machine's daemon opens the connection and keeps it. It goes over SSH, or, for a box without SSH, through a command such as Docker's exec that runs devlanes inside the box. The box sends its lanes, ports, hostnames and states over it, and opens no port of its own for it.

![my-vps and your machine joined by one connection; the browser on your machine opens https://myapp.localhost and reaches web on port 41001 on the box](remote.svg)

Nothing on the connection starts or stops a unit. Starting, stopping and reading logs happen on the box, in a shell there, and when web is stopped on my-vps your browser gets an error page naming the command to run on the box. Environments and secrets stay on the box as well; [Remote mirroring](../concepts/remote-mirroring.md) lists what crosses.

## Your machine pairs with the box once

One entry in your user config tells your machine how to reach my-vps, here the Docker command that runs the box's end of the connection. The box serves only machines it has paired with, so the first connection carries a one-time code that the box hands out.

```console title="on your machine"
$ devlanes config set remote.boxes.my-vps.command '[docker, exec, -i, my-vps, devlanes, tunnel, --stdio]'
$ docker exec my-vps devlanes daemon pair
$ devlanes remote connect my-vps --background --code BQGA-0RH8
```

The second command printed the code, and the third traded it for a token that your machine keeps. Later connections use the token, also after your daemon restarts, so the code is needed once. [Connect to a box](../how-to/remote/connect.md) has the steps, and [Security](../concepts/security.md) what pairing protects.

## Your machine shows the box's hostnames and ports

Once connected, `https://myapp.localhost` opens in the browser on your machine and reaches web inside my-vps. Your daemon listens on the box's port numbers on its own loopback, and it adds the box's hostnames to its proxy with certificates from your own CA, so the box needs no certificate setup of its own.

```console title="on your machine"
$ devlanes remote ls
items:
  state: connected
  server: devlanes 0.11.3
  host: my-vps
  transport: command
  https_port: 443
  lanes: 1
  ports:
    port: 41001
    lane: myapp@0
    unit: web
    kind: service
    state: bound
    addresses: 127.0.0.1, ::1
```

Port 41001 is the port web has on the box. The box's user config gives its lanes ports from 41000 to 41999, outside the pool your machine uses, so the same number is free on your machine. The admin service is missing from the list because the box's local file marks it hidden, which keeps a service on the box.

A lane on your own machine with the same hostnames keeps them while it runs. A stopped one keeps them too, unless you choose the box's lane, which [Work with several boxes](../how-to/remote/several-boxes.md) covers.

## On your own project

- [Prepare a box](../how-to/remote/prepare-a-box.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Connect to a box without SSH](../how-to/remote/connect-by-command.md)
- [Choose what a box shares](../how-to/remote/what-a-box-shares.md)
- [Remote mirroring](../concepts/remote-mirroring.md)
