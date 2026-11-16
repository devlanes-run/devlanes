---
title: Prepare a box
description: Install devlanes on another machine, keep its daemon running after you log out and start your project there once.
kind: how-to
---

Before your machine can open the lanes of a VPS or another computer, that machine has to run devlanes and the project itself, and then it is a box.

::example{slug="remote-ssh-box"}

This assumes devlanes runs on your own machine, as [Install devlanes](../../install.md) describes, and that you know lanes from [A service, a port and a hostname](../../learn/first-service.md).

## Install devlanes on the box

In an SSH shell on the box, install into `~/.devlanes/bin`, where your machine looks for devlanes when it connects, and put that folder on your shell's path.

```console title="on the box"
$ curl -fsSL https://devlanes.run/install.sh | sh -s -- --dir ~/.devlanes/bin
$ echo 'export PATH="$HOME/.devlanes/bin:$PATH"' >> ~/.profile
$ . ~/.profile
```

Run the same devlanes release on the box and on your machine, and update both together. The box also needs Git, and Docker when the project has containers. It needs no HTTPS setup, because your machine serves the box's hostnames with its own certificates.

## Give the box its own ports

Your machine opens the box's services on the same port numbers the box gave them, so give each machine its own part of the port range. Set the box's before its first devlanes command, and restart your daemon after changing yours.

```yaml devlanes-user
# ~/.devlanes/config.yaml on the box
ports:
  range: 28000-32767
```

```yaml devlanes-user
# ~/.devlanes/config.yaml on your machine
ports:
  range: 20000-27999
```

```console title="on your machine"
$ devlanes daemon restart
```

See [`ports.range`](/docs/reference/config/user/ports/#ports-range) for the setting.

## Keep the daemon running after you log out

On a Linux box, turn on linger so the daemon outlives your SSH session, and install it as a service so it comes back after a reboot.

```console title="on the box"
$ sudo loginctl enable-linger "$USER"
$ devlanes daemon install
```

## Start the project on the box

Clone the project and start its lane, as on your own machine. Each service has to listen on loopback or on all addresses.

```console title="on the box"
$ git clone https://git.example.invalid/myapp.git ~/dev/myapp
$ cd ~/dev/myapp
$ devlanes start --no-ui
```

The box is ready; [Connect to a box](connect.md) opens its lanes on your machine.

## If the box runs Windows

Install the OpenSSH Server that ships with Windows, then install devlanes with the PowerShell installer from [Install devlanes](../../install.md), as the account you log in with over SSH.

## If the box daemon cannot find Docker or a tool

A daemon started by a connection gets the short path of a non-interactive SSH session. Restart it from an interactive shell on the box so it takes that shell's path.

```console title="on the box"
$ devlanes daemon restart
```

## Related

- [Connect to a box](connect.md)
- [Connect to a box without SSH](connect-by-command.md)
- [Lanes on another machine](../../learn/remote.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
