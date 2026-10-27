---
sidebar:
  order: 206
---

# DL206 Box pairing needed

`503`. The hostname belongs to a lane on a remote box that serves only machines it paired with, and the box does not accept this machine's pairing. Someone ran `devlanes remote revoke` or `devlanes daemon reset-state` on the box, the box dropped this pairing as the oldest of the 8 it keeps for one machine, or the box asked for a pairing code while no `devlanes remote connect` was attached to enter one. The daemon stopped retrying. While the daemon keeps running, the box's ports stay bound on this machine and its hostnames stay routed, so requests get this page right away instead of waiting. After a daemon restart, the box's ports and hostnames appear once it connects, so until then its hostnames are unknown here ([DL101](unknown-host.md)).

## Fix

```sh
devlanes remote ls                                # the box shows needs_pairing with its reason
devlanes remote pair                              # on the box: prints a code such as K7QM-3XWD
devlanes remote connect my-box --code K7QM-3XWD   # here; without --code it asks for the code
```

A code works once, for 10 minutes. A wrong code voids every code the box handed out, so ask for a new one after a typo; after 5 wrong codes in 10 minutes the box hands out none for a while, and `remote ls` says how long. See [pairing](../remote.md#pairing).
