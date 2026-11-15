---
title: Prepare a box
description: Install devlanes on another machine, keep its daemon running after you log out and start your project there once.
kind: how-to
---

Before your machine can open the lanes of a VPS or another computer, that machine has to run devlanes and the project itself. Once it does, it is a box, and your machine mirrors its lanes under the same hostnames and ports.

::example{slug="remote-ssh-box"}

This assumes devlanes runs on your own machine, as [Install devlanes](../../install.md) describes, and that you know lanes from [A service, a port and a hostname](../../learn/first-service.md).

## Install devlanes on the box

In an SSH shell on the box, install into `~/.devlanes/bin`. When your machine connects, it looks for devlanes on the path of a non-interactive SSH session and then in that folder, and a non-interactive session does not read the profile where you would add a folder to the path.

```console title="on the box"
$ curl -fsSL https://devlanes.run/install.sh | sh -s -- --dir ~/.devlanes/bin
```

Your own shell on the box needs the folder on its path as well, for the commands that follow.

```console title="on the box"
$ echo 'export PATH="$HOME/.devlanes/bin:$PATH"' >> ~/.profile
$ . ~/.profile
```

The installer takes the latest release, and your machine should run the same one; `devlanes --version` prints it on each. Releases too far apart do not connect, and releases that differ less connect with a warning, as [Remote mirroring](../../concepts/remote-mirroring.md) explains. Your machine never installs or updates devlanes on the box, so when you update one machine, update the other.

The box also needs Git, and Docker when the project has containers. A Linux box needs glibc 2.35 or later. It needs no HTTPS setup: your machine serves the box's hostnames with its own certificates.

## Give the box its own ports

Your machine listens on each box port under the same number, so a port that one of your own lanes holds cannot be mirrored. Your machine keeps new lanes clear of a connected box's ports, but the box knows nothing of yours. Both machines hand out ports from the same default range, so give each its own part of it.

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

The daemon reads [`ports.range`](/docs/reference/config/user/ports/) when it starts, so restart it on a machine where it already runs.

```console title="on your machine"
$ devlanes daemon restart
```

A lane that already has ports keeps them, so a local lane with ports at 28000 or above keeps clashing with the box until you clean it.

## Keep the daemon running after you log out

The box daemon starts with the first devlanes command and leaves your SSH session. On Linux, systemd can stop your processes when your last session ends unless linger is on for your user.

```console title="on the box"
$ sudo loginctl enable-linger "$USER"
```

While linger is off, `devlanes remote ls` on your machine warns about it. To have the daemon back after the box reboots as well, install it as a service, which depends on linger too.

```console title="on the box"
$ devlanes daemon install
```

The service starts the daemon without the lanes, and [Bring lanes back after a reboot](../lanes/after-a-reboot.md) covers those.

## Start the project on the box

Clone the project on the box and start its lane there, as you would on your own machine. Each service has to listen on loopback or on all addresses, since the box reaches it on its own loopback when your machine opens a connection.

```console title="on the box"
$ git clone https://git.example.invalid/myapp.git ~/dev/myapp
$ cd ~/dev/myapp
```

```console title="~/dev/myapp on the box"
$ devlanes start --no-ui
```

The URLs of this lane open unchanged on your machine once it connects, as [Connect to a box](connect.md) shows.

## If the box runs macOS

Turn on Remote Login in the system settings so the box accepts SSH. Linger has no counterpart on macOS.

## If the box runs Windows

A Windows box needs the OpenSSH Server that ships with Windows. Install devlanes with the PowerShell installer from [Install devlanes](../../install.md), as the account you log in with over SSH. Your machine looks for it on that account's path and then in `%USERPROFILE%\.devlanes\bin`.

With some SSH servers the box daemon ends with the SSH session, which `devlanes remote ls` on your machine warns about. Then run `devlanes daemon install` on the box, so the daemon starts when you sign in there instead.

## If the box daemon cannot find Docker or a tool

A new daemon takes its path, and a few other variables listed under [Variables a new daemon keeps](../../reference/environment-variables.md#variables-a-new-daemon-keeps), from whatever started it. When a connection from your machine started it, that is the short path of a non-interactive SSH session, which often lacks Homebrew or a version manager's folder. Restart the daemon from an interactive shell on the box so it takes that shell's path.

```console title="on the box"
$ devlanes daemon restart
```

## Related

- [Connect to a box](connect.md)
- [Connect to a box without SSH](connect-by-command.md)
- [Lanes on another machine](../../learn/remote.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
