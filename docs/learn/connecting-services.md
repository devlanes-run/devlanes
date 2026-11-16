---
title: Services that find each other
description: myapp gains a web app that calls the API of its own lane, in every lane.
kind: learn
---

You add a web app to myapp: a page served by Vite that asks the API which lane it belongs to. Each lane serves the page, and each page has to call the API of its own lane, so no URL written into the code can be right for both.

## The web app takes the lane's domain

On the main branch you add the web service and a task that installs the packages. Where the web app needs the API's URL, the file holds an expression that reads the API's public URL; like the one in each hostname, devlanes works it out for each lane before the service starts.

```yaml devlanes
tasks:
  install:
    run: [npm, install]
    when: lane.create

services:
  web:
    preset: vite
    run_script:
      file: web/package.json
      name: dev
    hosts: "${{ lane.domain }}"
    env:
      VITE_API_URL: "${{ services.api.public_url }}"
```

The web service takes the lane's domain itself as its hostname, which had nothing on it until now. Vite takes its port as a flag and ignores `PORT`, so the web service names a preset, a set of flags and settings devlanes adds for a dev server it knows; the vite preset passes the lane's port, the loopback address and a flag that makes Vite exit when that port is taken.

## Starting a stopped lane runs its tasks

Both lanes are stopped. You merge main into the checkout branch and start each lane.

```console title="~/dev/myapp"
$ devlanes start --no-ui
```

```console title="~/dev/myapp-checkout"
$ devlanes start --no-ui
```

Each start runs the install, then the API and the web app, whose page is at `https://myapp.localhost` in lane 0 and `https://myapp1.localhost` in lane 1. The install is a task, a unit that runs a command and ends. As a `lane.create` task it runs at each start of a stopped lane until it has succeeded once in that lane. A start into a running lane adds the services it lacks and runs no tasks, so you stop a lane before a start that has to run them.

## Each lane hands the web app its own API

```console title="~/dev/myapp-checkout"
$ devlanes env show web
```

In lane 1 the web app gets `VITE_API_URL=https://api.myapp1.localhost`, the URL a browser uses, through the proxy. A call from another service of the lane would use `services.api.url` instead, the API's port on the loopback address.

## The API answers the page of its own lane

Vite passes variables whose names start with `VITE_` to the page code, so the page fetches the URL its lane gives it. The page and the API have different hostnames, so the browser reads the API's answer only if the API allows the page's origin. devlanes gives every service of a lane the lane's origins in `DEVLANES_ORIGINS`, and the API checks each request against it.

```js
// web/main.js
const apiUrl = import.meta.env.VITE_API_URL;
const el = document.getElementById("lane");

fetch(apiUrl)
  .then((res) => res.json())
  .then((body) => {
    el.textContent = `The API at ${apiUrl} answers from lane ${body.lane} (${body.lane_name}).`;
  });

// api/server.mjs
const origins = (process.env.DEVLANES_ORIGINS ?? "").split(",").filter(Boolean);

const server = createServer((req, res) => {
  const origin = req.headers.origin;
  if (origin && origins.includes(origin)) {
    res.setHeader("access-control-allow-origin", origin);
  }
```

The API of lane 1 allows the page of lane 1 and sends no such header to the page of lane 0.

The API is about to keep data in Postgres, and [A database for each lane](databases.md) gives each lane a database of its own.

## On your own project

- [Run Vite and Next.js](../how-to/projects/vite-and-next.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Set CORS and cookies for each lane](../how-to/hostnames/cors-and-cookies.md)
- [Environment and expressions](../concepts/environment.md)
