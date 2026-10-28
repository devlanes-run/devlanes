# Ports and environment

## Ports

Each lane of each project gets a block of consecutive ports from one machine-wide pool (`ports.range` in `~/.devlanes/config.yaml`, default `20000-32767`). Blocks of different lanes and projects never overlap, so two projects and five worktrees run at once without a port plan. Inside the block every service has a fixed offset, shared by all lanes of the project: if `api` is `lane.port + 3` in lane 0 it is `lane.port + 3` in lane 4 too. Offsets persist in the daemon's database and survive restarts, config edits and branch switches; `devlanes lane gc` drops offsets of services that no longer exist.

The block size is what the project needs plus half again, rounded up to ten, so a project with 7 ports gets 20. `lanes.block_size` fixes it (a config error if too small). A running lane keeps its block when the config grows; when the new ports do not fit, the next start says `the edited config needs ports the running lane's block cannot hold; stop the lane, then start it again`.

Ports that browsers refuse to connect to (6000, 6665-6669, 10080 and the rest of Chrome's list) are skipped.

### The service's port

```yaml
services:
  api:
    run: node server.mjs          # reads process.env.PORT
  docs:
    run: rspress dev --port ${{ self.port }} --host 127.0.0.1
  web:
    preset: vite                  # adds --port for you
    run_script:
      file: web/package.json
      name: dev
```

The service gets `PORT` and `HOST=127.0.0.1` in its environment and must listen there. There is no port detection: devlanes dials `127.0.0.1:<port>` for `ready: tcp`, for the proxy and for `services.<name>.url`. A service that listens somewhere else fails readiness after 60 s with the ports its process tree did bind, so the message tells you what it chose instead.

Three ways to override the port:

```yaml
env:
  PORT: "${{ lane.port + 5 }}"   # a fixed offset in the lane's block; the offset is reserved
env:
  PORT: 3998                     # the same literal port in every lane; only one lane can run it
```

Never write `env: { PORT: "${{ self.port }}" }` (`self.port` is computed from `PORT`; it is rejected as a cycle) and never put `PORT` in `global_env`, which would give every service the same port.

A literal `PORT` is for stubs that a client has hardcoded. Starting it in a second lane fails with `port 3998 is taken by acme@0:sms-stub`.

### Several ports for one service

Mention `self.ports.<name>` anywhere in the service and a second port named `<name>` is allocated:

```yaml
services:
  api:
    run: node --inspect=127.0.0.1:${{ self.ports.inspect }} server.mjs
    env:
      METRICS_PORT: "${{ self.ports.metrics }}"
    hosts:
      - api.${{ lane.domain }}
      - host: "metrics.${{ lane.domain }}"
        port: metrics          # route a hostname to a named port
    ready:
      tcp: metrics             # wait on a named port
```

`hosts[].port` and `ready.tcp` also allocate the name they mention, and the wrangler preset allocates `inspector`. Another service reads them as `${{ services.api.ports.metrics }}`, but only the owning service's own fields allocate; a reference from elsewhere to an unknown name fails at start as undefined. The primary port is named `default` in `hosts[].port` and `ready.tcp`.

Replicas each get their own ports: `${{ self.port }}` differs per replica, named ports too. `replicas` is a plain number and takes no expression, since every lane allocates the same set.

### Reading other services

| Expression | Value |
| --- | --- |
| `${{ services.api.port }}` | the allocated port |
| `${{ services.api.url }}` | `http://127.0.0.1:<port>`, for server-to-server calls inside the lane |
| `${{ services.api.public_url }}` | `https://api.acme-2.localhost` (with `:8443` when the proxy fell back), for browsers and anything that must see the hostname |
| `${{ services.api.hosts }}` | the list of hostnames |
| `${{ lanes.main.services.api.url }}` | the same service in lane 0, for profiles that reuse one backend |
| `${{ projects.billing.services.api.public_url \| default('https://billing.staging.example.invalid') }}` | a service of another running devlanes project |

A dash in a name becomes an underscore: `services.crm_api`, or `services['crm-api']`. `public_url` does not exist for a service without hosts; filter it with `default` or use `url`.

Two services may read each other's `url` and `public_url`. Only `env.PORT` and `hosts` take part in the cycle check.

### Containers

`${{ infra.postgres.port }}` is the host port of a container's first published port; `${{ infra.postgres.ports['5432'] }}` picks a container port for Compose services and `${{ infra.cerbos.ports.http }}` a named port for plain containers. See [infra](infra.md).

## Environment

### Layers

Layers, lowest first. A later layer wins on the same key.

1. The environment of the shell that ran `devlanes start` (or `restart`, `run`, `exec`, `lane resume`). The daemon keeps it in memory for the lane and reuses it for restarts from the TUI; it is never written to disk. `devlanes daemon restart` starts what ran again with the environment of the shell that ran it. After a reboot, a crash or `daemon stop`, a plain `devlanes start` brings its lane back with the current shell's environment and names the other lanes that ran; `devlanes lane resume` brings them all back.
2. Contributors: mise, direnv, then the platform (`NODE_EXTRA_CA_CERTS`, `SSL_CERT_FILE`, …), `WRANGLER_REGISTRY_PATH`, `CLOUDFLARE_DEV_REGISTRY_PATH` and `MINIFLARE_REGISTRY_PATH` (set on every unit, read only by Wrangler and Miniflare) and the preset.
3. [devlanes' own variables](#devlanes-variables): `DEVLANES=1`, `DEVLANES_PROJECT`, `DEVLANES_LANE`, `DEVLANES_LANE_NAME`, `DEVLANES_LANE_DATA`, `DEVLANES_CA_CERT`, `DEVLANES_CA_BUNDLE`; for services also `PORT`, `HOST`, `DEVLANES_SERVICE` and `DEVLANES_ORIGINS`.
4. `global_env`.
5. The unit's own `env`.
6. The profile's `env` (later `--profile` flags win).
7. `--env KEY=VALUE` on the command line.

On Windows, variable names are uppercased in every layer, since Windows treats `Path` and `PATH` as one variable; `${{ env.Path }}` reads `PATH`.

`devlanes env show api --explain` prints every variable with its layer; `devlanes env shell api` opens a shell with that environment; `devlanes env export | eval` loads the lane's variables into the current shell.

Tasks and infra commands get layers 1 to 7 as well, minus the service-only variables, plus Compose variables (`COMPOSE_PROJECT_NAME`, `COMPOSE_FILE`, `COMPOSE_PROFILES`, `DEVLANES_COMPOSE_LANE_PROJECT`) that let `docker compose exec` reach the project's containers without flags.

### devlanes variables

Every process devlanes starts (services, tasks, infra commands) and every command run with `devlanes with`, `devlanes exec` or `devlanes env shell` gets these. A program or script that wants to know whether it runs under devlanes checks `DEVLANES=1`, for example to skip its own port selection or to read its lane's data folder.

| Variable | Value |
| --- | --- |
| `DEVLANES` | `1` |
| `DEVLANES_PROJECT` | `project` from `devlanes.yaml`, such as `acme` |
| `DEVLANES_LANE` | the lane index: `0` for the primary worktree, then `1`, `2`, … A lane whose branch becomes the primary one takes index 0, so key lasting state on `DEVLANES_LANE_DATA` instead |
| `DEVLANES_LANE_NAME` | `main` for lane 0, `lane<N>` for the others |
| `DEVLANES_LANE_DATA` | the [lane data folder](#lane-data-folder), `~/.devlanes/data/<project>/<lane id>`. The lane id is not the index |
| `DEVLANES_CA_CERT` | the root certificate (PEM) the HTTPS proxy signs with: `~/.devlanes/ca/ca.pem`, or mkcert's `rootCA.pem` with `tls.ca: mkcert` |
| `DEVLANES_CA_BUNDLE` | `~/.devlanes/ca/bundle.pem`, the system roots plus that certificate, for tools that replace their trust store instead of adding to it |

The CA variables are missing when the CA could not be loaded and HTTPS is off; `devlanes doctor` says why. For Node, Bun, Deno, Python and Rust units the platform contributors already point `NODE_EXTRA_CA_CERTS`, `SSL_CERT_FILE` and similar variables at them, so those runtimes trust the proxy's certificates without setup.

Services also get:

| Variable | Value |
| --- | --- |
| `PORT` | the service's port, see [the service's port](#the-services-port) |
| `HOST` | `127.0.0.1` |
| `DEVLANES_SERVICE` | the service's name, the same for every replica |
| `DEVLANES_ORIGINS` | the comma-separated list of every https origin in the lane plus `http://localhost:<port>` and `http://127.0.0.1:<port>` for each service. Feed it to the CORS and CSRF allow-lists of your APIs so lane 3's frontend may call lane 3's API |

A service that needs its own public URL, for OAuth callbacks or absolute links, sets it in `env` with `${{ self.public_url }}`, and another service's with `${{ services.x.public_url }}`.

`DEVLANES_PUBLIC_URL_<SERVICE>` (uppercase, non-alphanumerics replaced by `_`) exists only in the lane-level environment: `devlanes with`, `devlanes exec`, `env show` without a unit. Tests and scripts read their base URLs from it; for a service without hosts the value is its `http://127.0.0.1:<port>` URL. Services do not get these variables.

### Expressions

Values may contain `${{ ... }}` (MiniJinja), evaluated per lane. A value that is exactly one expression keeps its type (`autostart: ${{ lane.index == 0 }}` is a boolean); mixed text is a string. Undefined names are errors; `| default('x')` makes them optional. Filters such as `upper`, `replace`, `join`, `int` and the `a if cond else b` form are available.

| Namespace | Contents |
| --- | --- |
| `project` | `name`, `primary_path` (the primary worktree, for files only it has) |
| `lane` | `index`, `name`, `label` (branch with `/` as `-`), `path`, `data_path` (the [lane data folder](#lane-data-folder)), `domain`, `port` (block start), every `lanes.vars` key |
| `self`, `services.<name>`, `lanes.main.services.<name>` | `name`, `port`, `ports.<name>`, `url`, `public_url`, `hosts` |
| `infra.<container>` | `port`, `ports` |
| `env` | the shell that ran devlanes, then the lane's `.env` |
| `replica` | `index` (from 1), `count` |
| `projects.<project>.services.<name>` | a service of another running project |

`lanes.vars` compute per-lane values once and expose them as `lane.<name>`:

```yaml
lanes:
  vars:
    db_name: acme_${{ lane.name }}
    redis_db: ${{ lane.index }}
    vhost: ${{ 'acme' if lane.index == 0 else 'acme_' ~ lane.name }}
global_env:
  DATABASE_URL: postgres://postgres:postgres@127.0.0.1:${{ infra.postgres.port }}/${{ lane.db_name }}
  REDIS_URL: redis://127.0.0.1:${{ infra.redis.port }}/${{ lane.redis_db }}
```

A var named `domain` replaces the default lane domain and `port` fixes where the block starts. `lanes.matrix` is the explicit alternative: entry N describes lane N.

Not every field sees every namespace. `lanes.vars` see `project`, `lane` and `env`. `infra.*` fields see `project`, `lane`, `infra` and `env`, but not `services`. Tasks and infra commands have an empty `self` apart from `name`.

### .env files

`${{ env.X }}` reads the shell that ran devlanes first, then `<worktree>/.env`, falling back to the primary worktree's `.env` (new worktrees have no gitignored files). The `.env` file feeds expressions only; devlanes never injects it into processes. Name what a process needs:

```yaml
global_env:
  SENTRY_DSN: ${{ env.SENTRY_DSN | default('') }}
  STRIPE_KEY: ${{ env.STRIPE_KEY }}          # from the shell or .env
```

or let the app load its own dotenv. Most dotenv loaders do not overwrite variables that are already set, so `global_env` wins over the file when both define a key.

Compose interpolation is separate: `infra.env` and the worktree's `.env` (passed with `--env-file`) reach `docker compose`, the process environment does not.

## Local overrides

`devlanes.local.yaml` holds the changes one developer wants on their machine only: a service turned off, a variable pointing at a local tool, a service or an [external port](infra.md#external-ports) that only they run. It sits next to `devlanes.yaml`, takes the same keys and stays out of Git. devlanes does not add it to `.gitignore` for you, so add `devlanes.local.yaml` there. Editors validate it with the same modeline as `devlanes.yaml`, `# yaml-language-server: $schema=https://devlanes.run/v1/schema.json`.

```yaml
# devlanes.local.yaml
services:
  web:
    autostart: false                    # a plain `devlanes start` skips it unless the lane ran it before or the default profile names it
  api:
    env:
      LOG_LEVEL: debug                  # added to api's env; its other variables stay
  scratch:                              # a service only this machine runs
    run: node scratch/server.mjs
    hosts: "scratch.${{ lane.domain }}"
infra:
  external:
    pgadmin:                            # a server you start yourself, routed and mirrored
      address: 127.0.0.1:5050
      hosts: "pgadmin.${{ lane.domain }}"
```

Each lane reads the file in its own worktree, and when that worktree has none, as a new one does, the primary worktree's. Changes to it are picked up like changes to `devlanes.yaml`, and an error in it is reported with its own file name and line.

The file is merged over `devlanes.yaml` before templates are expanded:

- Mappings merge by key at every level: services, tasks, templates, profiles, the top-level `tags`, `infra.containers`, `infra.external`, `infra.setup` and `infra.teardown`, every `env`, `lanes.vars`, a container's `ports`, a service's `ready` and `stop`, and preset options. A key the local file leaves out keeps its value from `devlanes.yaml`.
- Lists and single values replace: a service's `tags`, `hosts` and `sync`, `autostart`, `infra.hidden`, `infra.per_lane`, `lanes.matrix` and the like. An `infra.hosts` entry replaces that container's hostnames, and a profile written as a list replaces only its `include`.
- Alternatives drop each other: `run` drops the `run_script` from `devlanes.yaml` and the reverse (a task's `steps` too), and one `ready` check (`tcp`, `http`, `log`, `cmd`) drops the others. A `run_script` may set only `file` or only `name` and keep the other.
- A service that exists only in the local file needs `run` or `run_script`, its own or from the template it `extends`. `project` is refused there: the name lives in `devlanes.yaml`, so every machine gives the project the same lanes, domains and container names.

Nothing can be deleted: turn a service off with `autostart: false`, and empty a list by setting it to `[]`.

## Lane data folder

Every lane has a folder of its own outside the worktree, for state and generated files that must not be shared with another lane: local database files, emulator state, generated configs, sockets. Expressions read it as `${{ lane.data_path }}` and every process gets it as `DEVLANES_LANE_DATA`.

```yaml
services:
  api:
    run_script:
      file: apps/api/package.json
      name: dev
    env:
      SQLITE_PATH: ${{ lane.data_path }}/api/dev.db
  worker:
    preset: wrangler
    run_script:
      file: workers/api/package.json
      name: dev
    wrangler:
      args: [--persist-to, "${{ lane.data_path }}/wrangler/${{ self.name }}"]
infra:
  containers:
    minio:
      image: minio/minio
      command: [server, /data]
      volumes: ["${{ lane.data_path }}/minio:/data"]
      per_lane: true
```

It lives at `~/.devlanes/data/<project>/<lane id>`, where the lane id is a number the daemon gives the lane when it creates it. The id does not change when the lane's index does (a lane whose branch becomes the primary one takes index 0), so data stays with its lane. `lanes.data_dir` in `~/.devlanes/config.yaml` moves the root, for example to a faster disk:

```yaml
# ~/.devlanes/config.yaml
lanes:
  data_dir: /Volumes/fast/devlanes
```

On Windows the folder is under `%USERPROFILE%\.devlanes\data`, `lanes.data_dir` may also start with `~\` or a drive letter, and `${{ lane.data_path }}` and `DEVLANES_LANE_DATA` are written with `/` (`C:/Users/acme/.devlanes/data/acme/3`), which Windows programs accept and `sh` strings leave intact.

devlanes creates the folder when the lane is created and again before each start, run and container start, so deleting it by hand is safe while the lane is stopped. `devlanes lane clean` deletes it after `lane.clean` tasks and teardown have run, and the project's folder goes with its last lane. A lane that comes back after a clean starts with an empty folder.

A worktree is per lane only while each lane has its own worktree. `devlanes lane new --name` adds a lane to a folder that already has one, and those lanes share everything inside the worktree: `.wrangler/`, `node_modules/.vite`, build output and any file a script generates there. Use the data folder for anything a lane writes while it runs.

devlanes keeps its own per-lane files here too, each in a folder named after the integration. Do not write into these yourself:

| Folder | Written by |
| --- | --- |
| `wrangler-registry` | Wrangler and Miniflare, including the Cloudflare Vite plugin, in any unit of the lane (`WRANGLER_REGISTRY_PATH`, `CLOUDFLARE_DEV_REGISTRY_PATH`, `MINIFLARE_REGISTRY_PATH`) |
| `tauri` | the tauri preset's config overlays |
| `compose` | the Compose override for `per_lane` services |
