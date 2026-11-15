---
title: Give a server you run yourself a hostname
description: Put an HTTPS hostname in front of a server you start yourself, through the devlanes proxy, without devlanes running it.
kind: how-to
---

When you already run a server yourself, such as a docs site, pgAdmin or a tool from another project, devlanes can give it an HTTPS hostname without starting or watching it.

This needs devlanes installed with HTTPS set up, as [Install devlanes](../../install.md) describes, and a look at [A service, a port and a hostname](../../learn/first-service.md) helps.

## Declare the server

devlanes reads a project file, `devlanes.yaml`, in the folder you run it from. The folder can be a Git repository or a plain folder. For a server devlanes does not start, the file names the project and the server's port:

```yaml devlanes
# devlanes.yaml
project: myapp

infra:
  external:
    docs:
      address: 127.0.0.1:4800
      hosts: "${{ lane.domain }}"
```

The address is where the server already listens, on the loopback interface. The hostname is the lane's domain, which is the project name followed by `.localhost`, so this file gives `myapp.localhost`. Pick the project name for the hostname you want.

Then run `devlanes start --no-ui` once in the folder. It has nothing to start, but it makes the folder a lane of the project, and from then on the proxy sends each request for the hostname to port 4800, whether the server is up or not. When devlanes or a lane already holds that port, the start refuses the entry and names what holds it.

On Linux the proxy uses port 8443 unless the daemon may bind 443, and the URL then carries the port; [Choose the proxy's ports](../hostnames/proxy-ports.md) changes that.

devlanes never starts, stops or checks the server. Naming the entry in a start, stop, restart or logs command is an error, since no process of devlanes is behind it.

## Keep the hostname answering

The daemon that a command starts exits a short while after the last command or TUI that watched it, and the proxy goes with it. [The daemon](../../concepts/daemon.md) has the details. Installing the daemon as a login service keeps it, and the hostname, up:

```console
$ devlanes daemon install
```

::::tabs{sync="os"}
:::tab[macOS and Linux]
The service starts a daemon at once and again at each login. If a daemon started by a command is already running, the new one gives way to it, and the installed service takes over only at your next login. To have it take over at once, run `devlanes daemon stop` before the install; that also stops whatever that daemon runs.
:::
:::tab[Windows]
The install adds an entry that starts the daemon at each sign-in. It starts none at the install itself, so the hostname keeps answering from your next sign-in.
:::
::::

`devlanes daemon uninstall` removes the login service and leaves a running daemon alone.

## If only you run the server

A server the rest of the team does not run belongs in the local file, `devlanes.local.yaml` next to the project file, which stays out of Git. [Keep personal changes out of Git](../projects/local-overrides.md) covers the file.

## If the server rejects the hostname

Some dev servers refuse a request whose Host header is not their own address. The mapping form of a hostname makes the proxy send the local address instead:

```yaml devlanes
infra:
  external:
    docs:
      address: 127.0.0.1:4800
      hosts:
        host: "${{ lane.domain }}"
        host_header: upstream
```

## If the server runs on a box

When the server runs on a machine whose lanes you open from your own, declare it in the project file on that machine. [Choose what a box shares](../remote/what-a-box-shares.md) covers what your machine gets of it.

## Related

- [A service, a port and a hostname](../../learn/first-service.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [The daemon](../../concepts/daemon.md)
- [Trust the devlanes CA](../hostnames/trust-the-ca.md)
