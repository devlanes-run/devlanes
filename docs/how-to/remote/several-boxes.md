---
title: Work with several boxes
description: Stay connected to several boxes that run the same project, choose which one serves each lane and switch between them.
kind: how-to
---

When two boxes, here fork-a and fork-b, run the same lanes, as copies made from one image do, your machine stays connected to both and one of them serves each lane at a time.

## Give each box its own ports

Two boxes know nothing of each other's ports, so split the box's part of the range between them, keeping your machine's part from [Prepare a box](prepare-a-box.md). Copies of one image start with the same [`ports.range`](/docs/reference/config/user/ports/#ports-range), so set it on each copy before its first devlanes command.

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

Then connect to each box under its own name, as [Connect to a box](connect.md) describes.

## Know which box serves a lane

When lanes share hostnames, as lane 0 of myapp does on both boxes, your machine mirrors one of them, in this order:

1. a lane on your machine that runs
2. the box lane you chose
3. a lane on your machine that is stopped
4. the lane of the box that connected first

`devlanes remote ls` lists the lanes it skipped, each with the command that switches to it.

```console title="on your machine"
$ devlanes remote ls
```

## Switch to another box

Choose the box that serves the lane.

```console title="on your machine"
$ devlanes remote use fork-b myapp@0
```

The switch keeps the lane's hostnames, but not its port numbers, since each box gave the lane ports from its own range. A browser follows the switch; a program that connects to a port number, such as a database client, needs the new box's port, which `devlanes remote ls` shows. The choice lasts until another one replaces it or you disconnect the box.

The browser extension's popup also switches which box serves a lane. See [Use the browser extension](../use-the-browser-extension.md#switch-which-box-serves-a-lane).

## If a lane on your machine runs

A running local lane keeps its hostnames, so the choice is refused. Stop the lane, then choose the box again.

```console title="on your machine"
$ devlanes stop
$ devlanes remote use fork-b myapp@0
```

## Related

- [Connect to a box](connect.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
- [Lanes on another machine](../../learn/remote.md)
