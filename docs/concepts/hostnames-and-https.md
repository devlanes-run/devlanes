---
title: Hostnames and HTTPS
description: Each lane has a domain of its own, and the daemon's proxy answers its hostnames over HTTPS with certificates from a local authority.
kind: concept
---

A service with a hostname is reached over HTTPS through a proxy inside the daemon, which signs a certificate for the name on the spot. The lane's number is part of the domain, so a browser treats each lane as a separate site, with its own cookies and storage, the way it would treat two different websites.

## The lane number is part of the domain

A lane's domain is the project name under localhost, with the lane number joined to the name: `myapp.localhost` for lane 0 and `myapp1.localhost` for lane 1. A hostname in the project file is written under that domain once, and each lane gets its own.

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domain }}"
  web:
    hosts: "${{ lane.domain }}"
```

The number goes into the name and not into a label of its own, as in `api.lane1.myapp.localhost`, because a browser groups hostnames into sites by the label just before localhost. With a label of its own, every lane would sit under the one site `myapp.localhost` and a cookie set for it would reach all of them. The cost is that two project names can meet: lane 1 of a project called myapp2 and lane 0 of one called myapp21 are both myapp21.localhost, and both lanes get a warning about the clash.

Under the resolver strategy the domains end in the top-level domain you chose, such as myapp1.test ([`hostnames`](/docs/reference/config/user/hostnames/)).

## The proxy finds the lane by the hostname

The hostnames of all lanes arrive on one port, 443 by default. The browser sends the name it wants at the start of the TLS handshake, the proxy looks it up among its routes, and only then hands over a certificate for it, signed for that name. A name with no route is refused during the handshake, so the proxy signs certificates only for names that something serves.

![A browser asks for https://api.myapp1.localhost; the name resolves to 127.0.0.1; the proxy in the daemon finds the route, signs a certificate with the devlanes CA and passes the request to api of lane 1 on port 20070](hostnames-and-https.svg)

The proxy passes the request to the service on 127.0.0.1 over plain HTTP, with the original Host header and the usual forwarded headers. WebSockets and server-sent events pass through without an idle timeout. A request to a service that is still starting waits for it, up to a limit, and a stopped or failed service answers with a page that names the commands to look into it. On Linux the proxy falls back to port 8443 unless the daemon may bind 443, and the URLs then carry the port; [Choose the proxy's ports](../how-to/hostnames/proxy-ports.md) covers that.

## The CA signs only development names

The certificates come from an authority created on your machine for your user. It can sign names under localhost, test, internal and home.arpa, plus the top-level domain configured when it was created. A key that leaked could then impersonate development names and no public site. Switching to another top-level domain later needs a new authority.

The daemon never adds the authority to a trust store; the setup command does, in your terminal, and [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md) covers browsers, tools and runtimes. Node, Bun, Deno, Python and Rust processes that devlanes starts get variables that point their runtime at it. An existing mkcert root can be used instead, through [`tls.ca`](/docs/reference/config/user/tls/), and it has no such limits. [Security](security.md) has what the authority protects against.

## Hostnames under localhost resolve to loopback

Names ending in `.localhost` are reserved for the machine itself, so they need no DNS server. Chrome and Firefox answer them with 127.0.0.1 by themselves, and so do the system resolvers of Windows 11 and of many Linux and macOS systems.

Programs that send the name to DNS instead, such as Wrangler's workerd, musl and pure-Go binaries, or anything inside a container, do not find them. On macOS and Linux devlanes can write the exact hostnames to the hosts file for them, through a small helper the setup command installs. A hosts file cannot hold a wildcard, which is what the resolver strategy is for: the daemon then runs its own DNS server for one top-level domain. [Fix programs that cannot resolve *.localhost](../how-to/hostnames/resolve-localhost.md) chooses between the two.

## Each lane is its own site

A browser keeps cookies per site, and under localhost a site is the label just before it. The web app at `myapp1.localhost` and the API at `api.myapp1.localhost` are one site, so a cookie the app sets for the domain reaches the API. Lane 0 is another site, and that cookie does not reach it.

`localhost` with a port is the opposite case. Cookies ignore the port, so lanes opened as localhost and a port would all share their cookies, and they would get none of the proxy's HTTPS. Names that serve content the app does not control, such as pages its users write, can get a site of their own in each lane.

```yaml devlanes
lanes:
  vars:
    pages_domain: ${{ domain('myapp-pages') }}
```

That gives myapp-pages.localhost in lane 0 and myapp-pages1.localhost in lane 1, apart from the app's cookies. [Serve wildcard and tenant subdomains](../how-to/hostnames/wildcards.md) and [Set CORS and cookies for each lane](../how-to/hostnames/cors-and-cookies.md) build on it.

## Plain http redirects to HTTPS

On port 80 the proxy answers a known hostname with a permanent redirect to the same URL over HTTPS. The redirect is marked not to be cached, so turning HTTPS off later takes effect at once.

```console title="on Windows 11"
$ curl.exe -s -i http://api.myapp.localhost/
HTTP/1.1 308 Permanent Redirect
cache-control: no-store
location: https://api.myapp.localhost/
content-length: 0
```

When the proxy runs without HTTPS, as when the authority could not be loaded, plain http serves the request instead of redirecting it.

## Related

- [A service, a port and a hostname](../learn/first-service.md)
- [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md)
- [Ports](ports.md)
- [Security](security.md)
