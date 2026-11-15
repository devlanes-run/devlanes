---
title: Work with several boxes
description: Stay connected to several boxes that run the same project, choose which one serves each lane and switch between them.
kind: how-to
---

When two boxes run the same lanes, as copies made from one image do, your machine stays connected to both and one of them serves each lane at a time. You choose which, and the switch keeps every hostname and port.

## Give each box its own ports

Two boxes know nothing of each other's ports, so lanes that only one of them runs can get the same numbers, and then only one of them is mirrored. Give each box its own part of the range in its user config, before those lanes get their ports.

```yaml devlanes-user
# ~/.devlanes/config.yaml on fork-a
ports:
  range: 25000-28999
```

```yaml devlanes-user
# ~/.devlanes/config.yaml on fork-b
ports:
  range: 29000-32767
```

Copies of one image start with the same [`ports.range`](/docs/reference/config/user/ports/), so set it on a copy right after it is made, then run `devlanes daemon restart` there.

While two connected boxes share a range, `devlanes remote ls` warns on both. For the lanes both boxes run, that is expected, since one box serves each of them.

Then connect to each box under its own name, as [Connect to a box](connect.md) describes. Both stay connected.

## Know which box serves a lane

Two lanes clash when their hostnames overlap, and the same lane on two boxes always clashes, since its ports could lead to only one of them. Of the lanes that clash, your machine mirrors one, in this order:

1. a lane on your machine that runs
2. the box lane you chose
3. a lane on your machine that is stopped
4. the lane of the box that connected first

The others are skipped whole, ports included, rather than renamed; [Remote mirroring](../../concepts/remote-mirroring.md) explains why. `devlanes remote ls` lists a skipped lane under its box, with the lane that serves its hostnames and the command that switches to it.

## Switch to another box

To have fork-b serve lane 0 of myapp, run `devlanes remote use fork-b myapp@0` on your machine. It returns once the routes and ports lead to fork-b.

The lane's hostnames and ports move to that box in one step, and your machine keeps listening on the ports throughout. Open connections to the old box close, so database clients reconnect and a dev server's page reloads. The browser keeps its cookies for the hostnames, so the app on the new box may ask you to sign in again when it rejects what the old one set.

The choice lasts across reconnects and daemon restarts, until another choice for that lane replaces it. A box that is not connected yet can be chosen too, and takes the lane when it connects. `devlanes remote use --local myapp@0` hands the lane back to your machine.

## If a lane on your machine runs

A running local lane holds its ports, so it keeps its hostnames and the choice is refused. The refusal names the commands that stop the lane, and its own containers when they publish ports. Stop them, then choose the box again.

Starting a local lane later takes the hostnames back and drops the choices it clashes with. Stopping it again does not return them to the box, so choose again.

## If a box is gone for good

Your machine keeps reconnecting a box after each restart until you disconnect it, which also deletes its choices.

```console title="on your machine"
$ devlanes remote disconnect fork-a
```

## If copies come and go

When SSH already decides who may reach a copy, set [`remote.require_pairing`](/docs/reference/config/user/remote/) to false in the image's user config, so each new copy connects without a code. [Security](../../concepts/security.md) covers what that gives up.

## Related

- [Connect to a box](connect.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
- [Remote connection states](../../reference/remote-states.md)
- [Lanes on another machine](../../learn/remote.md)
