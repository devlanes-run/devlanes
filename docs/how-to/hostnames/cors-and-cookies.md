---
title: Set CORS and cookies for each lane
description: Let a web app call its own lane's API, share a login cookie between the two, and keep both out of every other lane.
kind: how-to
---

Follow these steps when a web app and its API run on two hostnames of one lane and the browser has to call the API with the user's cookie, without any other lane getting through.

::example{slug="web-frontend-api"}

## Allow the lane's own origins

devlanes gives every service a variable listing the origins of its lane: the HTTPS hostname of each service, and the local address of each one. Build the API's CORS allow-list from it instead of writing hostnames down, so each lane allows its own pages and no other lane's.

```ts
import { cors } from "hono/cors";

const origins = (process.env.DEVLANES_ORIGINS ?? "").split(",").filter(Boolean);

export const laneCors = () =>
  cors({ origin: origins, credentials: true });
```

The credentials option lets the browser send the session cookie with these requests. The variable belongs to the process, so read it in code; an expression in the project file cannot read it.

## Check what a lane allows

The environment of a service shows the list it gets. In lane 1 of myapp it names only lane 1's hostnames:

```console title="~/dev/myapp-checkout"
$ devlanes env show web | grep -E '^(PORT|HOST|VITE_[A-Z_]*|DEVLANES_ORIGINS|DEVLANES_LANE|DEVLANES_SERVICE)='
DEVLANES_ORIGINS=https://api.myapp1.localhost,http://localhost:20070,http://127.0.0.1:20070,https://myapp1.localhost,http://localhost:20071,http://127.0.0.1:20071
VITE_API_URL=https://api.myapp1.localhost
```

When the proxy runs on its fallback port, the HTTPS origins carry that port, the way a browser sends them. [Environment variables](../../reference/environment-variables.md) describes the variable in full.

## Share a cookie between the app and its API

The app at myapp1.localhost and the API at api.myapp1.localhost are one site, so a cookie set for the lane's domain reaches both. Pass the domain to the service that sets the cookie:

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

Lane 0 is a different site, so the cookie never reaches it. [Hostnames and HTTPS](../../concepts/hostnames-and-https.md#each-lane-is-its-own-site) explains why.

## Sign in to each lane

Because each lane is its own site, signing in on lane 0 does not sign you in on lane 1, and signing out of one leaves the other alone. That lets you test two users side by side, one per lane.

A login through an outside provider sends the browser back to a callback URL, and each lane needs its own. Build it from the service's public URL and register each lane's URL with the provider:

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

The variable leaves out wildcard hostnames. An API called from tenant pages checks the origin against the tenant domain itself, as [Serve wildcard and tenant subdomains](wildcards.md) sets it up.

## Related

- [Services that find each other](../../learn/connecting-services.md)
- [Serve wildcard and tenant subdomains](wildcards.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
