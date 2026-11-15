---
title: A service, a port and a hostname
description: One service of myapp starts under devlanes and answers on an HTTPS hostname.
kind: learn
---

These pages follow one project, `myapp`, from a single service to copies of the whole app running on another machine. This first page takes its API from a command you would type in a terminal to a service with a hostname, a certificate and a place in a status table.

## One file describes the project

The file sits at the root of the repository and is committed, so every branch has it. It names the project and lists its services. A service is a long-running process, here the API's dev script.

```yaml devlanes
project: myapp

services:
  api:
    run_script:
      file: api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz
```

The file never mentions a port. It says which script to run, which hostname the service wants and how to tell that it works.

## A start gives the service a port and a hostname

A start runs the script, waits until the health route answers and prints the address.

```console title="~/dev/myapp"
$ devlanes start --no-ui
✓ services ready
myapp@0 · main
✓ services  1 of 1 ready
  ✓  api  https://api.myapp.localhost
```

devlanes chose a free port and passed it to the API in the `PORT` variable. The API listens there, and the proxy puts the hostname in front of it.

In a terminal the same start runs inside the TUI, a full-screen view of every lane and its output. The `--no-ui` flag prints it as text.

## The hostname answers over HTTPS

The address works in a browser and in any tool, with a certificate the machine trusts.

```console
$ curl -s https://api.myapp.localhost/
{"lane":0,"lane_name":"main","service":"api","port":20060}
```

The API reports what devlanes told it. This copy of `myapp` is lane 0, the one that belongs to the `main` branch, and it listens on port 20060. The next page adds a second lane.

## The service keeps running after the terminal closes

A background daemon owns the process. You can close the terminal, open another and find the service where you left it.

```console title="~/dev/myapp"
$ devlanes status
myapp
  LANE  BRANCH  STATE    SERVICES  CPU   MEM       URL
  0     main    running  1/1       0.0%  38.0 MiB  https://myapp.localhost

  UNIT  STATE  PORT   CPU   MEM       URL
  api   ready  20060  0.0%  38.0 MiB  https://api.myapp.localhost
```

Starting again while it runs changes nothing, so a start is always safe to repeat.

## A stopped lane keeps its place

Stopping ends the process and nothing else. The lane keeps its port and its hostname for the next start.

```console title="~/dev/myapp"
$ devlanes stop
stopped myapp@0:api
```

## On your own project

- [Add devlanes to a project](../how-to/projects/add-devlanes.md)
- [Run scripts from project files](../how-to/projects/run-scripts.md)
- [Wait until a service is ready](../how-to/services/ready-checks.md)
- [The daemon](../concepts/daemon.md) and [Hostnames and HTTPS](../concepts/hostnames-and-https.md)
