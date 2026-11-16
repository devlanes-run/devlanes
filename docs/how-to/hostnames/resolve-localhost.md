---
title: Fix programs that cannot resolve *.localhost
description: Make lane hostnames resolve for programs that send them to DNS, through the hosts file or a top-level domain of your own.
kind: how-to
---

Use this guide when a lane hostname opens in your browser but another program fails to find it, often with ENOTFOUND or "no such host".

::example{slug="frameworks-wrangler-workers"}

## Check your system

Chrome, Edge and Firefox answer names under localhost themselves, but other programs ask the operating system, and some, such as workerd behind wrangler dev, musl binaries and Go programs built without cgo, ask DNS directly. The doctor's hosts group says whether your system resolves the names:

```console
$ devlanes doctor
```

## Write the hostnames to the hosts file

On macOS and Linux devlanes keeps the exact hostnames of your lanes in the hosts file when your system does not resolve them. A project that runs one of the programs above turns it on for itself:

```yaml devlanes
lanes:
  hosts_file: always
```

devlanes writes the file through a small helper. Start a lane of the project, then run the setup command, which installs the helper and asks for your password once:

```console
$ devlanes start
$ devlanes setup
```

A hosts file holds exact names only, so wildcard hostnames need the resolver below. [`hostnames.hosts_file`](/docs/reference/config/user/hostnames/#hostnames-hosts_file) set to never in your user config turns the file off. devlanes does not write the hosts file on Windows; Windows 11 resolves the names itself.

## Use a top-level domain of your own

The resolver strategy covers every name, wildcards included. Lane domains then end in a domain you choose, such as myapp1.test, and the daemon answers them. The domain has to be test, internal or home.arpa, or a name under one of them.

```yaml devlanes-user
# ~/.devlanes/config.yaml
hostnames:
  resolver: resolver
  tld: test
```

Then let the system send that domain to the daemon, which asks for your password or an administrator's approval, and restart the daemon so lanes take the new domain:

```console
$ devlanes setup
$ devlanes daemon restart
```

On Linux this needs systemd-resolved from systemd 258 or later.

## If you stop using the resolver

Take the resolver out while keeping the CA trusted, remove the two settings, and restart the daemon. Run `devlanes setup` again afterwards if a project still needs the hosts file.

```console
$ devlanes setup --remove --keep-ca
$ devlanes config unset hostnames.resolver
$ devlanes config unset hostnames.tld
$ devlanes daemon restart
```

## If the program runs in a container

A container resolves names on its own, and devlanes has no step that connects it to a lane by hostname.

## Related

- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
- [Run Cloudflare Workers](../projects/cloudflare-workers.md)
- [Serve wildcard and tenant subdomains](wildcards.md)
