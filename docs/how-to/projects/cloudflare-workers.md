---
title: Run Cloudflare Workers
description: Run each lane's Workers on their own ports, inspector and local state, with Wrangler or the Cloudflare Vite plugin.
kind: how-to
---

Use this guide when a project runs Cloudflare Workers locally with Wrangler or the Cloudflare Vite plugin, and each lane needs its own Workers, bindings and stored data.

::example{slug="frameworks-wrangler-workers"}

## Add each Worker as a service

The wrangler preset starts `wrangler dev` on the lane's port and gives it a second port for the inspector. A template holds what the Workers share, and each service extends it with its own script:

```yaml devlanes
templates:
  worker:
    preset: wrangler
    ready:
      http:
        path: /healthz
        timeout: 180s
services:
  gateway:
    extends: worker
    run_script:
      file: apps/gateway/package.json
      name: dev
    hosts: "${{ lane.domain }}"
  catalog:
    extends: worker
    run_script:
      file: apps/catalog/package.json
      name: dev
    hosts: "catalog.${{ lane.domain }}"
```

The script stays a plain `wrangler dev`. A script that passes its own port or inspector port is refused at start, since the preset sets both for each lane. A default you want outside devlanes goes into the Wrangler config, where the preset's flags override it.

## Pass lane values as Worker variables

A Worker does not see the environment of the process that runs it. The preset turns each entry of [`services.<name>.wrangler.vars`](/docs/reference/config/project/services/#services-name-wrangler-vars) into a `--var` flag, which wins over the same name in the Wrangler config:

```yaml devlanes
services:
  gateway:
    wrangler:
      vars:
        PUBLIC_BASE_URL: "${{ self.public_url }}"
        CATALOG_URL: "${{ services.catalog.public_url }}"
```

## Keep local state per lane

KV, D1, R2 and Durable Objects keep their local data under `.wrangler/` in the Worker's folder. Lanes in separate worktrees keep separate copies there, but two lanes of one folder would share them. Send the data to the lane data folder, a folder outside the worktree that only one lane uses:

```yaml devlanes
templates:
  worker:
    wrangler:
      args: [--persist-to, "${{ lane.data_path }}/wrangler/${{ self.name }}"]
```

A service that sets its own `args` replaces this list, so repeat `--persist-to` there.

## Route the inspector

The inspector port is a named port called `inspector`, allocated for each Worker in each lane. A hostname reaches it when the route names that port. The inspector refuses a Host header other than its own address, so the route sends the local address instead:

```yaml devlanes
services:
  gateway:
    hosts:
      - "${{ lane.domain }}"
      - host: "inspect-gateway.${{ lane.domain }}"
        port: inspector
        host_header: upstream
```

[Give a service several ports and hostnames](../services/ports-and-hostnames.md) covers named ports and routes.

## Bind the Workers to each other

A service binding names the other Worker, and needs nothing from devlanes:

```jsonc title="apps/gateway/wrangler.jsonc"
{
  "services": [{ "binding": "CATALOG", "service": "catalog" }]
}
```

Each lane has its own Wrangler dev registry in its data folder, and devlanes points every service and task of the lane at it, so the binding reaches the catalog of the same lane. Setting `WRANGLER_REGISTRY_PATH` or a variable like it in the project file overrides the lane's registry, so leave them out.

## Start the lane

The start brings up both Workers on their hostnames:

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ install
✓ services ready
myapp@0 · main
✓ services  2 of 2 ready
  ✓  gateway  https://myapp.localhost
  ✓  catalog  https://catalog.myapp.localhost
```

With the same inspector route on the catalog, the list of routed hostnames shows each inspector hostname on its Worker's inspector port:

```console title="~/dev/myapp"
$ devlanes url ls
HOST                                 TARGET           PORT   STATE
myapp.localhost                  ->  myapp@0:gateway  20102  ready
inspect-gateway.myapp.localhost  ->  myapp@0:gateway  20103  ready
catalog.myapp.localhost          ->  myapp@0:catalog  20100  ready
inspect-catalog.myapp.localhost  ->  myapp@0:catalog  20101  ready
proxy: http :80, https :443
```

## If a Worker fetches another by hostname

workerd looks the name up itself, and the request goes over HTTPS, so workerd has to resolve the lane hostname and accept the devlanes certificate.

::::tabs{sync="os"}
:::tab[macOS]
workerd sends `*.localhost` names to DNS. Have devlanes write the lane's hostnames to the hosts file:

```yaml devlanes
lanes:
  hosts_file: always
```

Writing the file needs the hosts helper. When `devlanes setup` skipped it because macOS resolves `*.localhost` itself, run it again once the lane has started, and it installs the helper the project now needs. Wildcard hostnames cannot go in the hosts file.
:::
:::tab[Linux]
workerd sends `*.localhost` names to DNS. Have devlanes write the lane's hostnames to the hosts file:

```yaml devlanes
lanes:
  hosts_file: always
```

Writing the file needs the hosts helper. When `devlanes setup` skipped it because your system resolves `*.localhost` itself, run it again once the lane has started, and it installs the helper the project now needs. Wildcard hostnames cannot go in the hosts file.
:::
:::tab[Windows]
Nothing to set. Windows 11 answers `*.localhost` names with loopback, devlanes does not write the hosts file on Windows, and workerd accepts the devlanes certificate once the CA is trusted. In [Cloudflare Workers](/examples/frameworks-wrangler-workers/), the gateway's fetch returns the catalog of its own lane:

```console title="on Windows 11"
$ curl.exe -sS -i --ssl-revoke-best-effort https://myapp.localhost/api/catalog-by-hostname
HTTP/1.1 200 OK
content-length: 165
content-type: application/json
via: 1.1 devlanes.run

{"gateway_lane":0,"catalog":{"project":"myapp","lane":0,"lane_name":"main","branch":"main","service":"catalog","port":20100,"url":"https://catalog.myapp.localhost"}}
```
:::
::::

When the fetch fails on the certificate, [Trust the devlanes CA](../hostnames/trust-the-ca.md) covers the stores to add it to. A Worker that only needs to reach the other one can take `services.catalog.url` instead, the catalog's own port over plain http, which needs neither the name nor the certificate.

## If you use the Cloudflare Vite plugin

The plugin runs the Worker inside Vite, so the service uses the vite preset from [Run Vite and Next.js](vite-and-next.md). The plugin opens its inspector on a port of its own choosing, so ask for a named port and hand it over:

```yaml devlanes
services:
  web:
    preset: vite
    run_script:
      file: package.json
      name: dev
    env:
      INSPECTOR_PORT: "${{ self.ports.inspector }}"
```

```ts title="vite.config.ts"
import { cloudflare } from "@cloudflare/vite-plugin";
import { defineConfig } from "vite";

const env = process.env;
const statePath = env.DEVLANES_LANE_DATA && `${env.DEVLANES_LANE_DATA}/cloudflare-state`;

export default defineConfig({
  plugins: [
    cloudflare({
      inspectorPort: env.INSPECTOR_PORT ? Number(env.INSPECTOR_PORT) : undefined,
      persistState: statePath ? { path: statePath } : true,
    }),
  ],
});
```

The `persistState` line keeps the plugin's state in the lane data folder, as `--persist-to` does for Wrangler. Route the inspector as above.

## Related

- [Share settings and choose which services start](templates-and-profiles.md)
- [Run a folder as two lanes](../lanes/two-lanes-one-folder.md)
- [Files and folders](../../reference/files.md)
- [Ports](../../concepts/ports.md)
