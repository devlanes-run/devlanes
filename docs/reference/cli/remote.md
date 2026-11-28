---
title: devlanes remote
description: Connect to boxes and mirror their lanes here.
kind: reference
---

<!-- Generated from the clap definitions in crates/cli by crates/cli/tests/it/reference_docs.rs. Edit the help text in crates/cli, then run DEVLANES_BLESS=1 cargo nextest run -p devlanes-cli -E 'test(cli_reference_is_current)'. -->

```text
devlanes remote [OPTIONS] <COMMAND>
```

## devlanes remote connect

Connect to a box and mirror its lanes, ports and hostnames here.

With a ticket from `devlanes daemon pair` on the box, this machine pairs with the box and connects to it peer to peer, without SSH. It keeps the box under a name, printed once paired, and `devlanes remote connect NAME` connects to it again later without a ticket.

```text
devlanes remote connect [OPTIONS] <HOST>
```

- `<HOST>`: A pairing ticket from devlanes daemon pair on the box, a box name paired by ticket or from remote.boxes in the user config, or an SSH destination.
- `--background`: Return once connected instead of staying attached for prompts.
- `--code <CODE>`: One-time pairing code from devlanes daemon pair on the box, for a box reached over SSH or remote.boxes.
- `--name <NAME>`: Name to keep the box under when pairing by ticket [default: the box's own host name].

## devlanes remote disconnect

Disconnect from a box and remove its mirrored ports and hostnames.

```text
devlanes remote disconnect [OPTIONS] [HOST]
```

- `[HOST]`: Box to disconnect [default: every box].
- `--forget`: Also delete the pairing of a box paired by ticket, so connecting it again needs a new ticket. The box keeps accepting this machine until devlanes daemon revoke there, and folders synced from it stay on disk.

## devlanes remote ls

List known boxes and their connection state.

```text
devlanes remote ls [OPTIONS]
```

## devlanes remote sync

List the synced folders of running lanes.

```text
devlanes remote sync [OPTIONS]
```

## devlanes remote use

Choose which box serves a lane that clashes with others.

Lanes clash when their hostnames overlap. The same lane on two boxes always clashes, as on forks of one box image. This machine mirrors one of them. A lane running here comes first, then the chosen box lane, then a stopped lane here, then the box connected first. The latest choice wins and deletes the choices it clashes with. A choice stays until another one replaces it, `devlanes remote disconnect` forgets the box, or a lane here that clashes with it starts. A box that is not connected can be chosen too.

--local gives the hostnames back to this machine's lane.

```text
devlanes remote use <BOX> <LANE>
devlanes remote use --local <LANE>
```

- `<BOX>`: Box whose lane serves the hostnames.
- `<LANE>`: Lane reference, such as acme@0.
- `--local <LANE>`: Give the lane's hostnames back to this machine.

```text
Examples:
  devlanes remote use fork-b acme@0
  devlanes remote use --local acme@0
```

Every command also takes the [global options](README.md#global-options).
