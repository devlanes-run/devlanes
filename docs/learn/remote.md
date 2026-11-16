---
title: Lanes on another machine
description: myapp moves onto a box, and the browser on your machine opens its lanes under the same hostnames and port numbers.
kind: learn
---

myapp moves onto a box: a Linux machine with a Docker engine of its own, where the lanes run while your machine only opens them. Your machine mirrors the box, which means it serves the box's hostnames and port numbers as if its lanes ran here. In this story the box is a container named devbox on your machine's Docker, with a second Docker engine inside it, and the commands on it run through Docker's exec command. A VPS reached over SSH works the same way.

## The lanes on your machine are cleaned first

The box will serve these hostnames, and the databases on this machine are no longer needed. A clean stops a lane, runs its teardown and deletes its data, and in myapp the teardown drops the lane's database.

```console title="~/dev/myapp"
$ devlanes lane clean 2 --yes
cleaned myapp@2: stopped 0 units, teardown database
$ devlanes lane clean 1 --yes
cleaned myapp@1: stopped 2 units, teardown database
$ devlanes lane clean 0 --yes
cleaned myapp@0: stopped 2 units, teardown database
  stopped shared containers (data kept)
```

Lane 2 has nothing left to stop after the agent's stop. With lane 0 cleaned, no lane of myapp uses the shared container, so the last clean stops it and keeps its volume. Lane 0 does not stay gone, though: the primary worktree's lane comes back, stopped and empty, the next time a command acts on the project's lanes, with the same hostnames.

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH  STATE  SERVICES  CPU  MEM  URL
  0     main    idle   –         –    –    https://myapp.localhost

  UNIT  STATE  PORT   CPU  MEM  URL
  api   idle   20060  –    –    https://api.myapp.localhost
  web   idle   20061  –    –    https://myapp.localhost

not running; next: devlanes start --lane myapp@0
```

## The box runs a lane's first start

The box's user config, `~/.devlanes/config.yaml` on the box, gives its lanes ports from 28000 up. That range lies inside the one your machine hands out by default, above the blocks it has handed out so far, so the box's port numbers are free here as well. A port that something on your machine already uses would stay local, and `devlanes remote ls` would report it.

```yaml devlanes-user
ports:
  range: 28000-32767
```

On the box the repository is on the main branch, and its start is a lane's first start: the shared container, setup, the install task, the migration, then the services. Postgres runs on the box's own Docker engine.

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

## Your machine pairs with the box once

By default the box serves only machines it has paired with. On the box, `devlanes daemon pair` prints a code that works once.

```console title="on the box"
$ devlanes daemon pair
pairing code: V7G3-AFH2
it works once and expires in 10 minutes
on your other machine, run: devlanes remote connect <box> --code V7G3-AFH2
```

Your user config gets one entry for devbox: the command that runs the box's end of the connection. The first connect trades the code for a token your machine keeps, so later connections need no code. With `--background` the command returns once connected, and the connection stays with your machine's daemon. Both machines run the same release, and [Prepare a box](../how-to/remote/prepare-a-box.md) says how closely releases have to match.

```console title="~/dev/myapp"
$ devlanes config set remote.boxes.devbox.command '[docker, exec, -i, devbox, devlanes, tunnel, --stdio]'
$ devlanes remote connect devbox --background --code V7G3-AFH2
state: connected
server: devlanes 0.13.0
host: devbox
transport: command
lanes: 1
ports: -
routes: -
```

## A stopped lane here holds the hostnames until you choose the box's

The connection comes up with no ports and no routes. `devlanes remote ls` lists each box your machine knows, one item per box, and the item for devbox says why.

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

Lane 0 on your machine is stopped, and a stopped lane here keeps its hostnames over a box's lane until you choose the box's. The note names the command that does it.

```console title="~/dev/myapp"
$ devlanes remote use devbox myapp@0
myapp@0 now comes from devbox. This machine's lane gets the hostnames back when it starts.
```

## Your machine serves the box's hostnames on the box's ports

The hostnames lead to the box's lane, on the port numbers its services have on the box. The targets read like a local lane's, and the ports are the box's.

```console title="~/dev/myapp"
$ devlanes url ls
HOST                     TARGET       PORT   STATE
api.myapp.localhost  ->  myapp@0:api  28000  ready
myapp.localhost      ->  myapp@0:web  28001  ready
```

When the browser on your machine opens `https://myapp.localhost`, your proxy answers with a certificate from the devlanes CA, a certificate authority devlanes creates on your machine, so the box needs no certificate of its own. The proxy passes the request to 127.0.0.1 on port 28001, where your daemon listens, and your daemon carries it over the one connection to the daemon on devbox, which hands it to the web app. Starting and stopping the box's lanes happen on the box, in a shell there.

![The browser on your machine opens https://myapp.localhost; your proxy passes it to your daemon's listener on 127.0.0.1:28001, which sends it over one connection to the daemon in devbox, a container on your machine's Docker, where web in myapp lane 0 answers on port 28001](remote.svg)

## On your own project

- [Prepare a box](../how-to/remote/prepare-a-box.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Connect to a box without SSH](../how-to/remote/connect-by-command.md)
- [Work with several boxes](../how-to/remote/several-boxes.md)
- [Choose what a box shares](../how-to/remote/what-a-box-shares.md)
- [Remote mirroring](../concepts/remote-mirroring.md)
