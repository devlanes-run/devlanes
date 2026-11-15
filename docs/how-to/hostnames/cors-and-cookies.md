---
title: Set CORS and cookies for each lane
description: Let a web app call its own lane's API, share a login cookie between the two, and keep both out of every other lane.
kind: how-to
---

Follow these steps when a web app and its API run on two hostnames of one lane and the browser has to call the API with the user's cookie, without any other lane getting through.

::example{slug="web-frontend-api"}

## Allow the lane's own origins

devlanes gives every service a variable with its lane's origins: the HTTPS origin of each hostname in the lane, wildcards left out, and the localhost and 127.0.0.1 address of each service's port. Build the API's CORS allow-list from it instead of writing hostnames down, so each lane allows its own pages and no other lane's.

```ts
import { cors } from "hono/cors";

const origins = (process.env.DEVLANES_ORIGINS ?? "").split(",").filter(Boolean);

export const laneCors = () =>
  cors({ origin: origins, credentials: true });
```

The credentials option lets the browser send the session cookie with these requests. The variable belongs to the process, so read it in code; an expression in the project file cannot read it.

## Check that another lane gets no CORS headers

Send the lane 1 API a request with the origin of lane 1's web app, then one with lane 0's. Only the first gets an allow-origin header back, so a page of lane 0 cannot read the answer.

::::tabs{sync="os"}
:::tab[macOS and Linux]
When your lane URLs carry a port, such as 8443, add it to the URL and to both origins.

```console
$ curl -s -i -H "Origin: https://myapp1.localhost" https://api.myapp1.localhost/
```

```console
$ curl -s -i -H "Origin: https://myapp.localhost" https://api.myapp1.localhost/
```
:::
:::tab[Windows]
```console
$ curl.exe -s -i -H "Origin: https://myapp1.localhost" --ssl-revoke-best-effort https://api.myapp1.localhost/
HTTP/1.1 200 OK
access-control-allow-origin: https://myapp1.localhost
vary: origin
content-type: application/json

{"lane":1,"lane_name":"lane1","service":"api","port":20070}
```

```console
$ curl.exe -s -i -H "Origin: https://myapp.localhost" --ssl-revoke-best-effort https://api.myapp1.localhost/
HTTP/1.1 200 OK
content-type: application/json

{"lane":1,"lane_name":"lane1","service":"api","port":20070}
```
:::
::::

When the proxy runs on its fallback port, the HTTPS origins carry that port, the way a browser sends them. [Environment variables](../../reference/environment-variables.md) describes the variable in full.

## Share a cookie between the app and its API

A cookie set for the lane's domain reaches every hostname under it, so in lane 1 it reaches both the app at myapp1.localhost and the API at api.myapp1.localhost. Pass the domain to the service that sets the cookie:

```yaml devlanes
services:
  api:
    run_script:
      file: apps/api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
    env:
      COOKIE_DOMAIN: ${{ lane.domain }}
```

The API then sets the cookie with that domain, `SameSite=Lax` and `Secure`:

```ts
setCookie(c, "session", token, {
  domain: process.env.COOKIE_DOMAIN,
  path: "/",
  sameSite: "Lax",
  secure: true,
});
```

Lane 0's hostnames are outside that domain, so the cookie never reaches them, and signing in on one lane leaves you signed out on the other. [Hostnames and HTTPS](../../concepts/hostnames-and-https.md#each-lane-is-a-site-of-its-own) explains how a browser groups names into sites.

## Give each lane its own login callback

A login through an outside provider sends the browser back to a callback URL, and each lane needs its own. Build it from the service's public URL, and register each lane's URL with the provider, port included when lane URLs carry one:

```yaml devlanes
services:
  web:
    hosts: "${{ lane.domain }}"
    env:
      OAUTH_REDIRECT_URI: "${{ self.public_url }}/auth/callback"
```

## If the web app proxies its API calls

A dev server that forwards a path such as `/api` to the API makes every call same-origin, so the API needs no CORS. Point the forward at the API's local URL, which skips the proxy:

```yaml devlanes
services:
  web:
    env:
      API_PROXY_TARGET: ${{ services.api.url }}
```

The cookie then belongs to the app's own hostname and needs no domain.

## If tenant subdomains call the API

The variable leaves out wildcard hostnames, and tenant pages on a domain of their own, as [Serve wildcard and tenant subdomains](wildcards.md) sets them up, are another site. Their calls to the API are cross-site, so the session cookie does not go with them. The API checks their origin against the tenant domain itself, which it needs in its own environment:

```yaml devlanes
lanes:
  vars:
    sites_domain: ${{ domain('myapp-sites') }}

services:
  api:
    env:
      SITES_DOMAIN: ${{ lane.sites_domain }}
```

## Related

- [Services that find each other](../../learn/connecting-services.md)
- [Serve wildcard and tenant subdomains](wildcards.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
