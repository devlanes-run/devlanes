---
title: Serve wildcard and tenant subdomains
description: Route a subdomain per tenant to one service on a cookie site of its own, keep exact hostnames beside it, and catch every other name with a fallback.
kind: how-to
---

Use a wildcard hostname when your app answers on a subdomain per tenant or per user, and each lane should have its own set of them.

::example{slug="web-wildcard-tenants"}

## Give tenant pages a domain of their own

When tenants write what their pages contain, keep those pages off the app's cookie site. The domain function gives them a domain that, like the lane's domain, carries the lane number:

```yaml devlanes
lanes:
  vars:
    sites_domain: ${{ domain('myapp-sites') }}
```

That is myapp-sites.localhost in lane 0 and myapp-sites1.localhost in lane 1. Pick a name no other project uses, ending in a letter.

## Route the wildcard to the service

A hostname that starts with `*.` matches every name below the rest of it. Quote it, since a leading `*` means something else in YAML.

```yaml devlanes
services:
  sites:
    run_script:
      file: apps/sites/package.json
      name: dev
    hosts: "*.${{ lane.sites_domain }}"
    env:
      SITES_DOMAIN: ${{ lane.sites_domain }}
```

The service gets the request with its original Host header and reads the tenant from it.

## Keep exact hostnames and a fallback beside it

An exact hostname wins over any wildcard, so the app and fixed names stay under the lane's domain. A wildcard marked as a fallback takes every name nothing else claims, which suits a gateway that answers for unknown names.

```yaml devlanes
services:
  app:
    run_script:
      file: apps/app/package.json
      name: dev
    hosts: app.${{ lane.domain }}
  gateway:
    run_script:
      file: apps/gateway/package.json
      name: dev
    hosts:
      - host: "*.${{ lane.domain }}"
        fallback: true
```

Start the lane and list its routes, wildcards and the fallback included:

```console
$ devlanes start
$ devlanes url ls
```

[`services.<name>.hosts[]`](/docs/reference/config/project/services/#services-name-hosts) lists the fields of a route.

## If tenant pages may share the app's cookies

Put the tenant wildcard under the lane's domain, `"*.${{ lane.domain }}"`, without a fallback. It then takes every name the exact hostnames leave, ahead of the gateway.

## If a program other than a browser opens a tenant hostname

A hosts file cannot hold wildcards, so such a program needs the resolver strategy from [Fix programs that cannot resolve *.localhost](resolve-localhost.md).

## Related

- [Set CORS and cookies for each lane](cors-and-cookies.md)
- [Give a service several ports and hostnames](../services/ports-and-hostnames.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
