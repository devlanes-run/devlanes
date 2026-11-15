---
title: Run Vite and Next.js
description: Put a Vite or Next.js dev server on its lane's port and hostname, with hot reload through the HTTPS proxy.
kind: how-to
---

Use this guide when a web app runs on a Vite or Next.js dev server and each lane should serve its own copy under the lane's hostname.

::example{slug="web-next-app"}

## Keep the dev script plain

devlanes starts the dev server from a script in `package.json` and adds the port flags after the script name, in the form your package manager expects. They reach the last command of the script, so that command has to be the dev server itself.

```json title="web/package.json"
{
  "scripts": {
    "dev": "vite"
  }
}
```

A script such as `vite && echo done` would hand the flags to `echo`. [Run scripts your project already has](run-scripts.md) covers the other runners.

## Add the preset

A preset is the part of devlanes that knows a framework's flags. You name it on the service, next to the script:

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

The preset passes the lane's port, binds the server to 127.0.0.1 and adds `--strictPort`. With that flag Vite exits when its port is taken, where it would otherwise move to the next free port and leave the proxy pointing at nothing.
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

Next reads `PORT` by itself, so it finds its port even without the preset. The preset passes the port as a flag as well and binds the server to 127.0.0.1, since Next otherwise listens on every network interface. The first request makes next dev compile the page, which is why the ready check waits longer than the default.
:::
::::

`devlanes config check` refuses a preset on a service that uses `run` instead of a script. The last section shows the flags to write yourself in that case.

## Allow the lane hostnames

The proxy passes the hostname the browser asked for, such as `myapp1.localhost`, on to the dev server, and both frameworks check it.

::::tabs
:::tab[Vite]
Vite accepts `localhost` and its subdomains without a setting. When lane domains use another top-level domain through [`hostnames.tld`](/docs/reference/config/user/hostnames/), list that domain:

```ts title="web/vite.config.ts"
export default defineConfig({
  server: { allowedHosts: [".test"] },
});
```
:::
:::tab[Next.js]
Recent Next releases accept `localhost` and its subdomains. Some earlier ones warn about or block a hostname with several labels, such as `web.myapp1.localhost`. On those, and with another top-level domain on every release, list the domain:

```js title="web/next.config.js"
module.exports = { allowedDevOrigins: ["**.localhost"] };
```
:::
::::

Open the app on its hostname and hot reload works as it does on `localhost`. The page's update client connects back to the host it was loaded from, and the proxy carries that WebSocket like any other request.

## If the dev server proxies API calls

Point the dev server's proxy at the API's port, which `services.api.url` gives, rather than at the API's hostname. A proxy that sends a lane hostname back to devlanes with the original Host header reaches the same service again, and the proxy answers with [DL104 Proxy loop](../../errors/proxy-loop.md).

```yaml devlanes
services:
  web:
    env:
      API_PROXY_TARGET: "${{ services.api.url }}"
```

## If two lanes of Next.js run from one folder

Lanes of one folder share the `.next` build folder, and a second next dev exits while the first holds it. Give each lane its own build folder, which has to stay inside the app:

```js title="web/next.config.js"
module.exports = { distDir: process.env.NEXT_DIST_DIR || ".next" };
```

```yaml devlanes
services:
  web:
    env:
      NEXT_DIST_DIR: ".next/lane${{ lane.index }}"
```

[Run a folder as two lanes](../lanes/two-lanes-one-folder.md) sets up such lanes.

## If the command cannot be a script

Drop the preset and write its flags into `run` with the lane's port:

```yaml devlanes
services:
  web:
    run: npx vite --port ${{ self.port }} --strictPort --host 127.0.0.1
    hosts: "${{ lane.domain }}"
```

For Next.js the flags are `--port` and `--hostname 127.0.0.1` after `next dev`.

## Related

- [Services that find each other](../../learn/connecting-services.md)
- [Run dev servers and a production build side by side](dev-and-prod.md)
- [Wait until a service is ready](../services/ready-checks.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
