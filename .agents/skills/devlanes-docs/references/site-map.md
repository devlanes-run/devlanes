# Site map

Every page under `docs/`, its title, what it holds, and which page owns each fact. `docs/nav.yaml` is the sidebar; keep the two in step. The title here is the page's `title` and the text of every link to it. A page marked (skill) states facts that `skills/devlanes/` repeats, so a change to one changes the other.

## Start

| Page | Title | Holds |
|---|---|---|
| `README.md` | devlanes | The overview at `/docs/`: what devlanes does in one paragraph, the prompt for an agent, one diagram of a machine with its daemon, proxy and two lanes, and a way in per goal. |
| `install.md` | Install devlanes | Installing per OS, `devlanes setup`, `devlanes doctor`, package managers, shell completions. |

## Learn

Read in order. One project, myapp, from one service to lanes on a box.

| Page | Title | Holds |
|---|---|---|
| `learn/first-service.md` | A service, a port and a hostname | One service in a project file; the lines of the app that read its port; what a start does; the daemon keeping it running; logs. |
| `learn/worktrees.md` | Every worktree gets a lane | A second worktree becomes lane 1 next to lane 0: lane numbers, lane domains, a block of ports each, what a service is told about its lane. |
| `learn/connecting-services.md` | Services that find each other | A web app that calls the API of its own lane: expressions, the two kinds of URL, a preset, a task that installs dependencies. |
| `learn/databases.md` | A database for each lane | One Postgres for every lane with a database per lane: containers, per-lane values, setup, tasks at lane events, cleaning a lane. |
| `learn/failures.md` | When a service fails | A service that fails and a config edit: the failed state, no automatic restart, finding the reason, reload. |
| `learn/coding-agents.md` | A coding agent runs the lane | An agent in a worktree of its own: the skill, a long-running command, tests against a lane, stopping only what it started. |
| `learn/remote.md` | Lanes on another machine | A box, pairing once, connecting, the same hostnames in your browser. Carries the diagram of a box and your machine. |

## How-to guides

### Projects (`how-to/projects/`)

| Page | Title | Goal |
|---|---|---|
| `add-devlanes.md` | Add devlanes to a project | Put a project that already runs some other way on devlanes. |
| `migrate.md` | Migrate from another process manager | Move from mprocs, a Procfile, process-compose, pm2, tmuxinator, `concurrently` or Compose, with an agent and the skill. |
| `run-scripts.md` | Run scripts your project already has | Run scripts from `package.json`, a Makefile, a justfile, a Taskfile, `Cargo.toml`, `pyproject.toml`, `deno.json` or `go.mod`, and Maven goals and Gradle tasks. |
| `runtimes.md` | Run a service in its runtime | What a Node, Bun, Deno, Python, Rust, Go or Java service needs, with tabs per runtime. |
| `vite-and-next.md` | Run Vite and Next.js | The two presets and their gotchas. |
| `spring-boot-and-quarkus.md` | Run Spring Boot and Quarkus | The two presets, a Maven multi-module build, and leaving containers to devlanes. |
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
| `one-off-commands.md` (skill) | Run a command with a lane's environment | A reset, a seed or a script with the lane's variables and without its services: a task run by hand, `exec` with `wait`, or a shell. |
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
| `connect.md` (skill) | Connect to a box | Connect, pair, reconnect without a terminal, revoke a machine, turn every machine away. |
| `connect-by-command.md` | Connect to a box without SSH | A container, pod, WSL distribution or codespace. |
| `tailscale.md` | Connect over Tailscale | A box on a tailnet. |
| `cloudflare.md` | Connect through Cloudflare | A box behind Cloudflare. |
| `several-boxes.md` | Work with several boxes | Boxes that run the same project. |
| `what-a-box-shares.md` | Choose what a box shares | Keep a service on the box; mirror a port devlanes does not run. |

### Other (`how-to/`)

| Page | Title | Goal |
|---|---|---|
| `use-the-browser-extension.md` | Use the browser extension | See lanes and their URLs in the browser, open a page in another lane, switch boxes, fill test logins. |
| `wsl.md` | Run devlanes in WSL | devlanes in a WSL distribution with the browser on Windows. |
| `update.md` | Update and uninstall | New releases, builds of `main`, removing devlanes. |
| `report-a-bug.md` | Report a bug | Collect what a bug report needs. |

## Concepts (`concepts/`)

