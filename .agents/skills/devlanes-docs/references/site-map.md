# Site map

Every page under `docs/`, its title, what it holds, and which page owns each fact. `docs/nav.yaml` is the sidebar; keep the two in step. The title here is the page's `title` and the text of every link to it. A page marked (skill) states facts that `skills/devlanes/` repeats, so a change to one changes the other.

## Start

| Page | Title | Holds |
|---|---|---|
| `README.md` | devlanes | The overview at `/docs/`: what devlanes does in one paragraph, the prompt for an agent, one `devlanes status` block, one diagram of a machine with its daemon, proxy and two lanes, and a way in per goal. |
| `install.md` | Install devlanes | Installing per OS and install method, what `devlanes setup` changes and how to undo it, a first `devlanes doctor`, shell completions. |

## Learn

Read in order. One project, myapp, from one service to lanes on a box.

| Page | Title | Holds |
|---|---|---|
| `learn/first-service.md` | A service, a port and a hostname | One service in a project file; the lines of the app that read its port; what a start does; the daemon keeping it running; logs. |
| `learn/worktrees.md` | Every worktree gets a lane | A second worktree becomes lane 1 next to lane 0: lane numbers, lane domains, a block of ports each, what a service is told about its lane. |
| `learn/connecting-services.md` | Services that find each other | A web app that calls the API of its own lane: expressions, the two kinds of URL, presets, a task that installs dependencies in a new lane. |
| `learn/databases.md` | A database for each lane | One Postgres for every lane with a database per lane: containers, per-lane values, setup, tasks at lane events, cleaning a lane. |
| `learn/failures.md` | When a service fails | A service that fails and a config edit: the failed state, no automatic restart, finding the reason, reload. |
| `learn/coding-agents.md` | A coding agent runs the lane | The daemon owns the processes, the skill, ad-hoc commands, tests against a lane, stopping what the agent started. |
| `learn/remote.md` | Lanes on another machine | A box, pairing, the same hostnames and ports on your machine. Carries the diagram of a box and your machine. |

## How-to guides

### Projects (`how-to/projects/`)

| Page | Title | Goal |
|---|---|---|
| `add-devlanes.md` | Add devlanes to a project | Put a project that already runs some other way on devlanes. |
| `migrate.md` | Migrate from another process manager | Move from mprocs, a Procfile, process-compose, pm2, tmuxinator, `concurrently` or Compose, with an agent and the skill. |
| `run-scripts.md` | Run scripts from project files | Run scripts from `package.json`, a Makefile, a justfile, `Cargo.toml`, `pyproject.toml` or `deno.json`. |
| `runtimes.md` | Run Node, Bun, Deno, Python and Rust services | What each runtime needs, with tabs per runtime. |
| `vite-and-next.md` | Run Vite and Next.js | The two presets and their gotchas. |
| `cloudflare-workers.md` | Run Cloudflare Workers | Wrangler and the Cloudflare Vite plugin per lane. |
| `tauri.md` | Run a Tauri app | A desktop app per lane and replica. |
| `monorepo.md` | Split a monorepo into projects | One project for the repository, or one per app. |
| `dev-and-prod.md` | Run dev servers and a production build side by side | A second project file next to the first. |
| `local-overrides.md` | Keep personal changes out of Git | The local file. |
| `templates-and-profiles.md` | Share settings and choose which services start | Templates, tags and profiles. |
| `env-files-and-tools.md` | Read .env values and pin tool versions | `.env`, mise and direnv. |

### Services (`how-to/services/`)

| Page | Title | Goal |
|---|---|---|
| `ready-checks.md` | Wait until a service is ready | Make a start wait until a service works. |
| `workers.md` | Run workers and replicas | Services without a port, queue consumers, several copies of one service. |
| `ports-and-hostnames.md` | Give a service several ports and hostnames | Named ports and extra hostnames. |
| `on-demand.md` | Start a service only on demand | Keep a service stopped until someone asks for it. |
| `tasks.md` | Run installs, migrations and seeds | One-off commands at the right lane event. |
| `external.md` | Give a server you run yourself a hostname | A hostname and HTTPS for a server devlanes does not start. |
| `sync-build-output.md` | Copy build output to a fixed folder | A folder a service builds, copied on this machine or from a box. |
| `gui-tools.md` | Open a GUI tool as a service | A simulator or a remote desktop that serves HTTP or TCP. |

### Hostnames and HTTPS (`how-to/hostnames/`)

| Page | Title | Goal |
|---|---|---|
| `trust-the-ca.md` | Trust the devlanes CA | Make browsers, command-line tools and runtimes accept the certificates, per OS. |
| `wildcards.md` | Serve wildcard and tenant subdomains | Wildcards, a fallback, and a hostname with its own cookie site. |
| `cors-and-cookies.md` | Set CORS and cookies for each lane | Keep each lane's origins and cookies apart. |
| `resolve-localhost.md` | Fix programs that cannot resolve *.localhost | The hosts file and the resolver. |
| `proxy-ports.md` | Choose the proxy's ports | Ports 443 and 80, another reverse proxy, access from another device. |

