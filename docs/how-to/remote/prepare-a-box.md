---
title: Prepare a box
description: Install devlanes on another machine, keep its daemon running after you log out and start your project there once.
kind: how-to
---

Before your machine can open the lanes of a VPS or another computer, that machine has to run devlanes and the project itself. Once it does, it is a box, and your machine mirrors its lanes under the same hostnames and ports.

::example{slug="remote-ssh-box"}

This assumes devlanes runs on your own machine, as [Install devlanes](../../install.md) describes, and that you know lanes from [A service, a port and a hostname](../../learn/first-service.md).

## Install devlanes on the box

Run the installer in an SSH shell on the box. Your machine looks in the folder below when it connects, so the box's shell profile does not have to put devlanes on its path.

```console title="on the box"
$ curl -fsSL https://devlanes.run/install.sh | sh -s -- --dir ~/.devlanes/bin
```

The installer takes the latest release, and your machine should run the same one; `devlanes --version` prints it on each. Releases that speak another protocol revision do not connect, and releases that differ on the same revision connect with a warning to install the same release on both. devlanes never updates a box, so when you update one machine, update the other.

The box also needs Git, and Docker when the project has containers. A Linux box needs glibc 2.35 or later.

## Give the box its own ports

Your machine listens on each box port under the same number, so a port that one of your own lanes already holds cannot be mirrored. Both machines hand out ports from the same default range, so split it between them before either one gives lanes their ports.

```yaml devlanes-user
# ~/.devlanes/config.yaml on the box
ports:
  range: 25000-32767
```

```yaml devlanes-user
# ~/.devlanes/config.yaml on your machine
ports:
  range: 20000-24999
```

The daemon reads [`ports.range`](/docs/reference/config/user/ports/) when it starts, so run `devlanes daemon restart` on a machine whose daemon already runs. Lanes that have ports keep them until you clean them.

## Keep the daemon running after you log out

The box daemon starts with the first devlanes command and leaves your SSH session. On Linux, systemd still stops your processes when your last session ends, unless linger is on for your user.

```console title="on the box"
$ sudo loginctl enable-linger "$USER"
```

While it is off, `devlanes remote ls` on your machine warns that the box stops your processes at logout. To have the daemon back after the box reboots as well, run `devlanes daemon install` there; the service it adds depends on linger too.

## Start the project on the box

Clone the project into a folder on the box and start its lane there, as you would on your own machine. Each service has to listen on loopback or on all addresses, since the box reaches it on its own loopback when your machine opens a connection.

```console title="on the box"
$ git clone https://git.example.invalid/myapp.git ~/dev/myapp
```

Run `devlanes start --no-ui` in `~/dev/myapp`. The URLs it prints are the box's, and they open unchanged on your machine once it connects, as [Connect to a box](connect.md) shows.

## If the box runs macOS

Turn on Remote Login in the system settings so the box accepts SSH. macOS does not stop your processes at logout, so linger has no counterpart there. `devlanes daemon install` adds a login item that loads only while someone is logged in at the box's screen.

## If the box runs Windows

A Windows box needs the OpenSSH Server that ships with Windows, and devlanes installed for the account you log in with. Its daemon runs in the background session of SSH, which changes what its units can do; [Windows](../../reference/windows.md) lists the differences.

## If the box daemon cannot find Docker or a tool

The box daemon keeps the environment of whatever started it. When a connection from your machine started it, that is the short path of a non-interactive SSH session, which often lacks Homebrew or a version manager's folder. Run `devlanes daemon restart` from an interactive shell on the box, and the daemon takes that shell's path.

## Related

- [Connect to a box](connect.md)
- [Connect to a box without SSH](connect-by-command.md)
- [Lanes on another machine](../../learn/remote.md)
- [Remote mirroring](../../concepts/remote-mirroring.md)
