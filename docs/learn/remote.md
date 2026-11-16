---
title: Lanes on another machine
description: The main branch of myapp moves onto a box, and the browser and tools on your machine reach its lane under the same hostnames and port numbers.
kind: learn
---

The main branch of myapp, and only main, moves onto a box: a Linux machine with a Docker engine of its own. Your machine mirrors the box: it serves the box's hostnames and port numbers as if the lane ran here. Here the box is devbox, a container on your machine's Docker that you reach with Docker's exec command, and a VPS reached over SSH works the same way.

## You clean the local lanes, and lane 0 comes back empty

The checkout branch and the agent's branch are finished, so you clean their lanes, and the teardown from [A database for each lane](databases.md) drops each lane's database. Main moves to the box, so you clean lane 0 as well.

```console title="~/dev/myapp"
$ devlanes lane clean 2 --yes
cleaned myapp@2: stopped 0 units, teardown database
$ devlanes lane clean 1 --yes
cleaned myapp@1: stopped 2 units, teardown database
$ devlanes lane clean 0 --yes
cleaned myapp@0: stopped 2 units, teardown database
  stopped shared containers (data kept)
```

The last clean also stops the shared container and keeps its volume. The main worktree keeps a lane of its own, so the clean of lane 0 only empties it: the lane comes back stopped, without its database, and holds myapp's hostnames.

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH  STATE  SERVICES  CPU  MEM  URL
  0     main    idle   –         –    –    https://myapp.localhost
```

## The box runs main on ports of its own

devbox has devlanes installed, a Docker engine of its own and a clone of myapp on the main branch; [Prepare a box](../how-to/remote/prepare-a-box.md) has the steps and says how closely its release has to match yours. Its user config, `~/.devlanes/config.yaml` on the box, gives its lanes ports from 28000 up, where your machine starts at 20000 by default.

```yaml devlanes-user
ports:
  range: 28000-32767
```

Each machine's daemon places services in blocks of its own, so these numbers are the box's, above the blocks your machine has handed out so far. A port that a program here already holds is not mirrored, and `devlanes remote ls` names the program. The box's first start brings up the shared container on the box's own Docker, then setup, the tasks and the services.

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

## You pair your machine with the box once

By default the box serves only machines it has paired with, so reaching it is not enough: you also need a code that `devlanes daemon pair` prints on the box.

```console title="on the box"
$ devlanes daemon pair
pairing code: V7G3-AFH2
it works once and expires in 10 minutes
on your other machine, run: devlanes remote connect <box> --code V7G3-AFH2
```

Your user config gets one entry for the box. Its key ends in devbox, the name your machine gives the box, and its command runs `devlanes tunnel --stdio` in the container named devbox; the connection is that command's input and output. Over SSH, `devlanes remote connect` takes the SSH destination and needs no entry.

```console title="~/dev/myapp"
$ devlanes config set remote.boxes.devbox.command '[docker, exec, -i, devbox, devlanes, tunnel, --stdio]'
$ devlanes remote connect devbox --background --code V7G3-AFH2
state: connected
host: devbox
ports: -
routes: -
```

The first connect trades the code for a token your machine keeps, so later connections need none. With `--background` it returns once connected, and your daemon keeps the connection.

## Lane 0 here holds the hostnames until you hand them to the box

The connection comes up with no ports and no routes, and `devlanes remote ls`, which lists one item per box, says why in the item for devbox.

```console title="~/dev/myapp"
$ devlanes remote ls
items:
  state: connected
  host: devbox
  skipped_lanes:
    lane: myapp@0
    hosts: api.myapp.localhost, myapp.localhost
    note: not mirrored because this machine's myapp@0 is in use. devlanes remote use devbox myapp@0 switches.
```

In use here means that lane 0 exists on this machine and holds the hostnames, although it runs nothing. It keeps them until you choose the box's lane with the command the note names.

```console title="~/dev/myapp"
$ devlanes remote use devbox myapp@0
myapp@0 now comes from devbox. This machine's lane gets the hostnames back when it starts.
```

## Your browser and your tools reach the box's lane

After the switch, the hostnames lead to the box's port numbers.

```console title="~/dev/myapp"
$ devlanes url ls
HOST                     TARGET       PORT   STATE
api.myapp.localhost  ->  myapp@0:api  28000  ready
myapp.localhost      ->  myapp@0:web  28001  ready
```

When your browser opens `https://myapp.localhost`, your proxy answers with a certificate from the devlanes CA, a certificate authority devlanes creates on your machine. It passes the request to your daemon's listener on `127.0.0.1:28001`, which carries it over the connection to the daemon on devbox and on to the web app. A client of yours that connects to that port takes the same way, which is why the numbers match the box's. You start and stop the box's lane in a shell on the box.

![The browser on your machine opens https://myapp.localhost and your proxy passes it to your daemon's listener on 127.0.0.1:28001, which a client on your machine can also reach; the listener sends it over one connection to the daemon on devbox, a container here, where web in myapp lane 0 answers on port 28001](remote.svg)

From your machine you open main's lane by its hostname and reach its services by their ports, as if it ran here.

## On your own project

- [Prepare a box](../how-to/remote/prepare-a-box.md)
- [Connect to a box](../how-to/remote/connect.md)
- [Connect to a box without SSH](../how-to/remote/connect-by-command.md)
- [Work with several boxes](../how-to/remote/several-boxes.md)
- [Choose what a box shares](../how-to/remote/what-a-box-shares.md)
- [Remote mirroring](../concepts/remote-mirroring.md)
