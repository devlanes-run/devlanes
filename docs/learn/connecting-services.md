---
title: Services that find each other
description: myapp gains a web app that calls the API of its own lane, in every lane.
kind: learn
---

You add a web app to myapp: a page served by Vite that asks the API which lane it belongs to. Each lane serves the page, and each page has to call the API of its own lane, so no URL written into the code can be right for both.

## The web app takes the lane's domain

The project file gains the web service and an install step for the packages, the marked lines below. Where the web app needs the API's URL, the file holds an expression that reads the API's public URL. Like the expression in each hostname, devlanes works it out for each lane before the service starts. [Expressions](../reference/expressions.md) lists what else an expression can read.

```yaml devlanes ins={4-7,19-26}
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

The web service takes the lane's domain itself as its hostname, so the web app answers on the domain that has had nothing on it until now. Vite takes its port on its command line and ignores `PORT`, so the web service names a preset, a set of flags and settings devlanes adds for a dev server it knows. The vite preset passes Vite the lane's port, a flag that makes Vite exit when that port is taken instead of moving to another, and the loopback address.

## Starting a stopped lane runs its tasks

Both lanes are stopped. You add the web app on the main branch, merge it into the checkout branch and start each lane. A start of a stopped lane runs the lane's tasks; a start into a running lane adds the services the lane lacks and runs no tasks, which is why the lanes are stopped first.

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ install
✓ services ready
myapp@0 · main
✓ services  2 of 2 ready
  ✓  api  https://api.myapp.localhost
  ✓  web  https://myapp.localhost
```

```console title="~/dev/myapp-checkout"
$ devlanes start --no-ui
✓ install
✓ services ready
myapp@1 · feat/checkout
✓ services  2 of 2 ready
  ✓  api  https://api.myapp1.localhost
  ✓  web  https://myapp1.localhost
```

The install is a task: a unit that runs a command and ends, while a service keeps running. As a `lane.create` task it runs at each start of a stopped lane until it has succeeded once in that lane, and again after its entry in the file changes. Neither lane has run it yet, so each start runs it before any service, and one `npm install` at the root, which lists the API and the web app as npm workspaces, installs the packages of both.

## Each lane hands the web app its own API

These are some of the variables the web app gets in the checkout worktree: its lane, its port and the API's URL that the expression gave it.

```console title="~/dev/myapp-checkout"
$ devlanes env show web
DEVLANES_LANE=1
DEVLANES_SERVICE=web
HOST=127.0.0.1
PORT=20072
VITE_API_URL=https://api.myapp1.localhost
```

That is the URL for a browser: the API's hostname, over HTTPS, through the proxy. A call from another service of the lane would use `services.api.url` instead, the API's port on the loopback address with neither the proxy nor TLS in between. The web app's code runs in the browser, so it takes the first.

## The API answers the page of its own lane

Vite passes variables whose names start with `VITE_` to the page code, so the web app fetches the URL its lane gives it. The page and the API have different hostnames, so the browser reads the API's answer only if the API allows the page's origin. devlanes lists the lane's origins for every service in `DEVLANES_ORIGINS`, shown here among the variables the API of lane 0 gets, with the layer each comes from.

```console title="~/dev/myapp"
$ devlanes env show api --explain
KEY               VALUE                                                                                                                     LAYER
DEVLANES_LANE     0                                                                                                                         devlanes
DEVLANES_ORIGINS  https://api.myapp.localhost,http://localhost:20060,http://127.0.0.1:20060,https://myapp.localhost,http://localhost:20...  devlanes
DEVLANES_SERVICE  api                                                                                                                       devlanes
PORT              20060                                                                                                                     devlanes
```

The list holds the lane's hostnames over HTTPS and each of its ports at localhost and 127.0.0.1; the explain view cuts a long value short with dots. The page fetches its URL, and the API checks each request's origin against the list inside its request handler.

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

## You stop both lanes before the database arrives

The database brings a task that has to run at each start, and only a start of a stopped lane runs tasks, so you stop both lanes first.

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

The API is about to keep data in Postgres, and [A database for each lane](databases.md) gives each lane a database of its own.

## On your own project

- [Run Vite and Next.js](../how-to/projects/vite-and-next.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Set CORS and cookies for each lane](../how-to/hostnames/cors-and-cookies.md)
- [Environment and expressions](../concepts/environment.md)