| Page | Title | Explains |
|---|---|---|
| `lanes-and-projects.md` | Lanes and projects | Worktrees, lanes, lane 0, lane names, what a lane is made of, projects and their names. |
| `daemon.md` | The daemon | What the daemon runs, when it starts and exits, what a reboot leaves, that it does not restart a failed service. |
| `ports.md` | Ports | A block of ports per lane and a place in it per service. |
| `hostnames-and-https.md` | Hostnames and HTTPS | Lane domains, the proxy, certificates, a lane as its own site for cookies. |
| `environment.md` | Environment and expressions | Where a unit's variables come from and how expressions are evaluated. |
| `start-order.md` | Start order and readiness | Containers, setup, tasks, services; what ready means; that services have no dependency graph. |
| `config-and-reload.md` | Config files and reload | How the project file and the local file merge, drift, what a reload does. |
| `freezing.md` | Throttling and freezing | What happens to a lane nobody uses and what thaws it. |
| `remote-mirroring.md` | Remote mirroring | Two daemons and one connection, and why a box keeps its hostnames and ports on your machine. |
| `security.md` | Security | Who can reach the daemon, that a lane is not a sandbox, what the CA can sign, what pairing protects, the release check. |

## Reference (`reference/`)

| Page | Title | Lists |
|---|---|---|
| `cli/<command>.md` | `devlanes <command>` | Generated. One page per command. |
| config (website) | Project file reference, User config reference | Generated by the website from the schemas, at `/docs/reference/config/project/` and `/docs/reference/config/user/`. |
| `expressions.md` | Expressions | Namespaces and functions of `${{ }}`. |
| `environment-variables.md` | Environment variables | Variables devlanes sets for a service, and variables a person sets to change devlanes. |
| `lane-and-unit-references.md` (skill) | Lane and unit references | How to name a lane and a unit on the command line. |
| `states-and-exit-codes.md` (skill) | States and exit codes | Unit and lane states, exit codes, output modes. |
| `tui.md` | The TUI | The views and the keys a person uses. |
| `mcp-tools.md` (skill) | MCP tools | The tools `devlanes mcp` offers and its options. |
| `remote-states.md` | Remote connection states | The states and warnings of `devlanes remote ls` and what to do about each. |
| `files.md` | Files and folders | Where the home, the logs, the CA, the user config and a project's files are. |
| `windows.md` | Windows | What a person on Windows has to know or do differently. |
| `limits.md` | Limits | What devlanes does not do, each with its issue. |
| `glossary.md` | Glossary | The words a person meets in the first week, one sentence each. |
| `errors/` | | Proxy errors DL101 to DL303, one page each. The binary links these paths and a test checks their headings; keep both. |

`troubleshooting.md`, titled Troubleshooting, sits at the top level: the problems people hit, each with its fix or the page that has it.

## Owners of facts that tend to spread

| Fact | Owner | Elsewhere |
|---|---|---|
| Lane numbers, lane names, lane domains | `concepts/lanes-and-projects.md` | One sentence in `learn/worktrees.md` |
| What `devlanes setup` changes | `install.md` | Linked from the hostname guides |
| Trusting the CA in browsers, tools and runtimes, `JAVA_TOOL_OPTIONS` included | `how-to/hostnames/trust-the-ca.md` | Linked from `install.md` |
| The proxy's ports | `how-to/hostnames/proxy-ports.md` | Nowhere else |
| Ready checks | The generated config reference | `how-to/services/ready-checks.md` shows choosing one |
| Start order, setup against tasks | `concepts/start-order.md` | The table on `learn/databases.md` |
| A failed service stays failed | `concepts/daemon.md` | One sentence in `learn/failures.md` |
| Freezing | `concepts/freezing.md` | `how-to/lanes/many-lanes.md` changes the settings |
| What the CA can sign | `concepts/security.md` | One sentence in `concepts/hostnames-and-https.md` |
| What a clean deletes | `how-to/lanes/clean-up.md` | Linked from the infra guides |
| Pairing: the steps | `how-to/remote/connect.md` | `learn/remote.md` shows it once |
| Pairing: what it protects | `concepts/security.md` | Nowhere else |
| Installing devlanes on a box | `how-to/remote/prepare-a-box.md` | Linked from the other remote guides |
| States and exit codes | `reference/states-and-exit-codes.md` | The skill |
| Windows differences | `reference/windows.md` | OS tabs where a command differs |
| Problems and their fixes | `troubleshooting.md` | Nowhere else |
| What devlanes does not do | `reference/limits.md` | Nowhere else |
