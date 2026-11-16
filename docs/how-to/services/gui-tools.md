---
title: Open a GUI tool as a service
description: Run a remote desktop or an Android emulator's adb server that serves HTTP or TCP as a service, with a hostname where it has a page, also from a box.
kind: how-to
publish: false
---

devlanes does not stream windows, but a simulator, an emulator or a remote desktop that already shows itself in a browser or over a TCP port runs in a lane as an ordinary service, with a hostname you open on your machine.

## Give the tool the service's port

devlanes gives each service a port and sends the service's hostnames to it, so the tool has to listen on that port. Each tool has its own flag for it. Xpra, which runs X11 programs on a Linux machine and serves them through an HTML5 client on its TCP port, takes it in its bind option:

```yaml devlanes
services:
  desktop:
    run: "xpra start :100 --daemon=no --html=on --bind-tcp=127.0.0.1:${{ self.port }} --start=gedit"
    hosts: "desktop.${{ lane.domain }}"
```

The HTML5 client is then behind the service's hostname, such as `https://desktop.myapp.localhost` in lane 0. Keep the tool in the foreground, as the daemon flag here does. A service whose process exits shows as failed, so a tool that moves itself to the background looks broken to devlanes.

## Reach an emulator over TCP

An Android emulator is reached through an adb server, which speaks plain TCP and needs no hostname. Its port flag sets the port the server listens on, and its nodaemon form keeps it in the foreground:

```yaml devlanes
services:
  adb:
    run: "adb -P ${{ self.port }} nodaemon server"
```

The server listens on the port that status lists for the service. Point adb and the tools that use it at that port, with adb's own port flag or the `ANDROID_ADB_SERVER_PORT` variable.

## If the tool runs on a box

On a box, another machine whose lanes you open from yours, the same services reach your machine. Hostnames open there through your proxy, and ports are mirrored under the same numbers, so adb on your machine reaches the box's adb server on the port the box lists. The connection carries TCP only, so a tool that needs UDP does not work across it. [Lanes on another machine](../../learn/remote.md) shows a box and your machine side by side.

## Related

- [Connect to a box](../remote/connect.md)
- [Start a service only on demand](on-demand.md)
- [Limits](../../reference/limits.md)
