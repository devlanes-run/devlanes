---
title: Fix programs that cannot resolve *.localhost
description: Make lane hostnames resolve for programs that send them to DNS, through the hosts file or a top-level domain of your own.
kind: how-to
---

Use this guide when a lane hostname opens in your browser but a program, a test runner or a server-side call fails to find it, often with an error such as ENOTFOUND or "no such host".

::example{slug="frameworks-wrangler-workers"}

## Find out whether your system resolves the names

Browsers answer names under localhost with your machine by themselves. Other programs ask the operating system, and the doctor command shows whether yours does the same:

```console title="on Windows 11"
$ devlanes doctor
hosts
  ✓ *.localhost resolves to loopback; no hosts file entries needed
```

Windows 11, macOS 26 and Linux with systemd-resolved answer these names. macOS 15 and older, Windows Server and some Linux setups send them to your DNS server, which knows nothing about them. Some programs ask DNS even where the system answers: workerd, the runtime behind wrangler dev, binaries built against musl, and Go programs built without cgo. Programs in a container resolve names inside the container, which the steps below do not reach.

## Write the hostnames to the hosts file

On macOS and Linux devlanes can keep the exact hostnames of your lanes in the hosts file. By default it does so only when the doctor's check fails. A project whose own program asks DNS, such as a Wrangler project, asks for it in every case:

```yaml devlanes
lanes:
  hosts_file: always
```

The hosts file belongs to root, so devlanes writes it through a small helper. The setup command installs the helper and the rule that lets the daemon run it, and asks for your password once:

```console
$ devlanes setup
```

A hosts file holds exact names, so wildcard hostnames stay out of it. Setting [`hostnames.hosts_file`](/docs/reference/config/user/hostnames/) to never in your user config turns the file off for every project.

devlanes does not write the hosts file on Windows. On Windows Server, add each hostname to `%SystemRoot%\System32\drivers\etc\hosts` yourself from an elevated shell, or use the resolver below.

## Use a top-level domain of your own

The resolver strategy covers wildcard names too. Lane domains then end in a top-level domain you choose, such as myapp1.test, and the daemon answers every name under it with a DNS server of its own.

```yaml devlanes-user
# ~/.devlanes/config.yaml
hostnames:
  resolver: resolver
  tld: test
```

The domain has to be test, internal, home.arpa or a name under one of them, since any other could belong to a real site. Run the setup command so the system sends that domain to the daemon, then restart the daemon so lanes take the new domain:

```console
$ devlanes setup
$ devlanes daemon restart
```

::::tabs{sync="os"}
:::tab[macOS]
The setup command writes a file under `/etc/resolver`. `dig` and `nslookup` skip that file, so test a name with `ping` or a browser.
:::
:::tab[Linux]
The setup command adds a delegate to systemd-resolved, which needs systemd 258 or later. On an older systemd it refuses and changes nothing.
:::
:::tab[Windows]
The setup command adds a Name Resolution Policy Table rule after an administrator prompt, and the rule applies without a restart. Where Group Policy sets such rules, Windows ignores local ones, and the command refuses and says so.
:::
::::

Other lookups keep going to your usual DNS servers. A project that sets its own domain in `lanes.vars` keeps names under localhost. `devlanes setup --remove` takes the resolver out again.

## If the program runs in a container

Inside a container, localhost names resolve to the container itself, and neither the hosts file nor the resolver of your machine applies. Have the container call the service by its port on your machine instead, which Docker Desktop names host.docker.internal. [Trust the devlanes CA](trust-the-ca.md) covers HTTPS from a container.

## Related

- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Run Cloudflare Workers](../projects/cloudflare-workers.md)
- [Serve wildcard and tenant subdomains](wildcards.md)
