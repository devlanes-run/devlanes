---
title: Services that find each other
description: myapp gains a web app that calls the API of its own lane, in every lane.
kind: learn
---

myapp gains a web app: a page served by Vite that asks the API which lane it belongs to. Each lane serves the page, and each page has to call the API of its own lane, so no URL written into the code can be right for both.

## The web app takes the lane's domain

The project file gains the web service and a task, a unit that runs a command to the end where a service keeps running. Where the web app needs the API's URL, the file holds an expression that reads the API's public URL. Like the expression in each hostname, devlanes works it out for each lane before a unit starts. [Expressions](../reference/expressions.md) lists what else an expression can read.

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

Both lanes were stopped the evening before, while they still ran the API alone. Then the web app was added on the main branch and merged into the checkout branch, and each lane was started again.

```console title="~/dev/myapp"
$ devlanes stop
stopped myapp@0:api
$ devlanes start --no-ui
✓ install
✓ services ready
myapp@0 · main
✓ services  2 of 2 ready
  ✓  api  https://api.myapp.localhost
  ✓  web  https://myapp.localhost
```

The install is a `lane.create` task. It runs at each start that brings the lane up until it has succeeded once in that lane, and again after its entry in the file changes. Lane 0 had never run it, so it runs now, before any service starts, and one install at the root of the npm workspace covers the API and the web app.

Here the start brings a stopped lane up. A start into a running lane adds the services it lacks, such as one just added to the file, and runs no tasks, so the web app would have started without its packages.

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

Vite passes variables whose names start with `VITE_` to the page code, so the web app fetches whatever URL the lane gave it. The page and the API have different hostnames, so the browser reads the API's answer only if the API allows the page's origin. The API reads the list of its lane's origins from `DEVLANES_ORIGINS`, shown above, and checks each request against it.

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
    res.setHeader("vary", "origin");
  }
```

The list holds the lane's hostnames over HTTPS and each of its ports at localhost and 127.0.0.1. The API of lane 1 allows the page of lane 1 and sends no such header to the page of lane 0.

## A preset hands Vite its port as a flag

Vite takes its port on its command line and ignores `PORT`. The preset line in the project file adds the flags to the dev script: the lane's port, a flag that makes Vite exit when that port is taken instead of moving to another one, and the loopback address.

```console title="~/dev/myapp, on Windows 11"
$ devlanes env show web
DEVLANES_NPM_SCRIPT="vite --port 20062 --strictPort --host 127.0.0.1"
PORT=20062
VITE_API_URL=https://api.myapp.localhost
```

On Windows, `DEVLANES_NPM_SCRIPT` holds the script as devlanes runs it, which is where the flags show. The web app's package file still says plain vite. In lane 0 the web app has a port of its own and the API URL of lane 0.

The API is about to keep data in Postgres, and [A database for each lane](databases.md) gives each lane a database of its own, so that a migration on one branch leaves the other's data alone.

## On your own project

- [Run Vite and Next.js](../how-to/projects/vite-and-next.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Set CORS and cookies for each lane](../how-to/hostnames/cors-and-cookies.md)
- [Environment and expressions](../concepts/environment.md)
