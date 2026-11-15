---
title: Glossary
description: The terms the devlanes docs use, each with a short meaning and the page that explains it.
kind: reference
---

The words the devlanes docs use, in alphabetical order, each with the page that explains it.

| Term | Meaning | Explained in |
|---|---|---|
| Agent output | The output mode for coding agents: terse lines, failures first, capped in size, with no prompts. devlanes picks it when it detects an agent. | [States and exit codes](states-and-exit-codes.md#output-modes) |
| Box | A machine whose lanes are mirrored to another machine, such as a VPS, a container or a codespace. | [Remote mirroring](../concepts/remote-mirroring.md) |
| CA | The certificate authority devlanes creates on your machine to sign the certificates of its hostnames. It can only sign names for local development. | [Hostnames and HTTPS](../concepts/hostnames-and-https.md) |
| Container | Something a lane runs under `infra`: a Compose service or an entry of `infra.containers`. | [A database for each lane](../learn/databases.md) |
| Daemon | The background process that owns the lanes, their processes and their containers. The CLI and the TUI are its clients. | [The daemon](../concepts/daemon.md) |
| Drift | The difference between what a lane runs and what its project file now says. `devlanes reload` applies it. | [Config files and reload](../concepts/config-and-reload.md) |
| Exec command | A command started with `devlanes exec --name NAME`, which the daemon owns as a unit of the lane. | [A coding agent runs the lane](../learn/coding-agents.md) |
| Expression | A `${{ ... }}` value in the project file, evaluated for each lane. | [Expressions](expressions.md) |
| External port | An `infra.external` entry: a server devlanes does not start, given a hostname and mirrored to connected machines. | [Give a server you run yourself a hostname](../how-to/services/external.md) |
| Frozen | The state of an idle lane whose processes are paused. They keep their memory, ports and state, and the next request, connection or command resumes them. | [Throttling and freezing](../concepts/freezing.md) |
| Global env | `global_env` in the project file: variables every unit of the lane gets. | [Environment and expressions](../concepts/environment.md) |
| Hostname | A name the proxy serves over HTTPS, such as `api.myapp1.localhost`. | [Hostnames and HTTPS](../concepts/hostnames-and-https.md) |
| Hosts helper | `devlanes-hosts`, a small program that writes the routed hostnames into `/etc/hosts` for programs that cannot resolve `*.localhost`. macOS and Linux only. | [Fix programs that cannot resolve *.localhost](../how-to/hostnames/resolve-localhost.md) |
| Idle lane | A lane with no requests, connections, CPU use or commands for a while. devlanes throttles and then freezes it. | [Throttling and freezing](../concepts/freezing.md) |
| Lane | A running copy of the project for one worktree, with its own ports, hostnames and data. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Lane 0 | The lane of the primary worktree. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Lane data folder | A folder of each lane outside its worktree, for files no other lane may share. | [Files and folders](files.md#lane-data-folder) |
| Lane domain | The domain the hostnames of a lane end in: `myapp.localhost` in lane 0, `myapp1.localhost` in lane 1. | [Hostnames and HTTPS](../concepts/hostnames-and-https.md) |
| Lane name | `main` for lane 0 and `lane<N>` for lane N, or the name given to a lane added with `devlanes lane new --name`. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Lane number | A lane's index: `0` for the primary worktree, then `1`, `2` and so on. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Lane reference | How a command names a lane, such as `1`, `feat/checkout` or `myapp@1`. | [Lane and unit references](lane-and-unit-references.md) |
| Lane vars | `lanes.vars` in the project file: values computed once per lane and read as `lane.<name>`. | [Expressions](expressions.md#lane) |
| Local file | `devlanes.local.yaml`: one developer's changes to the project file, kept out of Git. | [Keep personal changes out of Git](../how-to/projects/local-overrides.md) |
| MCP server | `devlanes mcp`, which offers devlanes commands as tools to agents that use MCP. | [MCP tools](mcp-tools.md) |
| Mine | The units the caller started, such as one agent session. `--mine` selects them. | [A coding agent runs the lane](../learn/coding-agents.md) |
| Named port | A second port of a service, read as `self.ports.<name>`. | [Ports](../concepts/ports.md) |
| Orphaned unit | A process an earlier daemon started that still runs, and that nothing supervises. | [States and exit codes](states-and-exit-codes.md#unit-states) |
| Output mode | How a command prints: `human`, `plain`, `agent` or `json`. | [States and exit codes](states-and-exit-codes.md#output-modes) |
| Pairing | Proving to a box, with a one-time code from `devlanes daemon pair`, that your machine may connect to it. | [Connect to a box](../how-to/remote/connect.md) |
| Per-lane container | A container each lane runs for itself, rather than one container all lanes share. | [Give each lane its own container](../how-to/infra/container-per-lane.md) |
| Platform | The runtime of a unit, such as `node` or `python`, which decides the variables devlanes adds for it. | [Run Node, Bun, Deno, Python and Rust services](../how-to/projects/runtimes.md) |
| Port block | The range of consecutive ports each lane gets from the port pool. | [Ports](../concepts/ports.md) |
| Port pool | The range all port blocks of the machine come from, `ports.range` in the user config. | [Ports](../concepts/ports.md) |
| Preset | A service setting, such as `preset: vite`, that passes a framework its port and other per-lane settings. | [Services that find each other](../learn/connecting-services.md) |
| Primary worktree | The worktree that has the project's primary branch, such as `main`, checked out. Its lane is lane 0. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Profile | A named set of services, and their settings, that a start can select. | [Share settings and choose which services start](../how-to/projects/templates-and-profiles.md) |
| Project | What one project file describes, named by its `project` key. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Project file | `devlanes.yaml`, or another file named with `--file`. | [A service, a port and a hostname](../learn/first-service.md) |
| Proxy | The part of the daemon that serves every hostname over HTTPS and passes each request to its lane. | [Hostnames and HTTPS](../concepts/hostnames-and-https.md) |
| Ready check | `ready`: how devlanes decides that a service works, such as a port that accepts connections. | [Wait until a service is ready](../how-to/services/ready-checks.md) |
| Reload | `devlanes reload`: applies changes to the project file to the running lane. | [Config files and reload](../concepts/config-and-reload.md) |
| Remote mirroring | Your machine binding the ports and serving the hostnames of a box's lanes, so they open as if they ran locally. | [Remote mirroring](../concepts/remote-mirroring.md) |
| Replica | One of several copies of a service, set with `replicas`, named `worker.1`, `worker.2` and so on. | [Run workers and replicas](../how-to/services/workers.md) |
| Run script | `run_script`: a service or task that runs a script from a project file such as `package.json`. | [Run scripts from project files](../how-to/projects/run-scripts.md) |
| Service | A long-running process of a lane, under `services` in the project file. | [A service, a port and a hostname](../learn/first-service.md) |
| Setup entry | An `infra.setup` entry: a command run after a lane's containers are up, such as creating its database. `infra.teardown` entries run when it is cleaned. | [A database for each lane](../learn/databases.md) |
| Skill | The instructions coding agents load to use devlanes, installed with `npx skills add`. | [Set up a coding agent](../how-to/agents/set-up.md) |
| Start gate | The machine-wide limit on how many services start at once. A service waiting for it is `queued`. | [Start order and readiness](../concepts/start-order.md) |
| Sync | `sync`: copying a folder a service builds to a fixed path, on this machine or from a box. | [Copy build output to a fixed folder](../how-to/services/sync-build-output.md) |
| Tag | A label in a service's `tags`, which commands select with `+tag`. | [Share settings and choose which services start](../how-to/projects/templates-and-profiles.md) |
| Task | A one-off command of the lane, under `tasks`, such as an install or a migration. | [Run installs, migrations and seeds](../how-to/services/tasks.md) |
| Template | A set of service settings that services take with `extends`. | [Share settings and choose which services start](../how-to/projects/templates-and-profiles.md) |
| Throttled | The state of an idle lane whose processes run at background priority and still serve. On macOS and Windows only. | [Throttling and freezing](../concepts/freezing.md) |
| TUI | The terminal interface that `devlanes` opens on a terminal. Closing it stops nothing. | [The TUI](tui.md) |
| Unit | Something a lane runs or routes: a service, task, exec command, setup or teardown entry, or external port. | [Lane and unit references](lane-and-unit-references.md#unit-references) |
| Unit reference | How a command names a unit, such as `api`, `1:api` or `myapp@1:api`. | [Lane and unit references](lane-and-unit-references.md#unit-references) |
| User config | `~/.devlanes/config.yaml`: your settings for every project on the machine. | [Files and folders](files.md#the-devlanes-home) |
| Worktree | A Git worktree: a folder with one branch of the repository checked out. Each worktree of a project runs as a lane. | [Every worktree gets a lane](../learn/worktrees.md) |
| Your machine | The machine where the browser runs, when a box is involved. | [Remote mirroring](../concepts/remote-mirroring.md) |