### Databases and containers (`how-to/infra/`)

| Page | Title | Goal |
|---|---|---|
| `shared-database.md` | Share one database server between lanes | One server, a database per lane. |
| `container-per-lane.md` | Give each lane its own container | A container that is not shared. |
| `datastores.md` | Keep Redis, RabbitMQ, S3 and mail apart per lane | A namespace per lane in shared stores. |
| `plain-containers.md` | Run containers without Compose | Containers declared in the project file. |

### Lanes (`how-to/lanes/`)

| Page | Title | Goal |
|---|---|---|
| `two-lanes-one-folder.md` | Run a folder as two lanes | A second lane without a second worktree. |
| `attach-to-another-lane.md` | Point a frontend at another lane's backend | One lane's services against another lane's. |
| `after-a-reboot.md` | Bring lanes back after a reboot | After a reboot or a daemon restart. |
| `clean-up.md` | Remove lanes | Deleted worktrees, a renamed project, a reset. |
| `many-lanes.md` | Run many lanes on one machine | Freezing, keeping a lane awake, pacing starts, disk use. |

### Coding agents (`how-to/agents/`)

| Page | Title | Goal |
|---|---|---|
| `set-up.md` (skill) | Set up a coding agent | The skill and, where needed, the MCP server, with tabs per agent host. |
| `sandboxes.md` (skill) | Use devlanes from an agent sandbox | An agent whose shell cannot reach the daemon. |

### Tests and CI (`how-to/ci/`)

| Page | Title | Goal |
|---|---|---|
| `tests.md` (skill) | Run tests against a lane | Start, test, stop, with the lane's URLs in the environment. |
| `ci.md` | Run tests in CI | GitHub Actions and other CI systems, and keeping the logs. |

### Remote boxes (`how-to/remote/`)

| Page | Title | Goal |
|---|---|---|
| `prepare-a-box.md` (skill) | Prepare a box | Get a machine ready to be a box. |
| `connect.md` (skill) | Connect to a box | Connect, pair, reconnect without a terminal, revoke a machine. |
| `connect-by-command.md` | Connect to a box without SSH | A container, pod, WSL distribution or codespace. |
| `tailscale.md` | Connect over Tailscale | A box on a tailnet. |
| `cloudflare.md` | Connect through Cloudflare | A box behind Cloudflare. |
| `several-boxes.md` | Work with several boxes | Boxes that run the same project. |
| `what-a-box-shares.md` | Choose what a box shares | Keep a service on the box; mirror a port devlanes does not run. |

### Other (`how-to/`)

| Page | Title | Goal |
|---|---|---|
| `wsl.md` | Run devlanes in WSL | devlanes in a WSL distribution with the browser on Windows. |
| `update.md` | Update and uninstall | New releases, builds of `main`, removing devlanes. |
| `report-a-bug.md` | Report a bug | Collect what a bug report needs. |

## Concepts (`concepts/`)

| Page | Title | Explains |
|---|---|---|
| `lanes-and-projects.md` | Lanes and projects | First of the concepts, so it names what a lane is made of: units (services, tasks, containers), setup, profiles, the data folder. Worktrees, lanes, lane 0 and when it moves, lane names, what a deleted worktree leaves behind, projects and project names, two clones of one repository, several projects in one repository. |
| `daemon.md` | The daemon | The daemon and its clients, what it owns, when it starts and exits, sleep, reboot and daemon restart, that it does not restart a failed service or cap resources. Carries a diagram of the clients, the daemon and what it owns. |
| `ports.md` | Ports | The port pool, a block per lane, a place per service, named ports, that ports differ between machines. |
| `hostnames-and-https.md` | Hostnames and HTTPS | Lane domains, the proxy, certificates signed per hostname, how hostnames resolve, a lane as its own site for cookies. |
| `environment.md` | Environment and expressions | Where a unit's variables come from, in order, that the shell's environment is never written to disk and what that protects, how expressions are evaluated. |
| `start-order.md` | Start order and readiness | Containers, setup, tasks, services; setup against tasks; readiness; pacing of starts; that services have no dependency graph and what that asks of a service. |
| `config-and-reload.md` | Config files and reload | How the project file, local files and `extends` merge; drift; what reload does. |
| `freezing.md` | Throttling and freezing | Throttling and freezing: what counts as activity, what a frozen lane keeps, what thaws it. |
| `remote-mirroring.md` | Remote mirroring | Two daemons and one session: what crosses it, why a box keeps its hostnames and ports, one box per lane. |
| `security.md` | Security | Who can reach the daemon, what the CA can sign and where its key is, what pairing protects, what lanes do not isolate, what devlanes sends over the network. Carries a diagram of who is checked where. |

## Reference (`reference/`)

