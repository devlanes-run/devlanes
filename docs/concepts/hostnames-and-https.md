---
title: Hostnames and HTTPS
description: Each lane has a domain of its own and is a separate site in the browser, and the daemon's proxy answers its hostnames over HTTPS with a certificate signed for each name.
kind: concept
---

A service with a hostname is reached over HTTPS through a proxy inside the daemon. The lane's number is part of the domain, so each lane has hostnames of its own, and you write a hostname once in the project file for every lane.

## The lane number is part of the domain

A lane's domain is the project name under localhost, with the lane number joined to the name: `myapp.localhost` for lane 0 and `myapp1.localhost` for lane 1. Hostnames in the project file are written under that domain:

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domain }}"
  web:
    hosts: "${{ lane.domain }}"
```

In lane 1 that gives `api.myapp1.localhost` for the API and `myapp1.localhost` for the web service.

## Each lane is a site of its own

A browser keeps cookies per site, and under localhost a site is the label just before it. The web service at `myapp1.localhost` and the API at `api.myapp1.localhost` are one site, so a cookie the web service sets for its domain reaches the API. Lane 0 is another site and does not get it, so logging in on one branch does not log you in on another.

That is why the number goes into the name. A domain such as lane1.myapp.localhost would put every lane under the one site myapp.localhost, and opening lanes as localhost with a port would be worse, since cookies ignore the port. [Set CORS and cookies for each lane](../how-to/hostnames/cors-and-cookies.md) builds on this.

## The proxy finds the lane by the hostname

All hostnames arrive at the proxy on one port, 443 by default. The browser names the host it wants, the proxy finds the lane and service that route it, and passes the request on to that service's port on 127.0.0.1 over plain HTTP. Your service needs no TLS of its own. A name no lane routes is refused.

![A browser asks for https://api.myapp1.localhost; the name resolves to 127.0.0.1; the proxy in the daemon finds the route, signs a certificate with the devlanes CA and forwards the request to api in lane 1, the site myapp1.localhost; lane 0 is the site myapp.localhost](hostnames-and-https.svg)

When the proxy cannot have port 443, it uses another one and the URLs carry that port; [Choose the proxy's ports](../how-to/hostnames/proxy-ports.md) covers it. Plain http on port 80 redirects to HTTPS.

## The devlanes CA signs each hostname

The certificates come from an authority, the devlanes CA, that devlanes creates on your machine for your user, constrained to names meant for local development ([Security](security.md)). Your browser accepts them once the CA is trusted, which `devlanes setup` does ([Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md)). Services that devlanes starts in Node, Bun, Deno, Python and Rust trust it without any setup.

## Hostnames under localhost resolve to your machine

Names ending in `.localhost` are reserved for the machine itself, so they need no DNS server. Browsers and most systems answer them with 127.0.0.1 by themselves. A few programs ask DNS instead, and for those devlanes can write the names to the hosts file or answer them itself; [Fix programs that cannot resolve *.localhost](../how-to/hostnames/resolve-localhost.md) covers it.

## Related

- [A service, a port and a hostname](../learn/first-service.md)
- [Trust the devlanes CA](../how-to/hostnames/trust-the-ca.md)
- [Ports](ports.md)
- [Security](security.md)
