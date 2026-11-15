---
title: Lanes on another machine
description: myapp moves onto a box, and the browser on your machine opens its lanes under the same hostnames and port numbers.
kind: learn
---

myapp moves off your machine onto a box, a machine that runs lanes of its own which another machine mirrors. Here the box is a container named devbox, with a Docker engine inside it, reached with Docker's exec command. A VPS reached over SSH works the same way. The lanes run on the box, and the browser on your machine opens them under the hostnames they had before.

## The lanes on your machine are cleaned first

A clean stops a lane, runs its teardown and deletes its data. In myapp the teardown drops the lane's database, so each clean takes one database out of the shared Postgres.

```console title="~/dev/myapp"
$ devlanes lane clean 2 --yes
cleaned myapp@2: stopped 0 units, teardown database
$ devlanes lane clean 1 --yes
cleaned myapp@1: stopped 2 units, teardown database
$ devlanes lane clean 0 --yes
cleaned myapp@0: stopped 2 units, teardown database
  stopped shared containers (data kept)
```

Lane 2 had nothing left to stop after the agent's stop. With lane 0 gone, no lane of myapp uses the shared container, so the last clean stops it and keeps its volume. [Remove lanes](../how-to/lanes/clean-up.md) lists what a clean deletes.

## The box runs the same start

The box holds the repository on the main branch. Its start is the one your machine ran before: the shared container, setup, the install task, the migration, then the services. Postgres runs on the box's own Docker engine.

```console title="on the box"
$ devlanes start --no-ui
✓ infra
✓ setup
✓ install
✓ migrate
✓ services ready
myapp@0 · main
✓ services  2 of 2 ready
  ✓  api  https://api.myapp.localhost
  ✓  web  https://myapp.localhost
```

The box's user config gives its lanes ports from a range of their own, outside the blocks your machine hands out. A port number the box uses is then free on your machine as well.

```yaml devlanes-user
ports:
  range: 28000-32767
```

## Your machine pairs with the box once

The box serves only machines it has paired with. On the box, `devlanes daemon pair` prints a code that works once.

```console title="on the box"
$ devlanes daemon pair
pairing code: V7G3-AFH2
it works once and expires in 10 minutes
on your other machine, run: devlanes remote connect <box> --code V7G3-AFH2
```

Your machine gets one entry in its user config: the command that runs the box's end of the connection. The first connect carries the code and trades it for a token that your machine keeps, so later connections need no code.

```console title="~/dev/myapp, on Windows 11"
$ devlanes config set remote.boxes.devbox.command '[docker, exec, -i, devbox, devlanes, tunnel, --stdio]'
set remote.boxes.devbox.command to ["docker","exec","-i","devbox","devlanes","tunnel","--stdio"] in C:\Users\you\.devlanes\config.yaml; takes effect with the next devlanes command
$ devlanes remote connect devbox --background --code V7G3-AFH2
state: connected
server: devlanes 0.13.0
host: devbox
transport: command
lanes: 1
ports: -
routes: -
```

The connection belongs to your machine's daemon, which keeps it open after the command returns, and devlanes opens no port on the box for it. Both machines run the same release; [Prepare a box](../how-to/remote/prepare-a-box.md) says how closely releases have to match.

## This machine's lane 0 kept its hostnames

The connection came up with no ports and no routes. `devlanes remote ls` lists each box your machine knows, and its entry for devbox says why.

```console title="~/dev/myapp"
$ devlanes remote ls
items:
  state: connected
  host: devbox
  lanes: 1
  skipped_lanes:
    lane: myapp@0
    hosts: api.myapp.localhost, myapp.localhost
    note: not mirrored because this machine's myapp@0 is in use. devlanes remote use devbox myapp@0 switches.
```

The clean did not remove lane 0 for good. The main worktree's lane comes back, stopped, the next time a command acts on the project's lanes. A stopped lane on your machine keeps its hostnames over a box's lane, until the note's command hands them over.

```console title="~/dev/myapp"
$ devlanes remote use devbox myapp@0
myapp@0 now comes from devbox. This machine's lane gets the hostnames back when it starts.
```

`devlanes remote use --local myapp@0` gives them back without a start.

## Your machine serves the box's hostnames on the box's ports

The hostnames now lead to the box's lane, on the port numbers its services have there. Your daemon listens on those numbers on its own loopback, and your proxy serves the hostnames with certificates from your own CA, so the box needs no certificate of its own.

```console title="~/dev/myapp"
$ devlanes url ls
HOST                     TARGET       PORT   STATE
api.myapp.localhost  ->  myapp@0:api  28000  ready
myapp.localhost      ->  myapp@0:web  28001  ready
```

Opened in the browser on your machine, `https://myapp.localhost` makes one request through your proxy and the connection to the web app on devbox.

![The browser on your machine opens https://myapp.localhost; your proxy sends the request over one connection to devbox, a container, where web in myapp lane 0 answers as myapp.localhost on port 28001](remote.svg)

Nothing on the connection starts or stops a unit. Starting and stopping happen on the box, in a shell there, and environments and secrets stay on the box too. [Remote mirroring](../concepts/remote-mirroring.md) lists what crosses the connection.

## On your own project

- [Prepare a box](../how-to/remote/prepare-a-box.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Connect to a box without SSH](../how-to/remote/connect-by-command.md)
- [Work with several boxes](../how-to/remote/several-boxes.md)
- [Choose what a box shares](../how-to/remote/what-a-box-shares.md)
- [Remote mirroring](../concepts/remote-mirroring.md)