| Page | Title | Lists |
|---|---|---|
| `cli/<command>.md` | `devlanes <command>` | Generated. One page per command. |
| config (website) | | Generated by the website from the schemas, at `/docs/reference/config/`. |
| `expressions.md` | Expressions | Namespaces, functions and filters of `${{ }}`. |
| `environment-variables.md` | Environment variables | Variables devlanes sets for units, and variables devlanes reads. |
| `lane-and-unit-references.md` (skill) | Lane and unit references | The forms of a lane reference and a unit reference, globs and tags. |
| `states-and-exit-codes.md` (skill) | States and exit codes | Unit and lane states, exit codes, the shape of `--json` output, output modes. |
| `tui.md` | The TUI | Views and keys of the TUI. |
| `mcp-tools.md` (skill) | MCP tools | The tools `devlanes mcp` offers, per-host timeouts, consent. |
| `remote-states.md` | Remote connection states | Connection states and warnings of `remote ls`, the timings of pings, reconnects and waits, the pairing rules in numbers, what a box and a connected machine send each other. |
| `files.md` | Files and folders | The devlanes home, the lane data folder, logs, the CA files, the socket or pipe, files outside the home, the files of a project. |
| `windows.md` | Windows | Behaviour that differs on Windows: shells, stop signals, ports, terminals. |
| `limits.md` | Limits | What devlanes does not do, each with its issue. |
| `glossary.md` | Glossary | The terms, one or two sentences each. |
| `errors/` | | Proxy errors DL101 to DL303, one page each. The binary links these paths; keep them. |

`troubleshooting.md`, titled Troubleshooting, sits at the top level: symptoms and messages, each with the page that fixes it.

## Owners of facts that tend to spread

| Fact | Owner | Elsewhere |
|---|---|---|
| The form of a lane domain | `concepts/hostnames-and-https.md` | One sentence in `learn/worktrees.md`; the glossary |
| Lane names (`main`, `lane1`) | `concepts/lanes-and-projects.md` | One sentence in `learn/worktrees.md` |
| Order of environment layers | `concepts/environment.md` | Nowhere else |
| Variables devlanes sets | `reference/environment-variables.md` | A page names the ones it shows |
| What `devlanes setup` changes | `install.md` | `how-to/hostnames/trust-the-ca.md` covers each store and OS |
| Trusting the CA in browsers, tools and runtimes | `how-to/hostnames/trust-the-ca.md` | `install.md` links it |
| Proxy ports and the fallback to 8443 and 8080 | `how-to/hostnames/proxy-ports.md` | One sentence in `install.md` |
| Ready check kinds and fields | The generated config reference | `how-to/services/ready-checks.md` shows choosing one |
| Start order | `concepts/start-order.md` | One sentence in `learn/databases.md` |
| Freezing: what it keeps, what thaws a lane | `concepts/freezing.md` | One sentence in `learn/worktrees.md`; `how-to/lanes/many-lanes.md` changes the settings |
| Freezing: the two default times | `concepts/freezing.md`, once | The generated config reference has the settings |
| Freezing: the CPU threshold, how fast activity is noticed, how long a request waits for a thaw | `reference/states-and-exit-codes.md`, under the throttled and frozen states | Not on a prose page |
| A failed service stays failed | `concepts/daemon.md` | One sentence in `concepts/start-order.md` and `learn/failures.md` |
| The shell's environment is never written to disk | `concepts/environment.md` | One sentence in `concepts/daemon.md` |
| What the CA can sign, where its key is | `concepts/security.md` | One sentence in `concepts/hostnames-and-https.md` |
| A lane as its own site for cookies | `concepts/hostnames-and-https.md`, once | One sentence in `learn/worktrees.md` |
| Variables a new daemon keeps from the shell that started it | `reference/environment-variables.md` | One sentence in `concepts/environment.md` |
| The daemon and terminals that end their child processes | `reference/windows.md` | One sentence in `concepts/daemon.md`; a row in `troubleshooting.md` |
| Pairing: the steps | `how-to/remote/connect.md` | `learn/remote.md` shows it once |
| Pairing: what it protects and what it does not | `concepts/security.md` | Nowhere else |
| Pairing: code length and lifetime, wrong-code limits, pairings per machine | `reference/remote-states.md` | `concepts/security.md` gives the behaviour without the numbers |
| What crosses a remote connection | `concepts/remote-mirroring.md` | `concepts/security.md` links it; the full list in `reference/remote-states.md` |
| Reconnect, ping and wait times of a box | `reference/remote-states.md` | `concepts/remote-mirroring.md` gives the behaviour without the numbers |
| Connection states and warnings | `reference/remote-states.md` | `troubleshooting.md` links rows to it |
| Exit codes and unit states | `reference/states-and-exit-codes.md` | The skill |
| TUI keys | `reference/tui.md` | A page names a key only when its section is about that key |
| Windows differences | `reference/windows.md` | OS tabs where a command differs |
| Error messages and their fixes | `troubleshooting.md` | Nowhere else |
| What devlanes does not do | `reference/limits.md` | Nowhere else |
