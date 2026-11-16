---
title: Set CORS and cookies for each lane
description: Let a web app call its own lane's API, share a login cookie between the two, and keep both out of every other lane.
kind: how-to
---

Follow these steps when a web app and its API run on two hostnames of one lane and the browser calls the API with the user's cookie.

::example{slug="web-frontend-api"}

## Allow the lane's own origins

devlanes gives every service a variable, `DEVLANES_ORIGINS`, with the origins of its own lane, separated by commas. Build the API's CORS allow-list from it, so each lane allows its own pages and no other lane's:

```ts
import { cors } from "hono/cors";

const origins = (process.env.DEVLANES_ORIGINS ?? "").split(",").filter(Boolean);

export const laneCors = () => cors({ origin: origins, credentials: true });
```

[Environment variables](../../reference/environment-variables.md) has what the list holds.

## Share a cookie between the app and its API

A cookie set for the lane's domain reaches every hostname under it, so in lane 1 it reaches both myapp1.localhost and api.myapp1.localhost, and never lane 0. Pass the domain to the service that sets the cookie:

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

```ts
setCookie(c, "session", token, {
  domain: process.env.COOKIE_DOMAIN,
  sameSite: "Lax",
  secure: true,
});
```

Each lane then has its own login: signing in on one lane leaves you signed out on the others.

## Give each lane its own login callback

A login through an outside provider returns to a callback URL, and each lane needs its own. Build it from the service's public URL, and register each lane's URL with the provider:

```yaml devlanes
services:
  web:
    hosts: "${{ lane.domain }}"
    env:
      OAUTH_REDIRECT_URI: "${{ self.public_url }}/auth/callback"
```

## If the web app proxies its API calls

A dev server that forwards a path such as `/api` to the API makes every call same-origin, so the API needs no CORS and the cookie needs no domain. Point the forward at the API's local URL:

```yaml devlanes
services:
  web:
    env:
      API_PROXY_TARGET: ${{ services.api.url }}
```

## If tenant pages call the API

Tenant pages on a domain of their own, as in [Serve wildcard and tenant subdomains](wildcards.md), are another site: the session cookie does not go with their calls, and their origins are not in the variable.

## Related

- [Services that find each other](../../learn/connecting-services.md)
- [Serve wildcard and tenant subdomains](wildcards.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
