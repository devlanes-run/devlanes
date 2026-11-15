# Site map

Every page under `docs/`, what it holds, and which page owns each fact. `docs/nav.yaml` is the sidebar; keep the two in step. A page marked (skill) states facts that `skills/devlanes/` repeats, so a change to one changes the other.

## Start

| Page | Holds |
|---|---|
| `README.md` | The overview at `/docs/`: what devlanes does in one paragraph, one status table, one diagram naming project, lane, unit, daemon, proxy and box, and a way in per goal (a hostname and HTTPS for one app, every worktree at once, an agent running the stack, lanes on another machine, coming from another process manager). |
| `install.md` | Installing per OS and install method, shell completions, `devlanes setup`, a first `devlanes doctor`. |

## Learn

Read in order. One project, `myapp`, from one service to lanes on a box.

| Page | Holds |
|---|---|
| `learn/first-service.md` | One service in a project file; what a start does; the port, the hostname and HTTPS it gets; the daemon keeping it running; the TUI and the status table. |
| `learn/worktrees.md` | A second worktree becomes lane 1 next to lane 0: lane numbers, lane domains, a block of ports each. |
| `learn/connecting-services.md` | A web app that calls the API of its own lane: expressions, the two kinds of URL, presets. |
| `learn/databases.md` | One Postgres for every lane with a database per lane: containers, per-lane values, setup, tasks at lane events, cleaning a lane. |
| `learn/failures.md` | A service that fails and a config edit: the failed state, no automatic restart, finding the reason, reload. |
| `learn/agents.md` | A coding agent running the lane: the daemon owns the processes, the skill, ad-hoc commands, tests against a lane, stopping what the agent started. |
| `learn/remote.md` | The lanes on another machine: a box, pairing, the same hostnames and ports on your machine. |

## How-to guides

### Projects (`how-to/projects/`)

| Page | Goal |
|---|---|
| `add-devlanes.md` | Add devlanes to a project that already runs some other way. |
| `migrate.md` | Move from mprocs, a Procfile, process-compose, pm2, tmuxinator, `concurrently` or Compose, with an agent and the skill. |
| `run-scripts.md` | Run scripts from `package.json`, a Makefile, a justfile, `Cargo.toml`, `pyproject.toml` or `deno.json`. |
| `runtimes.md` | Run Node, Bun, Deno, Python and Rust services, with tabs per runtime. |
| `vite-and-next.md` | Run Vite and Next.js dev servers. |
| `cloudflare-workers.md` | Run Workers with Wrangler or the Cloudflare Vite plugin. |
| `tauri.md` | Run a Tauri app per lane. |
| `monorepo.md` | Run a monorepo as one project or as several. |
| `dev-and-prod.md` | Run dev servers and a production build from one folder. |
| `local-overrides.md` | Keep changes for one machine out of Git. |
| `templates-and-profiles.md` | Share settings between services and pick which ones start. |
| `env-files-and-tools.md` | Read values from `.env` and pin tool versions with mise or direnv. |

### Services (`how-to/services/`)

| Page | Goal |
|---|---|
| `ready-checks.md` | Make a start wait until a service works. |
| `workers.md` | Run workers, queue consumers and replicas. |
| `ports-and-hostnames.md` | Give one service several ports and hostnames. |
| `on-demand.md` | Keep a service stopped until someone asks for it. |
| `tasks.md` | Run installs, migrations and seeds at the right time. |
| `external.md` | Give a server devlanes does not run a hostname and HTTPS. |
| `sync-build-output.md` | Copy a folder a service builds to a fixed path, on this machine or from a box. |
| `gui-tools.md` | Open a GUI tool that serves HTTP or TCP, such as a simulator or a remote desktop, as a service. |

### Hostnames and HTTPS (`how-to/hostnames/`)

| Page | Goal |
|---|---|
| `trust-the-ca.md` | Make browsers, command-line tools and runtimes accept the certificates, per OS. |
| `wildcards.md` | Serve wildcard and tenant subdomains, and give a name its own cookie site. |
| `cors-and-cookies.md` | Set CORS, CSRF and cookies so each lane stays apart. |
| `resolve-localhost.md` | Fix programs that cannot resolve `*.localhost`. |
| `proxy-ports.md` | Use ports 443 and 80, run next to another reverse proxy, open lanes from another device. |

### Databases and containers (`how-to/infra/`)

| Page | Goal |
|---|---|
| `shared-database.md` | Share one database server with a database per lane. |
| `container-per-lane.md` | Give each lane its own container. |
| `datastores.md` | Keep Redis, RabbitMQ, object storage and mail apart per lane. |
| `plain-containers.md` | Run containers without a Compose file. |

### Lanes (`how-to/lanes/`)

| Page | Goal |
|---|---|
| `two-lanes-one-folder.md` | Run a folder twice. |
| `attach-to-another-lane.md` | Point one lane's frontend at another lane's backend. |
| `after-a-reboot.md` | Bring lanes back after a reboot or a daemon restart. |
| `clean-up.md` | Remove lanes: deleted worktrees, a renamed project, a reset. |
| `many-lanes.md` | Run many lanes on one machine: freezing, keeping a lane awake, pacing starts, disk use. |

### Coding agents (`how-to/agents/`)

