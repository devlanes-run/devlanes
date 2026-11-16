---
title: Services that find each other
description: myapp gains a web app that calls the API of its own lane, in every lane.
kind: learn
---

myapp gains a web app: a page served by Vite that asks the API which lane it belongs to. Each lane serves the page, and each page has to call the API of its own lane, so no URL written into the code can be right for both.

## The web app takes the lane's domain

The project file gains the web service and an install step for the packages. Where the web app needs the API's URL, the file holds an expression that reads the API's public URL. Like the expression in each hostname, devlanes works it out for each lane before the service starts. [Expressions](../reference/expressions.md) lists what else an expression can read.

```yaml devlanes
# devlanes.yaml
project: myapp

tasks:
  install:
    run: [npm, install]
    when: lane.create

services:
  api:
    run_script:
      file: api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz

  web:
    preset: vite
    run_script:
      file: web/package.json
      name: dev
    hosts: "${{ lane.domain }}"
    env:
      VITE_API_URL: "${{ services.api.public_url }}"
```

The web service takes the lane's domain itself as its hostname. That domain has been in the lane's row of the status table from the start with nothing on it, and the web app is what answers there.

## A start that brings a lane up runs its tasks

Both lanes have been stopped since the evening before, when they still ran the API alone. That matters, because a start into a running lane adds the services it lacks and runs no tasks. The web app lands on the main branch and is merged into the checkout branch, and each lane starts again.

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ install
✓ services ready
myapp@0 · main
✓ services  2 of 2 ready
  ✓  api  https://api.myapp.localhost
  ✓  web  https://myapp.localhost
```

The install is a task: a unit that runs a command and ends, where a service keeps running. As a `lane.create` task it runs at each start that brings the lane up until it has succeeded once in that lane, and again after its entry in the file changes. The task is new to the file, so lane 0 has never run it, and this start runs it before any service. One install at the root of the npm workspace covers the API and the web app.

## Each lane hands the web app its own API

The expression comes out different in each lane. In the checkout worktree it gives the API of lane 1.

```console title="~/dev/myapp-checkout"
$ devlanes env show web
DEVLANES_LANE=1
DEVLANES_ORIGINS=https://api.myapp1.localhost,http://localhost:20070,http://127.0.0.1:20070,https://myapp1.localhost,http://localhost:20072,http://127.0.0.1:20072
DEVLANES_SERVICE=web
HOST=127.0.0.1
PORT=20072
VITE_API_URL=https://api.myapp1.localhost
```

That is the URL for a browser: the API's hostname, over HTTPS, through the proxy. A service has a second URL, for a call from another service of its lane, which `services.api.url` gives: the API's port on the loopback address, with neither the proxy nor TLS in between. The web app's code runs in the browser, so it takes the first.

## The page calls the URL it is handed

Vite passes variables whose names start with `VITE_` to the page code, so the web app fetches whatever URL its lane gives it. The page and the API have different hostnames, so the browser reads the API's answer only if the API allows the page's origin. The API reads its lane's origins from `DEVLANES_ORIGINS`, shown above, and checks each request against them.

```js
// web/main.js
const apiUrl = import.meta.env.VITE_API_URL;
const el = document.getElementById("lane");

fetch(apiUrl)
  .then((res) => res.json())
  .then((body) => {
    el.textContent = `The API at ${apiUrl} answers from lane ${body.lane} (${body.lane_name}).`;
  })
  .catch((err) => {
    el.textContent = `Could not reach the API at ${apiUrl}: ${err}`;
  });

// api/server.mjs
const origins = (process.env.DEVLANES_ORIGINS ?? "").split(",").filter(Boolean);

const server = createServer((req, res) => {
  const origin = req.headers.origin;
  if (origin && origins.includes(origin)) {
    res.setHeader("access-control-allow-origin", origin);
    res.setHeader("vary", "origin");
  }
```

The list holds the lane's hostnames over HTTPS and each of its ports at localhost and 127.0.0.1. The API of lane 1 allows the page of lane 1 and sends no such header to the page of lane 0.

## A preset hands Vite its port as a flag

A preset is a set of flags and settings devlanes adds for a dev server it knows. Vite takes its port on its command line and ignores `PORT`, so the web service's preset adds the lane's port, a flag that makes Vite exit when that port is taken instead of moving to another one, and the loopback address.

```yaml devlanes
services:
  web:
    preset: vite
    run_script:
      file: web/package.json
      name: dev
```

The web app's package file keeps its plain `vite` script; devlanes adds the flags when it runs it.

## Both lanes stop before the database arrives

The next change brings a task that has to run at each start, and only a start that brings a lane up runs tasks, so both lanes stop first.

```console title="~/dev/myapp"
$ devlanes stop
stopped myapp@0:api
stopped myapp@0:web
```

```console title="~/dev/myapp-checkout"
$ devlanes stop
stopped myapp@1:api
stopped myapp@1:web
```

The API is about to keep data in Postgres, and [A database for each lane](databases.md) gives each lane a database of its own, so that a migration on one branch leaves the other's data alone.

## On your own project

- [Run Vite and Next.js](../how-to/projects/vite-and-next.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Set CORS and cookies for each lane](../how-to/hostnames/cors-and-cookies.md)
- [Environment and expressions](../concepts/environment.md)
