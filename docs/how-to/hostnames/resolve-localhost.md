---
title: Fix programs that cannot resolve *.localhost
description: Make lane hostnames resolve for programs that send them to DNS, through the hosts file or a top-level domain of your own.
kind: how-to
---

Use this guide when a lane hostname opens in Chrome but another program, a test runner or a server-side call fails to find it, often with an error such as ENOTFOUND or "no such host".

::example{slug="frameworks-wrangler-workers"}

## Find out whether your system resolves the names

Chrome, Edge and Firefox answer names under localhost with your machine by themselves. Other programs ask the operating system, which answers them on Windows 11 and on Linux with systemd-resolved or nss-myhostname, and on some macOS releases but not others. The doctor command reports what your system does in its hosts group:

::::tabs{sync="os"}
:::tab[macOS]
```console
$ devlanes doctor
```
:::
:::tab[Linux]
```console
$ devlanes doctor
```
:::
:::tab[Windows]
```console
$ devlanes doctor
hosts
  ✓ *.localhost resolves to loopback; no hosts file entries needed
```

Windows Server does not answer these names; [Windows](../../reference/windows.md#hostnames-and-https) covers it.
:::
::::

Some programs send the names to DNS even where the system answers them:

- workerd, the runtime behind wrangler dev
- binaries built against musl, as in Alpine images
- Go programs built without cgo

Programs in a container resolve names inside the container, which the steps below do not reach.

## Write the hostnames to the hosts file

On macOS and Linux devlanes can keep the exact hostnames of your lanes in the hosts file. By default it does so only when a check at daemon start finds that your system does not resolve the names. A project that runs one of the programs above, such as a Wrangler project, turns it on for itself:

```yaml devlanes
lanes:
  hosts_file: always
```

The hosts file belongs to root, so devlanes writes it through a small helper. Start a lane of the project, then run the setup command, which installs the helper and the rule that lets the daemon run it, and asks for your password once:

```console
$ devlanes start --no-ui
```

```console
$ devlanes setup
```

Until a lane of the project has started, the setup command finds nothing that needs the helper on a system that resolves the names itself. A hosts file holds exact names, so wildcard hostnames stay out of it. Setting [`hostnames.hosts_file`](/docs/reference/config/user/hostnames/) to never in your user config turns the file off for every project. devlanes does not write the hosts file on Windows.

## Use a top-level domain of your own

The resolver strategy covers wildcard names too. Lane domains then end in a top-level domain you choose, such as myapp1.test, and the daemon answers every name under it with a DNS server of its own.

```yaml devlanes-user
# ~/.devlanes/config.yaml
hostnames:
  resolver: resolver
  tld: test
```

The domain has to be test, internal, home.arpa or localhost, or a name under one of them, since any other could belong to a real site. Run the setup command so the system sends that domain to the daemon, then restart the daemon so lanes take the new domain:

```console
$ devlanes setup
```

```console
$ devlanes daemon restart
```

::::tabs{sync="os"}
:::tab[macOS]
The setup command writes a file under `/etc/resolver`. If another tool, such as Laravel Valet or puma-dev, already owns `/etc/resolver/test`, the command refuses; choose another domain, such as dev.internal.

`dig` and `nslookup` skip the resolver file, so test a name with the system's own lookup:

```console
$ dscacheutil -q host -a name api.myapp.test
```
:::
:::tab[Linux]
The setup command adds a delegate to systemd-resolved, which needs systemd 258 or later. On an older systemd it refuses and changes nothing. This prints the version:

```console
$ systemctl --version
```
:::
:::tab[Windows]
The setup command adds a Name Resolution Policy Table rule after an administrator prompt, and the rule applies without a restart. Where Group Policy sets such rules, Windows ignores local ones, and the command refuses and says so.
:::
::::

Other lookups keep going to your usual DNS servers. A project that sets `lanes.vars.domain` or `lanes.matrix` keeps the domains it sets.

## If you stop using the resolver

Take the resolver file or rule out first. `devlanes setup --remove` takes out the resolver, the hosts helper and the CA's trust together, and `--keep-ca` keeps the CA trusted:

```console
$ devlanes setup --remove --keep-ca
```

Then remove the two settings from the user config and restart the daemon, so lanes go back to names under localhost:

```console
$ devlanes config unset hostnames.resolver
```

```console
$ devlanes config unset hostnames.tld
```

```console
$ devlanes daemon restart
```

When a project still needs the hosts helper, run `devlanes setup` once more to install it again.

## If the program runs in a container

Inside a container, a name under localhost fails to resolve or means the container itself, and neither the hosts file nor the resolver of your machine applies. The services listen on your machine's 127.0.0.1, and the proxy on its loopback addresses unless [`proxy.lan`](/docs/reference/config/user/proxy/) is on, while a container's loopback is its own. devlanes has no step that connects a container to a lane by its hostname.

## Related

- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Run Cloudflare Workers](../projects/cloudflare-workers.md)
- [Serve wildcard and tenant subdomains](wildcards.md)
