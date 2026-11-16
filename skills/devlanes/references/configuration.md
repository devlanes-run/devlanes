# Writing devlanes.yaml

Read this before you create or change `devlanes.yaml` or `devlanes.local.yaml`. It covers each area with the keys that matter and the mistakes to avoid. Every key, with its default, is in the [config reference](https://devlanes.run/docs/reference/config.md); the schema is `https://devlanes.run/v1/schema.json`.

## After every edit

1. `devlanes config check`: validates keys, expressions, references and preset options, and names the file, line and column of a mistake. It does not check that a script named in `run_script` exists.
2. An edit never restarts what runs. `devlanes status` shows the lane as `config changed` with the units the edit changed, added or removed.
3. `devlanes reload` stops removed units, restarts changed ones and starts added ones. A change to infra or ports restarts the whole lane. `devlanes reload api` applies only that unit's change.
4. `devlanes debug config` prints the merged config, with `extends`, templates and local files applied and expressions left unevaluated.

## File layout

`devlanes.yaml` sits at the repository root, or at the root of an app in a monorepo as a project of its own, and is committed so every worktree has it. Start it with the schema line so editors complete keys:

```yaml
# yaml-language-server: $schema=https://devlanes.run/v1/schema.json
project: myapp
```

`project` is lowercase letters, digits, `.`, `_` and `-`, starting with a letter or digit. The other top-level keys are `extends`, `lanes`, `infra`, `global_env`, `tags`, `templates`, `tasks`, `services` and `profiles`.

Commands use the nearest `devlanes.yaml` above the current directory, up to the repository root. A `devlanes.yaml` in a subdirectory is a separate project, and its relative paths (`cwd`, where `run` starts, `run_script.file`, `sync.from`, `infra.compose`) start at that subdirectory.

Renaming `project` makes a new project with new lanes, ports, hostnames and containers. The old project's lanes stay listed until `devlanes lane gc --yes` cleans them. Tell the user before renaming, and run the gc only when they agree. Another repository cannot use a name already taken (`project_name_taken`).

## A second project file

A second file next to `devlanes.yaml` runs the same apps another way, such as a production build. It extends `devlanes.yaml` and sets its own `project`:

```yaml
# devlanes.prod.yaml
extends: devlanes.yaml
project: myapp-prod
global_env:
  NODE_ENV: production
tasks:
  install:
    run: [npm, install, --include=dev]   # NODE_ENV=production reaches tasks too
profiles:
  default:
    include: ["*"]
    run_scripts:
      dev: start
```

- `include` is required. A `default` profile without `include` selects no services, so the start runs none, unless the extended file's `default` brings an `include`.
- `run_scripts` swaps the script name for services whose `run_script` names `dev`. Each such `package.json` needs a `start` script; `devlanes config check` does not notice a missing one, the start fails on it.
- The extended files merge first, each followed by its local file, then this file and `devlanes.prod.local.yaml`. Later files win: mappings merge by key, lists and single values replace. An extended file may be partial; its `project` is ignored.
- Name it once with `devlanes --file devlanes.prod.yaml start`, then reach it with `devlanes -p myapp-prod status`.
- Both projects run side by side (`myapp.localhost` and `myapp-prod.localhost`). They share the shared Compose project (`devlanes infra down` from one stops it for both), any value built only from `lane.name` such as a database name (put `${{ project.name }}` in it), and build output in the worktree such as `.next`.

## Services

A service is a long-running process started with the lane. Exactly one of `run` or `run_script` is required, directly or through a template.

```yaml
services:
  api:
    run_script:                     # the project's package manager runs the script
      file: api/package.json
      name: dev
    hosts: "api.${{ lane.domain }}"
    ready:
      http:
        path: /healthz
  worker:
    run: [node, worker.mjs]         # a list is an argv, run without a shell
    cwd: worker
    ready:
      log: "worker ready"
  docs:
    run: >-                         # a string runs with /bin/sh -c
      pnpm -F docs dev --port ${{ self.port }} --host 127.0.0.1
    hosts: "docs.${{ lane.domain }}"
```

- `run_script: { file, name }` works with `package.json` (npm, pnpm, yarn or bun, from `packageManager` or the lockfile), `Makefile`, `justfile`, `Taskfile.yml` (task; preset flags arrive as `{{.CLI_ARGS}}`), `deno.json`, `pyproject.toml` (uv or poetry), `Cargo.toml` (a binary) and `go.mod` (`go run`; `api` means `./cmd/api`, and `.` or a path with `/` is relative to `go.mod`). It runs in that file's directory, so `devlanes config check` refuses a `cwd` next to it. Prefer it to `run: pnpm -F pkg dev` whenever the command is a script in a file: profiles can swap its name and presets require it.
- A `run` string is a shell script, run with `/bin/sh -c` (Git's `sh` on Windows). Without `cwd`, `run` starts in the project file's directory.
- `autostart: false` keeps a service out of a plain `devlanes start` when no `default` profile exists, unless the lane ran it before; naming it or its tag still starts it. It also takes an expression, such as `${{ lane.index == 0 }}`.
- `keep_awake: true` on a worker, queue consumer, scheduler or tunnel keeps its lane from being throttled or frozen while it runs. Without it, a lane with no requests, connections or devlanes commands is frozen after a while, and a worker waiting for jobs stalls until something thaws the lane.
- `replicas: N` runs N copies, each with its own port; `replica.index` counts from 1, and `worker.2` names one.
- `stop: { signal: SIGINT, timeout: 15s }` replaces the default SIGTERM and 10 seconds before the process tree is killed.
- `pty: false` uses pipes instead of a terminal. `hidden: true` keeps the service away from machines connected with `devlanes remote connect`.
- `name: API Admin` sets the label in the TUI and human output. Commands and expressions keep using the key, so keep keys short, lowercase and dash-separated.
- `sync` copies a folder the service builds to a fixed path, for a browser extension loaded unpacked: `sync: [{ from: dist, ignore: ["*.map"] }]`, by default to `~/devlanes/sync/<project>/<lane>/<folder>`, and to a connected machine too.

devlanes has no dependencies between services and does not restart a process that exits. Order comes from infra, setup and tasks; a crash shows as `failed` with its log tail.

## Readiness

`devlanes start` returns once every started service is ready or one failed. Without `ready`, a service with `hosts` is ready when its port accepts TCP, and one without hosts once its process has run half a second.

Give every server a check that fails when the app is broken. Dev servers under a watcher (`node --watch`, `tsx watch`, `nodemon`, `cargo watch`, `uvicorn --reload`) keep running after the program crashes, so `ready: started` reports them ready while nothing works. For a worker without a port, have it print a line once it is connected and wait for that line.

```yaml
ready:                # a route the app serves
  http:
    path: /healthz
    timeout: 60s
ready:                # the service's own port accepts connections
  tcp: true
ready:                # a named port
  tcp: inspector
ready:                # a regex on its output, escape codes removed
  log: "worker ready"
ready:                # a command that exits 0
  cmd: "pg_isready -h 127.0.0.1 -p ${{ infra.postgres.port }}"
ready: started        # only when there is nothing to probe
```

Use one of `tcp`, `http`, `log` or `cmd`. `http` passes on a 2xx answer unless `status` is set, and does not follow redirects. Make `timeout` generous for services that compile first. A route the app serves beats `tcp`, which only proves the socket is open. A service that fails its ready check keeps running and holds its port.

## Ports

Each lane gets a block of ports and each service one port from it, in `PORT` and `${{ self.port }}`, with `HOST=127.0.0.1`. The service must listen on that port.

- Pass the port with a preset (`preset: vite`), with `--port ${{ self.port }}` in `run`, or let the app read `PORT`. An app that reads another variable gets `MY_PORT: "${{ self.port }}"`.
- Never write a literal port. `env: { PORT: 3000 }` is the same in every lane, so two lanes collide.
- Never write `env: { PORT: "${{ self.port }}" }`: `self.port` comes from `PORT`, devlanes rejects the cycle, and `PORT` already holds that value.
- Never put `PORT` in `global_env`: it sits above devlanes' own variables and gives every service the same port.
- A second port: reference `${{ self.ports.inspector }}` in the service and devlanes gives it a port named `inspector`.
- Other services: `${{ services.api.port }}`, `${{ services.api.url }}` (`http://127.0.0.1:<port>`), `${{ services.api.public_url }}` (its HTTPS hostname). Write dashes as underscores: `services.admin_api`.

## Hostnames and HTTPS

```yaml
hosts:
  - api.${{ lane.domain }}
  - host: "*.${{ lane.domain }}"      # wildcard, used only when no other route matches
    fallback: true
  - host: inspect.${{ lane.domain }}  # routes to a named port
    port: inspector
  - host: funnels.${{ lane.domain }}  # sends the upstream address as Host
    host_header: upstream
```

`lane.domain` is `myapp.localhost` in lane 0 and `myapp1.localhost` in lane 1. `${{ domain('myapp-pages') }}` applies the same rule to another name, for hostnames that must not share the app's cookies. Keep every hostname under `lane.domain` or a `domain()` name, so lanes never claim the same one. The proxy serves them over HTTPS on 443, or 8443 when it cannot bind 443.

The certificates are already handled. Units of a detected platform get the devlanes CA: `NODE_EXTRA_CA_CERTS` for node and bun, `DENO_CERT` for deno, `SSL_CERT_FILE` and `REQUESTS_CA_BUNDLE` for python, `SSL_CERT_FILE` for rust and go (Go reads it on Linux and uses the system store on macOS and Windows). Every unit gets the paths in `DEVLANES_CA_CERT` and `DEVLANES_CA_BUNDLE`, for a runtime devlanes does not detect, such as `run: SSL_CERT_FILE=$DEVLANES_CA_BUNDLE bin/rails server`. Do not add `NODE_OPTIONS=--use-system-ca`, `NODE_TLS_REJECT_UNAUTHORIZED=0` or your own `NODE_EXTRA_CA_CERTS`. The user runs `devlanes setup` once for browsers and the system ([setup.md](setup.md)). `lanes.hosts_file: always` also writes the exact hostnames to `/etc/hosts` on macOS and Linux, for programs that do not resolve `*.localhost`; wildcards cannot go there.

## Environment and expressions

Values may contain `${{ ... }}` expressions, evaluated for each lane. An undefined name is an error; `${{ env.X | default('') }}` makes it optional. A value that is one whole expression keeps its type. Write `${{ '${{' }}` for a literal `${{`.

| Namespace | Contents |
|---|---|
| `project` | `name`, `primary_path` (the primary worktree, for files only it has) |
| `lane` | `index`, `name`, `label`, `path`, `data_path`, `domain`, `port` (block start), and each `lanes.vars` entry |
| `self`, `services.<name>` | `name`, `port`, `ports.<name>`, `url`, `public_url`, `hosts` |
| `infra.<container>` | `port` (its main published port), `ports.<name or container port>` |
| `env` | The environment of the shell that started the lane, then the lane's `.env` |
| `replica` | `index`, `count` |
| `lanes.main.services.<name>` | Services of lane 0 |
| `projects.<project>.services.<name>` | Services of another devlanes project |

Layers, lowest first: the shell that started the lane, tools and runtimes (mise, direnv, the CA variables, a preset's settings), devlanes' variables (`DEVLANES_*`, and `PORT`, `HOST` for services), `global_env`, the service's `env`, the profile's `env`, `--env` on the command line. `devlanes env show api --explain` names the layer of each variable.

`.env` is never put into processes; it only feeds `${{ env.X }}`. Name what a process needs (`SMTP_HOST: "${{ env.SMTP_HOST }}"`) or let the app load its own dotenv. Never copy `.env` values into the project file. mise `[env]` and an allowed `.envrc` already reach every unit.

[Expressions](https://devlanes.run/docs/reference/expressions.md) lists the functions and filters, and [Environment variables](https://devlanes.run/docs/reference/environment-variables.md) every variable devlanes sets.

## Lanes

```yaml
lanes:
  primary: main                        # branch of lane 0
  max: 6
  vars:
    db_name: myapp_${{ lane.name }}
```

`lane.data_path` (`DEVLANES_LANE_DATA` in processes) is a folder only this lane uses, outside the worktree. devlanes creates it with the lane and deletes it when the lane is cleaned. Put what a lane writes while it runs there (SQLite files, `wrangler dev --persist-to`, generated configs), never at a fixed path in the worktree: several lanes can run from one folder (`devlanes lane new --name review`).

`vars` are per-lane values read as `lane.<name>`: database names, vhosts, buckets, Redis database numbers. Build names from `lane.name`, which is `main` in lane 0 and `lane1`, `lane2` and so on in the others, whatever the branch and whatever name `devlanes lane new --name review` gave the lane. `lane.label` is the branch with each `/` as `-`, and `lane.index` the number. All three can be read inside `vars`. A var named `domain` replaces the lane domain and one named `port` fixes the block start; set them only when the default is wrong. `new_worktrees` (`register`, `setup`, `start`) and `removed_worktrees` (`stop`, `clean`) default to `register` and `stop`; leave them out unless the user wants another policy.

## Infra

Containers start and turn healthy before setup, tasks and services. Compose services are shared by every lane unless listed in `per_lane`.

```yaml
infra:
  compose: docker-compose.yml          # or a list, merged like repeated -f
  exclude: [caddy]                     # never started; the devlanes proxy replaces it
  per_lane: [mailpit]
  env:                                 # Compose interpolation only; processes do not see it
    POSTGRES_PORT: 15432
  hosts:
    mailpit: "mail.${{ lane.domain }}"
  containers:                          # plain Docker containers without Compose
    cerbos:
      image: ghcr.io/cerbos/cerbos:0.40.0
      ports:
        http: 3592
      per_lane: true
```

`exclude` takes names of services in the Compose file, and a name the file lacks fails `devlanes config check`. Processes reach a container through `${{ infra.postgres.port }}`. Keep databases, brokers and object stores in infra, not as services. A shared container keeps one copy of the data, so give each lane its own database, vhost or bucket with `setup` and `lanes.vars`.

A server devlanes does not run, such as a pgAdmin the developer starts by hand or a server started with `devlanes exec`, goes under `infra.external` with its loopback address. Every lane lists it as a unit of kind `external`, the proxy routes its `hosts` to the port, and connected machines mirror it. Only `127.0.0.1:<port>` and `[::1]:<port>` are accepted. Use a port outside `ports.range` of the user config (20000-32767 by default) and a name no `exec` command uses. `start`, `stop` and `logs` do not apply to it. Put an entry only one developer needs in `devlanes.local.yaml`.

```yaml
infra:
  external:
    pgadmin:
      address: 127.0.0.1:5050
      hosts: "pgadmin.${{ lane.domain }}"
```

## Setup and teardown

Named commands that run once per lane after its containers are healthy, recorded as done and skipped on later starts. They run again when their declaration changes. `teardown` runs when the lane is cleaned. As units they are `setup.<name>` and `teardown.<name>`, and `devlanes run infra.setup.database` runs one by hand.

```yaml
infra:
  setup:
    database: docker compose exec -T postgres createdb -U postgres ${{ lane.db_name }}
    vhost:
      run: docker compose exec -T rabbitmq rabbitmqctl add_vhost ${{ lane.vhost }}
      container: rabbitmq                # run again whenever this container restarted
      ignore_errors: true
      timeout: 60s
  teardown:
    database: docker compose exec -T postgres dropdb -U postgres --if-exists ${{ lane.db_name }}
```

## Tasks

One-shot commands with `run`, `run_script` or `steps`, plus `cwd`, `env` and `timeout`.

```yaml
tasks:
  install:
    run: [npm, install]
    when: lane.create
    timeout: 10m
  migrate:
    run_script:
      file: api/package.json
      name: migrate
    when: lane.start
  seed:
    run: node seed.mjs
    when: lane.create
    after: services
  reset-db:
    steps: [infra.teardown.database, infra.setup.database, migrate, seed]
    confirm: Drops ${{ lane.db_name }} and seeds it again
  login:
    run: npx prisma login
    interactive: true
```

- `when: lane.create` runs on each start until it has succeeded once for the lane, and again after its declaration changes (installs, codegen, a first seed). `lane.start` runs on every start that brings the lane up, before services (migrations). `lane.stop` runs when the whole lane stops, `lane.clean` when it is cleaned. Without `when`, a task runs only through `devlanes run NAME`.
- Create and start tasks run in the order the file lists them. A start into a running lane runs none.
- `after: services` runs the task once every started service is ready.
- `profiles: [default, e2e]` runs an automatic task only under those profiles.
- Put `confirm` on anything destructive; without a terminal, `devlanes run` then needs `--yes`.
- `interactive: true` for commands that need the user's terminal; it cannot have `when`, and you cannot run one, so ask the user to.
- A command that ends by itself is a task. As a service it would show as failed or stopped once it exits.

## Tags and profiles

```yaml
tags:
  api:
    color: cyan
profiles:
  default: [+essential]                # used when no --profile is given
  backend: [+api, worker]
  e2e:
    include: ["*"]
    run_scripts:                       # services whose run_script names dev run start
      dev: start
    env:
      NODE_ENV: test
  web-only:
    include: [+web]
    infra: false
```

Profiles select services by name, glob or `+tag`, and `exclude` removes some again. Globs skip `autostart: false` services; names and tags include them. When a `default` profile exists, a plain `devlanes start` of a new or stopped lane starts exactly its selection, so use either a `default` profile or `autostart: false`, not both. Without one, a plain start brings back the services the lane ran and nobody stopped. Under a profile with `infra: false`, a value that reads `infra.*` resolves only while that container already runs, as a shared one another lane started does. Otherwise a service or task whose `env` reads it fails with `... has no value in lane ...`, and a `global_env` or profile `env` entry that reads it fails every service and task of the lane. So when the project has such a profile, put `DATABASE_URL` and the like in the `env` of the services that use them, not in `global_env`. `devlanes exec` leaves the entry out and names it. Tags are any names in a service's `tags`; declaring one under `tags` only sets its color or description. Put a category tag such as `api`, `web` or `worker` first, so colors follow it.

## Templates

```yaml
templates:
  node-api:
    tags: [api]
    ready:
      http:
        path: /healthz
services:
  admin:
    extends: node-api
    run_script:
      file: admin/package.json
      name: dev
    hosts: "admin.${{ lane.domain }}"
```

A service's `extends` names a template; a file's `extends` names project files. Templates apply after local files and file-level `extends`. Mappings (`env`, `ready`, `stop`, preset options) merge by key; lists (`tags`, `hosts`) and single values replace. Setting `run` or `run_script`, or one `ready` check, replaces the inherited one.

## Presets and platforms

`platform` (`node`, `bun`, `deno`, `python`, `rust`, `go`) is detected from the `run_script` file or the working directory. Set it only when detection is wrong, or to `none` to skip it. It adds the CA and color variables.

| Preset | Adds | Options |
|---|---|---|
| `vite` | `--port <port> --strictPort --host 127.0.0.1` | `vite: { args }` |
| `next` | `--port <port> --hostname 127.0.0.1` | `next: { args }` |
| `wrangler` | `--port`, `--inspector-port` (a port named `inspector`), `--var` per var, and a registry path per lane | `wrangler: { vars, args, isolate_registry }` |
| `tauri` | A per-lane, per-replica identifier and dev URL, through an overlay in the lane data folder | `tauri: { identifier, dev_url, product_name, before_dev_command }` |

A preset needs `run_script`; with `run`, devlanes refuses the preset, so drop it and write its flags into `run`. With `run_script`, the flags are passed to the script, so the framework command must be the script's last command. The wrangler preset refuses a script or `wrangler.args` that already passes `--port` or `--inspector-port`. Do not set `WRANGLER_REGISTRY_PATH`, `CLOUDFLARE_DEV_REGISTRY_PATH` or `MINIFLARE_REGISTRY_PATH`; devlanes sets them per lane for every unit. Wrangler keeps its local state in the worktree's `.wrangler/`, so give each lane its own with `wrangler: { args: [--persist-to, "${{ lane.data_path }}/wrangler/${{ self.name }}"] }`. Workers that bind the same KV namespace, D1 database or R2 bucket id see the same local data only through one folder: give those `${{ lane.data_path }}/wrangler` without the service name. devlanes starts them at the same time and cannot start one after another.

## Local file and user config

`devlanes.local.yaml` next to `devlanes.yaml` holds one developer's changes and stays out of Git; add it to `.gitignore`, since devlanes does not. Another project file has its own, such as `devlanes.prod.local.yaml`. It merges by key over the project file, for example `services: { web: { autostart: false } }`; lists and single values replace. It cannot set `project` or `extends`. A lane whose worktree has none reads the primary worktree's.

Machine settings live in the user config, `~/.devlanes/config.yaml`, never in the project file: `ports.range`, `proxy` ports, `tls` stores, `hostnames`, `startup`, `lanes` freezing times, log retention, `updates`. Read them with `devlanes config get proxy.https_port`. Change them only when the user asks, with `devlanes config set KEY VALUE`, which keeps comments, refuses a value that would not load and says whether the change needs `devlanes daemon restart`; `devlanes config unset KEY` restores the default. None of these needs the daemon. An unknown key or invalid value written by hand keeps the daemon from starting; `devlanes doctor` reports the file and line.

## Checklist

- Each service has `run_script` or `run`, listens on its own port, keeps its hostnames under `lane.domain` and has a ready check that fails when the app is broken.
- No literal ports, no `PORT` in `global_env`, no CA workarounds, no secrets or `.env` values in the file.
- One-shot work is a task with the right `when`; destructive tasks have `confirm`.
- Databases and brokers are infra, with per-lane names from `setup` and `lanes.vars`.
- A `default` profile with options has an `include`.
- Defaults are left out: a detected `platform`, the default lane policies, a `lanes.vars.domain` equal to the built-in one.
- `devlanes config check` passes and `devlanes start` reaches ready.
