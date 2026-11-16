---
title: Open a GUI tool as a service
description: Run a remote desktop or an Android emulator's adb server that serves HTTP or TCP as a service, with a hostname where it has a page, also from a box.
kind: how-to
publish: false
---

devlanes does not stream windows, but a tool that already shows itself in a browser or over a TCP port runs in a lane as an ordinary service.

## Give the tool the service's port

devlanes routes the service's hostname to its port, so pass the port to the tool's own listen flag and keep the tool in the foreground. Xpra serves X11 programs through an HTML5 client:

```yaml devlanes
services:
  desktop:
    run: "xpra start :100 --daemon=no --html=on --bind-tcp=127.0.0.1:${{ self.port }} --start=gedit"
    hosts: "desktop.${{ lane.domain }}"
```

The desktop then opens at `https://desktop.myapp.localhost` in lane 0.

## If the tool speaks plain TCP

An Android emulator's adb server needs no hostname. Point adb at the port status lists for the service, with its own port flag:

```yaml devlanes
services:
  adb:
    run: "adb -P ${{ self.port }} nodaemon server"
```

## If the tool runs on a box

On a box, another machine whose lanes you open from yours, hostnames open on your machine and ports are mirrored under the same numbers. [Lanes on another machine](../../learn/remote.md) shows how.

## Related

- [Connect to a box](../remote/connect.md)
- [Start a service only on demand](on-demand.md)
- [Limits](../../reference/limits.md)
