---
title: Open a GUI tool as a service
description: Run a simulator, an Android screen or a remote desktop that serves HTTP or TCP as a service with a hostname, also from a box.
kind: how-to
---

A simulator, an Android screen or a remote desktop that already shows itself in a browser or over a TCP port runs in a lane as an ordinary service, with a hostname you open on your machine.

devlanes does not stream windows itself; [Limits](../../reference/limits.md) lists that. The tools below do the streaming, and devlanes gives each one a port, a hostname and a place in the lane.

## Give the tool the service's port

Each tool has its own flag for the port it listens on. Pass it the service's port, and add a hostname when the tool has a page to open:

```yaml devlanes
services:
  ios-sim:
    run: npx serve-sim --port ${{ self.port }}
    hosts: "sim.${{ lane.domain }}"
    autostart: false
```

This one is serve-sim, which shows a booted iOS simulator in the browser on a Mac. Autostart is off because a simulator is not needed on every start; [Start a service only on demand](on-demand.md) covers that. ws-scrcpy, which shows an Android screen in the browser, follows the same pattern.

## Open a desktop in the browser

Xpra runs X11 programs on a Linux machine and serves them through an HTML5 client. Bind it to the service's port on loopback:

```yaml devlanes
services:
  desktop:
    run: "xpra start :100 --daemon=no --html=on --bind-tcp=127.0.0.1:${{ self.port }} --start=gedit"
    hosts: "desktop.${{ lane.domain }}"
```

The desktop then opens at the service's hostname, such as `https://desktop.myapp.localhost` in lane 0. Keep the tool in the foreground, as the daemon flag here does. A service whose process exits shows as failed, so a tool that forks into the background and returns looks broken to devlanes.

## Reach an emulator over TCP

An Android emulator is reached through an adb server, which speaks plain TCP and needs no hostname:

```yaml devlanes
services:
  adb:
    run: "adb -P ${{ self.port }} nodaemon server"
```

Tools on the same machine reach it by port, the one status lists for the service.

## From a box

On a box you are connected to, the same services reach your machine. Hostnames open there through your proxy, and ports are mirrored under the same numbers, so the adb server on the box answers on your machine as well:

```console title="on your machine"
$ ANDROID_ADB_SERVER_PORT=<port> adb devices
```

The port is the one the box lists for the service. Only TCP is mirrored, so a tool that needs UDP does not work across the connection. [Lanes on another machine](../../learn/remote.md) shows a box and your machine side by side.

## Related

- [Connect to a box](../remote/connect.md)
- [Start a service only on demand](on-demand.md)
- [Limits](../../reference/limits.md)
