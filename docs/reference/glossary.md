---
title: Glossary
description: The words you meet in your first week with devlanes, each with a short meaning and the page that explains it.
kind: reference
---

The words the devlanes docs use most, in alphabetical order, each with the page that explains it.

| Term | Meaning | Explained in |
|---|---|---|
| Box | A machine whose lanes show up on another. | [Remote mirroring](../concepts/remote-mirroring.md) |
| CA | The certificate authority devlanes creates on your machine to sign the certificates of its hostnames. | [Hostnames and HTTPS](../concepts/hostnames-and-https.md) |
| Container | Something under `infra`, such as a Compose service. | [A database for each lane](../learn/databases.md) |
| Daemon | The background process that runs the lanes. | [The daemon](../concepts/daemon.md) |
| Data folder | A folder of each lane outside its worktree, read as `${{ lane.data_path }}`. | [Files and folders](files.md#lane-data-folder) |
| Drift | A difference between what a lane runs and what its project file now says, which `devlanes reload` applies. | [Config files and reload](../concepts/config-and-reload.md) |
| Expression | A `${{ ... }}` value in the project file, evaluated for each lane. | [Expressions](expressions.md) |
| Frozen, thaw | A lane nobody uses is suspended, and runs again on its next request, connection or command. | [Throttling and freezing](../concepts/freezing.md) |
| Hostname | A name the proxy serves over HTTPS, such as `api.myapp1.localhost`. | [Hostnames and HTTPS](../concepts/hostnames-and-https.md) |
| Lane | One worktree's place to run the project, with ports, hostnames and data of its own. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Lane 0 | The lane of the primary worktree, the folder you cloned into, whatever branch it has checked out. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Lane domain | The domain a lane's hostnames end in: `myapp.localhost` in lane 0, `myapp1.localhost` in lane 1. | [Hostnames and HTTPS](../concepts/hostnames-and-https.md) |
| Lane name | `main` for lane 0 and `lane<N>` for lane N. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Local file | `devlanes.local.yaml`: one developer's changes to the project file, kept out of Git. | [Keep personal changes out of Git](../how-to/projects/local-overrides.md) |
| Pairing | Proving to a box, with a one-time code from `devlanes daemon pair`, that your machine may connect to it. | [Security](../concepts/security.md) |
| Port block | The range of consecutive ports each lane gets. | [Ports](../concepts/ports.md) |
| Preset | A service setting, such as `preset: vite`, that passes a framework its port and other per-lane settings. | [Services that find each other](../learn/connecting-services.md) |
| Profile | A named set of services, and their settings, that a start can select. | [Share settings and choose which services start](../how-to/projects/templates-and-profiles.md) |
| Project | What one `devlanes.yaml` describes. | [Lanes and projects](../concepts/lanes-and-projects.md) |
| Project file | `devlanes.yaml`. | [A service, a port and a hostname](../learn/first-service.md) |
| Proxy | The part of the daemon that serves every hostname over HTTPS and passes each request to its lane. | [Hostnames and HTTPS](../concepts/hostnames-and-https.md) |
| Ready check | `ready`: how devlanes decides that a service works. | [Wait until a service is ready](../how-to/services/ready-checks.md) |
| Reload | `devlanes reload`, which applies changes to the project file to the running lane. | [Config files and reload](../concepts/config-and-reload.md) |
| Service | A long-running process of a lane. | [A service, a port and a hostname](../learn/first-service.md) |
| Setup | `infra.setup` entries: commands run once per lane after its containers are ready, such as creating its database. | [A database for each lane](../learn/databases.md) |
| Shared container | A container every lane of the project uses, such as one Postgres with a database per lane. | [Share one database server between lanes](../how-to/infra/shared-database.md) |
| Task | A one-off command of a lane, such as an install or a migration. | [Run installs, migrations and seeds](../how-to/services/tasks.md) |
| Unit | A service, task or command that a lane runs. | [Lane and unit references](lane-and-unit-references.md#unit-references) |
| User config | `~/.devlanes/config.yaml`. | [Files and folders](files.md#the-devlanes-home) |
| Worktree | A Git worktree. | [Every worktree gets a lane](../learn/worktrees.md) |
