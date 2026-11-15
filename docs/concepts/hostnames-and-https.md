---
title: Hostnames and HTTPS
description: Each lane has a domain of its own and is a separate site in the browser, and the daemon's proxy answers its hostnames over HTTPS with a certificate signed for each name.
kind: concept
---

A service with a hostname is reached over HTTPS through a proxy inside the daemon, which signs a certificate for each name it serves. The lane's number is part of the domain, so each lane has hostnames of its own and the browser keeps each lane's cookies apart.

## The lane number is part of the domain

By default a lane's domain is the project name under localhost, with the lane number joined to the name: `myapp.localhost` for lane 0 and `myapp1.localhost` for lane 1. A hostname in the project file is written once under that domain, and each lane gets its own.

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domain }}"
  web:
    hosts: "${{ lane.domain }}"
```

A project can replace the domain with a value of its own in [`lanes.vars.domain`](/docs/reference/config/project/lanes/). Under the resolver strategy, which [Fix programs that cannot resolve *.localhost](../how-to/hostnames/resolve-localhost.md) describes, the domains end in a top-level domain you choose, such as myapp1.test.

Joining the number to the name means two project names can meet: lane 1 of a project called myapp2 and lane 0 of one called myapp21 are both myapp21.localhost. Both lanes then get a warning, and one keeps the hostname: a lane that runs something wins over one that does not, and otherwise the first by project name and lane number.

## Each lane is a site of its own

A browser keeps cookies per site, and under localhost a site is the label just before it. The web service at `myapp1.localhost` and the API at `api.myapp1.localhost` are one site, so a cookie the web service sets for the domain reaches the API, while lane 0 is another site and does not get it. That is why the number goes into the name: a domain such as lane1.myapp.localhost would put every lane under the one site myapp.localhost, and a cookie set for it would reach all of them.

Opening a lane as localhost with a port gives the opposite. Cookies ignore the port, so every lane opened that way would share them, and none would get the proxy's HTTPS.

Names that serve content the app does not control, such as pages its users write, can get a site of their own in each lane with the `domain()` function in an expression ([Environment and expressions](environment.md)).

```yaml devlanes
lanes:
  vars:
    pages_domain: ${{ domain('myapp-pages') }}
```

That gives myapp-pages.localhost in lane 0 and myapp-pages1.localhost in lane 1, apart from the app's cookies. [Serve wildcard and tenant subdomains](../how-to/hostnames/wildcards.md) and [Set CORS and cookies for each lane](../how-to/hostnames/cors-and-cookies.md) build on it.

## The proxy finds the lane by the hostname

The hostnames of all lanes arrive on one port, 443 by default. The browser sends the name it wants at the start of the TLS handshake, the proxy looks it up among its routes, and only then hands over a certificate signed for that name. When no lane routes the name, the handshake is refused and the browser shows its own connection error, not a page from devlanes.

![A browser asks for https://api.myapp1.localhost; the name resolves to 127.0.0.1; the proxy in the daemon finds the route, signs a certificate with the devlanes CA and forwards the request to api in lane 1, the site myapp1.localhost; lane 0 is the site myapp.localhost](hostnames-and-https.svg)

The proxy forwards the request to the service on 127.0.0.1 over plain HTTP. By default the service sees the hostname the browser asked for in the Host header, along with the usual forwarded headers. WebSockets and server-sent events pass through without an idle timeout. A request to a service that is still starting waits for it, for up to half a minute, and then gets [DL202 Service starting](../errors/service-starting.md).

When the proxy cannot have port 443, it uses another one and the URLs carry that port ([Choose the proxy's ports](../how-to/hostnames/proxy-ports.md)).

## The devlanes CA signs each hostname

The certificates come from an authority, the devlanes CA, that devlanes creates on your machine for your user, and it can only sign names meant for local development. [Security](security.md) lists those names, says where its key is and what a leaked key could do.

The daemon does not add the CA to a trust store. `devlanes setup` does that in your terminal, and [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md) covers browsers, tools and runtimes. Node, Bun, Deno, Python and Rust processes that devlanes starts get variables that point their runtime at the CA. An existing mkcert root can sign instead, through [`tls.ca`](/docs/reference/config/user/tls/).

## Hostnames under localhost resolve to loopback

Names ending in `.localhost` are reserved for the machine itself, so they need no DNS server. Chrome and Firefox answer them with 127.0.0.1 by themselves, and so do the system resolvers of Windows 11 and of many Linux and macOS systems.

Some programs on your machine send the name to DNS instead, such as Wrangler's workerd and musl or pure-Go binaries. For those, on macOS and Linux, devlanes can write the exact hostnames to the hosts file through a small helper that `devlanes setup` installs. A hosts file cannot hold a wildcard, which is what the resolver strategy is for: the daemon then answers one top-level domain with a DNS server of its own. A program inside a container sees neither, since it resolves names in the container. [Fix programs that cannot resolve *.localhost](../how-to/hostnames/resolve-localhost.md) chooses between these.

## Plain http redirects to HTTPS

On port 80 the proxy answers a hostname a lane routes with a permanent redirect to the same URL over HTTPS.

```console title="on Windows 11"
$ curl.exe -s -i http://api.myapp.localhost/
HTTP/1.1 308 Permanent Redirect
cache-control: no-store
location: https://api.myapp.localhost/
content-length: 0
```

The redirect is sent only while HTTPS is up. When the proxy runs without it, as when the CA could not be loaded, plain http serves the request instead. A name no lane routes gets [DL101 Unknown host](../errors/unknown-host.md) over plain http.

## Related

- [A service, a port and a hostname](../learn/first-service.md)
- [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md)
- [Ports](ports.md)
- [Security](security.md)