| Page | Goal |
|---|---|
| `set-up.md` (skill) | Give an agent the skill and, where needed, the MCP server, with tabs per agent host. |
| `sandboxes.md` (skill) | Use devlanes from an agent that runs in a sandbox. |

### Tests and CI (`how-to/ci/`)

| Page | Goal |
|---|---|
| `tests.md` (skill) | Run tests against a lane. |
| `ci.md` | Run them in GitHub Actions and other CI systems, and keep the logs. |

### Remote boxes (`how-to/remote/`)

| Page | Goal |
|---|---|
| `prepare-a-box.md` (skill) | Get a machine ready to be a box. |
| `connect.md` (skill) | Connect, pair, reconnect without a terminal, revoke a machine. |
| `connect-by-command.md` | Reach a container, pod, WSL distribution or codespace without SSH. |
| `tailscale.md` | Connect over a tailnet. |
| `cloudflare.md` | Connect through Cloudflare. |
| `several-boxes.md` | Work with boxes that run the same project. |
| `what-a-box-shares.md` | Keep a service on the box, and mirror a port devlanes does not run. |

### Other (`how-to/`)

| Page | Goal |
|---|---|
| `wsl.md` | Run devlanes in WSL with the browser on Windows. |
| `update.md` | Update, run builds of `main`, uninstall. |
| `report-a-bug.md` | Collect what a bug report needs. |

## Concepts (`concepts/`)

| Page | Explains |
|---|---|
| `lanes-and-projects.md` | Worktrees, lanes, lane 0, projects and project names, several projects in one repository. |
| `daemon.md` | The daemon and its clients, what it owns, when it exits, what it never does (restart a process, cap resources). |
| `ports.md` | The port pool, a block per lane, offsets per service, named ports. |
| `hostnames-and-https.md` | Lane domains, the proxy, the CA, how names resolve, a lane as its own site for cookies. |
| `environment.md` | Where a unit's variables come from, in order, and how expressions are evaluated. |
| `start-order.md` | Containers, setup, tasks, services; readiness; pacing of starts; the reason there is no dependency graph. |
| `config-and-reload.md` | How the project file, local files and `extends` merge; drift; what reload does. |
| `idle-lanes.md` | Throttling and freezing: what counts as activity and what a frozen lane keeps. |
| `remote-mirroring.md` | Two daemons and one session: what crosses it, why a box keeps its names and ports, one box per lane. |
| `security.md` | Who can reach the daemon, what the CA can sign, what pairing protects, what lanes do not isolate. |

## Reference (`reference/`)

| Page | Lists |
|---|---|
| `cli/<command>.md` | Generated. One page per command. |
| config (website) | Generated by the website from the schemas, at `/docs/reference/config/`. |
| `expressions.md` | Namespaces, functions and filters of `${{ }}`. |
| `environment-variables.md` | Variables devlanes sets for units, and variables devlanes reads. |
| `lane-and-unit-references.md` (skill) | The forms of a lane reference and a unit reference, globs and tags. |
| `states-and-exit-codes.md` (skill) | Unit and lane states, exit codes, the shape of `--json` output, output modes. |
| `tui.md` | Views and keys of the TUI. |
| `mcp-tools.md` (skill) | The tools `devlanes mcp` offers, per-host timeouts, consent. |
| `remote-states.md` | Connection states and warnings of `remote ls`. |
| `files.md` | The devlanes home, the lane data folder, logs, the CA. |
| `windows.md` | Behaviour that differs on Windows: shells, stop signals, ports, terminals. |
| `limits.md` | What devlanes does not do, each with its issue. |
| `glossary.md` | The terms, one or two sentences each. |
| `errors/` | Proxy errors DL101 to DL303, one page each. The binary links these paths; keep them. |

`troubleshooting.md` sits at the top level: symptoms and messages, each with the page that fixes it.

## Owners of facts that tend to spread

| Fact | Owner | Elsewhere |
|---|---|---|
| The form of a lane domain | `concepts/hostnames-and-https.md` | One sentence in `learn/worktrees.md`; the glossary |
| Order of environment layers | `concepts/environment.md` | Nowhere else |
| Variables devlanes sets | `reference/environment-variables.md` | A how-to guide names the one it uses |
| Trusting the CA on each OS | `how-to/hostnames/trust-the-ca.md` | `install.md` runs `devlanes setup` and links it |
| Ready check kinds and fields | The generated config reference | `how-to/services/ready-checks.md` shows choosing one |
| Start order | `concepts/start-order.md` | One sentence in `learn/databases.md` |
| Freezing timings and settings | The generated config reference | `concepts/idle-lanes.md` explains; `how-to/lanes/many-lanes.md` changes them |
| Pairing: the steps | `how-to/remote/connect.md` | `learn/remote.md` shows it once |
| Pairing: code lifetime, limits, what it protects | `concepts/security.md` | Nowhere else |
| What crosses a remote connection | `concepts/remote-mirroring.md` | `concepts/security.md` links it |
| Connection states and warnings | `reference/remote-states.md` | `troubleshooting.md` links rows to it |
| Exit codes and unit states | `reference/states-and-exit-codes.md` | The skill |
| TUI keys | `reference/tui.md` | A page names a key only when its section is about that key |
| Windows differences | `reference/windows.md` | OS tabs where a command differs |
| Error messages and their fixes | `troubleshooting.md` | Nowhere else |
| What devlanes does not do | `reference/limits.md` | Nowhere else |
