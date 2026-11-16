---
title: Run Vite and Next.js
description: Put a Vite or Next.js dev server on its lane's port and hostname.
kind: how-to
---

Use this guide when a web app runs on a Vite or Next.js dev server and each lane should serve its own copy under the lane's hostname.

::example{slug="web-next-app"}

## Add the preset

A preset is the part of devlanes that knows a framework's flags. You name it on the service, next to the script that starts the dev server:

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

The preset passes the lane's port, binds the server to 127.0.0.1 and adds `--strictPort`. With that flag Vite exits when its port is taken, where it would otherwise move to the next free port and leave the proxy pointing at a port the server no longer uses.
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

`devlanes config check` refuses a preset on a service that uses `run` instead of a script. [If the command cannot be a script](#if-the-command-cannot-be-a-script) has the flags to write yourself.

## Keep the dev script plain

The preset adds its flags after the script name, in the form your package manager expects. They reach the last command of the script, so that command has to be the dev server itself.

```json title="web/package.json"
{
  "scripts": {
    "dev": "vite"
  }
}
```

A script such as `vite && echo done` would hand the flags to `echo`. [Run scripts your project already has](run-scripts.md) covers the other runners.

## Allow the lane hostnames

The proxy passes the hostname the browser asked for, such as `myapp1.localhost`, on to the dev server, and both frameworks refuse names they do not know.

::::tabs
:::tab[Vite]
Vite accepts `localhost` and its subdomains without a setting. When lane domains use another top-level domain through [`hostnames.tld`](/docs/reference/config/user/hostnames/#hostnames-tld), list that domain:

```ts title="web/vite.config.ts"
export default defineConfig({
  server: { allowedHosts: [".test"] },
});
```
:::
:::tab[Next.js]
Next.js checks where requests for its dev assets come from, and allows `localhost` and its subdomains without a setting. When lane domains use another top-level domain, or when next dev warns about or blocks a cross-origin request from a lane hostname, list the domain:

```js title="web/next.config.js"
module.exports = { allowedDevOrigins: ["**.test"] };
```
:::
::::

## Start the lane

The start runs the dev script with the preset's flags and lists the dev server on the lane's hostname. In [Next.js app with an HTTPS sibling API](/examples/web-next-app/), the web app sits next to its API:

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ install
✓ services ready
myapp@0 · main
✓ services  2 of 2 ready
  ✓  web  https://myapp.localhost
  ✓  api  https://api.myapp.localhost
```

Open the app on its hostname. With Vite, hot reload works through the proxy: its update client connects back to the host the page came from, and the proxy carries that WebSocket like any other request.

## If the dev server proxies API calls

Point the dev server's proxy at the API's port, which `services.api.url` gives, rather than at the API's hostname. A proxy that sends a lane hostname back to devlanes with the original Host header reaches the same service again, and the proxy answers with [DL104 Proxy loop](../../errors/proxy-loop.md).

```yaml devlanes
services:
  api:
    hosts: "api.${{ lane.domain }}"
  web:
    env:
      API_PROXY_TARGET: "${{ services.api.url }}"
```

The Vite config reads the variable when the dev server starts:

```ts title="web/vite.config.ts"
export default defineConfig({
  server: { proxy: { "/api": process.env.API_PROXY_TARGET } },
});
```

## If two lanes of Next.js run from one folder

Lanes of one folder share the `.next` build folder, so the second lane's next dev fails, and its log names the server that holds the folder. Here the second lane is named review:

```console title="~/dev/myapp"
$ devlanes logs review:web --tail 30
web#1 ▲ Next.js 16.4.5 (Turbopack)
web#2 - Local:         http://127.0.0.1:20091
web#6 ⨯ Another next dev server is already running.
web#8 - Local:        http://127.0.0.1:20081
web#9 - PID:          51872
```

Give each lane a build folder of its own, next to `.next` inside the app:

```js title="web/next.config.js"
module.exports = { distDir: process.env.NEXT_DIST_DIR || ".next" };
```

```yaml devlanes
services:
  web:
    env:
      NEXT_DIST_DIR: ".next-lane${{ lane.index }}"
```

Git does not ignore the new folders yet, so add `.next-lane*` to your `.gitignore`. [Run a folder as two lanes](../lanes/two-lanes-one-folder.md) sets up such lanes.

## If the command cannot be a script

Drop the preset and write its flags into `run` with the lane's port:

::::tabs
:::tab[Vite]
```yaml devlanes
services:
  web:
    run: npx vite --port ${{ self.port }} --strictPort --host 127.0.0.1
    hosts: "${{ lane.domain }}"
```
:::
:::tab[Next.js]
```yaml devlanes
services:
  web:
    run: npx next dev --port ${{ self.port }} --hostname 127.0.0.1
    hosts: "${{ lane.domain }}"
```
:::
::::

## Related

- [Services that find each other](../../learn/connecting-services.md)
- [Run dev servers and a production build side by side](dev-and-prod.md)
- [Wait until a service is ready](../services/ready-checks.md)
- [Hostnames and HTTPS](../../concepts/hostnames-and-https.md)
