---
title: Serve wildcard and tenant subdomains
description: Route a subdomain per tenant to one service on a cookie site of its own, keep exact hostnames beside it, and catch every other name with a fallback.
kind: how-to
---

Use a wildcard hostname when your app answers on a subdomain per tenant or per user, and each lane should have its own set of them.

::example{slug="web-wildcard-tenants"}

## Give tenant pages a domain of their own

Names under the lane's domain share the app's cookie site, so a cookie the app sets for that domain would reach every tenant page. When tenants write what their pages contain, give the pages a domain apart with the domain function. It adds the lane number the way the lane's domain does, so each lane still gets its own.

```yaml devlanes
lanes:
  vars:
    sites_domain: ${{ domain('myapp-sites') }}
```

That is myapp-sites.localhost in lane 0 and myapp-sites1.localhost in lane 1. [Hostnames and HTTPS](../../concepts/hostnames-and-https.md#each-lane-is-a-site-of-its-own) explains how a browser groups names into sites.

The argument is plain hostname labels: `domain('*.myapp-sites')` is refused, and the `*.` goes in the hostname instead, as the next step shows.

Pick a name only this project uses, and end it in a letter. Two projects that both ask for `domain('sites')` get the same names, and a name ending in a digit can meet another one: lane 1 of sites2 and lane 0 of sites21 are both sites21.localhost.

## Route the wildcard to the service

A hostname that starts with `*.` matches every name below the rest of it, at any depth. Quote it, since a leading `*` starts a YAML alias.

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

In lane 1 the route takes acme.myapp-sites1.localhost and eu.acme.myapp-sites1.localhost, but not myapp-sites1.localhost itself; list that name too if the service answers on it. The proxy passes the Host header on unchanged, and the variable tells the service which domain to strip from it to find the tenant.

## Keep exact hostnames under the lane's domain

The app itself and any fixed names stay under the lane's domain, where they share one cookie site. An exact hostname wins over a wildcard that matches the same name, and a longer wildcard wins over a shorter one.

```yaml devlanes
services:
  app:
    run_script:
      file: apps/app/package.json
      name: dev
    hosts: app.${{ lane.domain }}
  links:
    run_script:
      file: apps/links/package.json
      name: dev
    hosts: go.${{ lane.domain }}
```

## Catch every other name with a fallback

A wildcard under the lane's domain takes every name there that the exact hostnames leave, which suits a gateway that shows a page for names nobody serves, or forwards them somewhere. Mark it as a fallback, so that a wildcard on the same names, such as the tenant pages in the case below, wins over it.

```yaml devlanes
services:
  gateway:
    run_script:
      file: apps/gateway/package.json
      name: dev
    hosts:
      - host: "*.${{ lane.domain }}"
        fallback: true
```

With all four services running in lane 1, app.myapp1.localhost and go.myapp1.localhost reach their own services and any other name under myapp1.localhost reaches the gateway. [`services.<name>.hosts`](/docs/reference/config/project/services/) lists the fields of a route.

## If tenant pages may share the app's cookies

Put the tenant wildcard under the lane's domain instead, without a fallback:

```yaml devlanes
services:
  sites:
    hosts: "*.${{ lane.domain }}"
```

It then takes every name the exact hostnames leave, ahead of the gateway's fallback. Requests from those pages to the app count as same-site, so a SameSite cookie does not keep them out.

## If a program other than a browser opens a tenant hostname

Chrome, Edge and Firefox resolve wildcard names under localhost by themselves. A program that needs the hosts file to find lane hostnames cannot find these, because a hosts file holds exact names only. The resolver strategy answers every name under a top-level domain of your own, wildcards included, and [Fix programs that cannot resolve *.localhost](resolve-localhost.md) sets it up.

## Related

- [Set CORS and cookies for each lane](cors-and-cookies.md)
- [Give a service several ports and hostnames](../services/ports-and-hostnames.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
