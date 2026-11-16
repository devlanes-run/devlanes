---
title: Run Cloudflare Workers
description: Run each lane's Workers on their own ports, inspector and local state, with Wrangler or the Cloudflare Vite plugin.
kind: how-to
---

Use this guide when a project runs Cloudflare Workers locally with Wrangler, and each lane needs its own Workers, bindings and stored data.

::example{slug="frameworks-wrangler-workers"}

## Add the Workers

The wrangler preset starts `wrangler dev` on the lane's port, with a second named port for the inspector. Each lane gets its own Wrangler dev registry, so a service binding reaches the Worker of the same lane. A template holds what the Workers share:

```yaml devlanes
templates:
  worker:
    preset: wrangler
    ready:
      http:
        path: /healthz
        timeout: 180s
    wrangler:
      args: [--persist-to, "${{ lane.data_path }}/wrangler/${{ self.name }}"]
services:
  gateway:
    extends: worker
    run_script:
      file: apps/gateway/package.json
      name: dev
    hosts:
      - "${{ lane.domain }}"
      - host: "inspect-gateway.${{ lane.domain }}"
        port: inspector
        host_header: upstream
    wrangler:
      vars:
        CATALOG_URL: "${{ services.catalog.public_url }}"
  catalog:
    extends: worker
    run_script:
      file: apps/catalog/package.json
      name: dev
    hosts: "catalog.${{ lane.domain }}"
```

Keep the `dev` script a plain `wrangler dev`, since the preset passes the ports itself. `--persist-to` keeps each lane's KV, D1 and R2 data in the lane data folder, a folder only that lane uses. A Worker does not see the process environment, so lane values reach it through [`services.<name>.wrangler.vars`](/docs/reference/config/project/services/#services-name-wrangler-vars). The inspector hostname needs `host_header: upstream`, because the inspector accepts only its own address.

## Start the lane

```console title="~/dev/myapp"
$ devlanes start
```

The start lists each Worker on its hostname, and the inspector on `inspect-gateway.myapp.localhost`.

## If a Worker fetches another by its hostname

On macOS and Linux, workerd looks lane hostnames up in DNS, so set [`lanes.hosts_file`](/docs/reference/config/project/lanes/#lanes-hosts_file) to `always` and run `devlanes setup` once the lane has started. On Windows nothing is needed.

## If you use the Cloudflare Vite plugin

Use the vite preset from [Run Vite and Next.js](vite-and-next.md), set `INSPECTOR_PORT: ${{ self.ports.inspector }}` in the service's `env`, and pass `Number(process.env.INSPECTOR_PORT)` to the plugin's `inspectorPort` in `vite.config.ts`.

## Related

- [Share settings and choose which services start](templates-and-profiles.md)
- [Give a service several ports and hostnames](../services/ports-and-hostnames.md)
- [Run a folder as two lanes](../lanes/two-lanes-one-folder.md)
