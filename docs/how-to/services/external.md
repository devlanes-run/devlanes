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

## Install the daemon as a login service

The proxy runs inside the daemon. A daemon that a command starts exits soon after the last command that watched it, and the external entry does not keep it up, so the hostname would stop answering. A daemon installed as a login service keeps running, and comes back after a reboot.

::::tabs{sync="os"}
:::tab[macOS]
First check whether a daemon already runs. This command never starts one:

```console
$ devlanes daemon status
```

If one runs, stop it, which also stops what it runs. Otherwise it stays in place of the installed one until your next login, and exits when unused.

```console
$ devlanes daemon stop
```

On macOS, if you installed the login service before, an install that finds the same service loaded starts nothing, so remove it first:

```console
$ devlanes daemon uninstall
```

Then install the service. It starts the daemon at once and again at each login:

```console
$ devlanes daemon install
```
:::
:::tab[Linux]
First check whether a daemon already runs. This command never starts one:

```console
$ devlanes daemon status
```

If one runs, stop it, which also stops what it runs. Otherwise it stays in place of the installed one until your next login, and exits when unused.

```console
$ devlanes daemon stop
```

Then install the service. It starts the daemon at once and again at each login:

```console
$ devlanes daemon install
```
:::
:::tab[Windows]
Install the service. It adds an entry that starts the daemon at each sign-in, and starts none when you run it:

```console
PS> devlanes daemon install
```

To keep the daemon that commands start running as well, turn off its exit when unused:

```console
PS> devlanes config set daemon.exit_when_idle false
```

A daemon that already runs keeps its old setting until it restarts, so restart it if one runs:

```console
PS> devlanes daemon restart
```
:::
::::

## Start the lane

Run one start in the folder:

```console title="~/dev/myapp"
$ devlanes start --no-ui
```

It has nothing to start, but it makes the folder a lane of the project, and the proxy sends each request for the hostname to port 4800, whether the server is up or not. When devlanes or a lane already holds that port, the start refuses the entry and names what holds it.

Open `https://myapp.localhost`. While your server is down, the proxy answers with its [DL301 Service not listening](../../errors/upstream-unreachable.md) page instead. On Linux the proxy uses port 8443 unless the daemon may bind 443, and the URL then carries the port; [Choose the proxy's ports](../hostnames/proxy-ports.md) changes that.

devlanes never starts, stops or checks the server.

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
