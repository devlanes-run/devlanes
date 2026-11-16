---
title: Run Vite and Next.js
description: Put a Vite or Next.js dev server on its lane's port and hostname.
kind: how-to
---

Use this guide when a web app runs on a Vite or Next.js dev server and each lane should serve its own copy under the lane's hostname.

::example{slug="web-next-app"}

## Add the preset

A preset is the part of devlanes that knows a framework's flags: it passes the lane's port to the dev server and binds it to 127.0.0.1. Name it on the service, next to the script that starts the dev server:

::::tabs
:::tab[Vite]
```yaml devlanes
services:
  web:
    preset: vite
    run_script:
      file: web/package.json
      name: dev
    hosts: "${{ lane.domain }}"
```
:::
:::tab[Next.js]
```yaml devlanes
services:
  web:
    preset: next
    run_script:
      file: web/package.json
      name: dev
    hosts: "${{ lane.domain }}"
    ready:
      http:
        path: /
        timeout: 120s
```

next dev compiles a page on its first request, so the ready check waits longer than the default.
:::
::::

The preset adds its flags at the end of the script, so the script's last command has to be the dev server itself, as in `"dev": "vite"` or `"dev": "next dev"`.

## Start the lane

```console title="~/dev/myapp"
$ devlanes start
```

The start lists the web app on the lane's hostname, such as `https://myapp.localhost`. With Vite, hot reload works through the proxy.

## If lane domains use another top-level domain

Both dev servers accept `localhost` names without a setting. With another domain set through [`hostnames.tld`](/docs/reference/config/user/hostnames/#hostnames-tld), list it in `server.allowedHosts` for Vite, such as `[".test"]`, or in `allowedDevOrigins` for Next.js, such as `["**.test"]`.

## If the dev server proxies API calls

Point the proxy at `${{ services.api.url }}`, the API's own port, rather than at its hostname. A proxy that sends a lane hostname back to devlanes reaches the same service again and gets [DL104 Proxy loop](../../errors/proxy-loop.md).

## Related

- [Services that find each other](../../learn/connecting-services.md)
- [Run scripts your project already has](run-scripts.md)
- [Run dev servers and a production build side by side](dev-and-prod.md)
- [Wait until a service is ready](../services/ready-checks.md)
