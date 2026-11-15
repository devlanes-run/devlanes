---
title: Work with several boxes
description: Stay connected to several boxes that run the same project, choose which one serves each lane and switch between them.
kind: how-to
---

When two boxes, here fork-a and fork-b, run the same lanes, as copies made from one image do, your machine stays connected to both and one of them serves each lane at a time. You choose which, and the switch keeps every hostname and port.

## Give each box its own ports

Two boxes know nothing of each other's ports, so lanes that only one of them runs can get the same numbers, and then only one of them is mirrored. Split the box's part of the range between the two in their user configs, before those lanes get their ports. Your machine keeps its own part from [Prepare a box](prepare-a-box.md).

```yaml devlanes-user
# ~/.devlanes/config.yaml on fork-a
ports:
  range: 28000-30399
```

```yaml devlanes-user
# ~/.devlanes/config.yaml on fork-b
ports:
  range: 30400-32767
```

Copies of one image start with the same [`ports.range`](/docs/reference/config/user/ports/), so set it on a copy right after it is made, then restart its daemon.

```console title="on fork-b"
$ devlanes daemon restart
```

While two connected boxes share a range, `devlanes remote ls` warns on both. For the lanes both boxes run, that is expected, since one box serves each of them.

Then connect to each box under its own name, as [Connect to a box](connect.md) describes. Both stay connected.

## Know which box serves a lane

Two lanes clash when their hostnames overlap, and lane 0 of myapp on two boxes always clashes, since its ports could lead to only one of them. Of the lanes that clash, your machine mirrors one, in this order:

1. a lane on your machine that runs
2. the box lane you chose
3. a lane on your machine that is stopped
4. the lane of the box that connected first

The others are skipped whole, ports included, rather than renamed; [Remote mirroring](../../concepts/remote-mirroring.md) explains why. `devlanes remote ls` lists the skipped lanes of each box.

```console title="on your machine"
$ devlanes remote ls
```

## Switch to another box

Choose the box that serves the lane.

```console title="on your machine"
$ devlanes remote use fork-b myapp@0
```

The lane's hostnames and ports move to fork-b in one step, and your machine keeps listening on the ports throughout. When fork-b is not connected or does not have the lane yet, the switch happens once it does. Open connections to the old box close, so database clients reconnect and an open page loses its connection to the dev server. The browser keeps its cookies for the hostnames, so the app on the new box may ask you to sign in again when it rejects what the old one set.

The choice lasts across reconnects and daemon restarts, until another choice for that lane replaces it. Disconnecting a box deletes its choices. Handing a lane back to your machine works as [Connect to a box](connect.md#use-the-boxs-lane-instead-of-a-stopped-local-one) describes.

## If a lane on your machine runs

A running local lane holds its ports, so it keeps its hostnames and a choice of a box for that lane is refused. Stop the lane on your machine, and its own containers when they publish ports, then choose the box again.

## If copies come and go

Copies that SSH alone should guard can skip pairing, so each new copy connects without a code; [Connect to a box](connect.md#if-the-box-should-serve-without-pairing) has the setting.

## Related

- [Connect to a box](connect.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
- [Remote connection states](../../reference/remote-states.md)
- [Lanes on another machine](../../learn/remote.md)
