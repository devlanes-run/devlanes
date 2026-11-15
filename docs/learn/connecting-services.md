---
title: Services that find each other
description: myapp gains a web app that calls the API of its own lane, in every lane.
kind: learn
---

myapp gains a web app: a page served by Vite that asks the API which lane it belongs to. With two lanes up, the page is served twice, and each has to call the API of its own lane. No URL written into the code can be right for both.

## The web service names the API with an expression

The project file gains the web service and a task. Where the web app needs the API's URL, the file holds an expression instead: text between `${{` and `}}` that devlanes works out for each lane before the service starts. The API's hostname has been one from the start.

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

The web service takes the lane's domain itself as its hostname, so the page of lane 1 is at `https://myapp1.localhost`. Its environment reads the API's public URL from the same lane. [Expressions](../reference/expressions.md) lists what else an expression can read.

## Each lane hands the web app its own API

The variable comes out different in each lane. In the worktree of the feature branch, which took the new file by merging main, it holds the API of lane 1.

```console title="~/dev/myapp-checkout"
$ devlanes env show web
DEVLANES_LANE=1
DEVLANES_SERVICE=web
HOST=127.0.0.1
PORT=20071
VITE_API_URL=https://api.myapp1.localhost
```

In lane 0 the same line gives `https://api.myapp.localhost`. That is the URL for a browser: a hostname, over HTTPS, through the proxy.

A service has a second URL, for a call from another service of its lane. The expression `services.api.url` gives `http://127.0.0.1:20070` in lane 1, which goes straight to the API's port and skips the proxy and TLS. [When a service fails](failures.md) has a worker that uses it.

## The page uses the URL it is handed

Vite passes variables whose names start with `VITE_` to the page code, so the web app fetches whatever the lane gave it. The API has a part to play as well. The page and the API have different hostnames, so the browser reads the API's answer only if the API allows the page's origin.

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

const origin = req.headers.origin;
if (origin && origins.includes(origin)) {
  res.setHeader("access-control-allow-origin", origin);
}
```

devlanes sets `DEVLANES_ORIGINS` to the origins of the lane's own services. The API of lane 1 allows the page of lane 1 and sends no such header to the page of lane 0. [Set CORS and cookies for each lane](../how-to/hostnames/cors-and-cookies.md) goes further.

## A preset hands Vite its port as a flag

Vite does not read the port from the environment; it takes it on its command line. The preset line in the project file adds those flags to the dev script: the lane's port, the loopback address, and a flag that makes Vite exit instead of moving to another port when its port is taken.

```console title="~/dev/myapp-checkout"
$ devlanes logs web --tail 10
web#40   VITE v8.3.8  ready in 169 ms
web#41   ➜  Local:   http://127.0.0.1:20071/
web#42   ➜  press h + enter to show help
```

Vite listens on 20071, the port the environment above named, and the scripts in the web app's package file stay as they were. Presets exist for a few dev servers that take their port this way, Next.js among them.

## A task installs dependencies when a lane is new

A task runs a command to the end, where a service stays up. The install task runs on the lane's creation: at each start of the lane until it has succeeded once. A new worktree has no installed packages, so its first start installs them before any service starts.

```console title="~/dev/myapp-checkout"
$ devlanes start --no-ui
✓ install
✓ services ready
myapp@1 · feat/checkout
✓ services  2 of 2 ready
  ✓  api  https://api.myapp1.localhost
  ✓  web  https://myapp1.localhost
```

A start without unit names brings back the services the lane ran before. Lane 0 had run only the API, so a start there brought the API back and left the web app out. After a stop, a lane has nothing to bring back, and its next start runs both.

## On your own project

- [Run Vite and Next.js](../how-to/projects/vite-and-next.md)
- [Run installs, migrations and seeds](../how-to/services/tasks.md)
- [Set CORS and cookies for each lane](../how-to/hostnames/cors-and-cookies.md)
- [Environment and expressions](../concepts/environment.md)
