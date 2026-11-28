# Contributing

devlanes is a Cargo workspace: the `devlanes` binary (`bins/devlanes`) and the `devlanes-gui` binary that ships next to it (`bins/devlanes-gui`, the GUI hosts, kept apart so `devlanes` links no GUI libraries), built from small crates under `crates/`. Each crate has a README saying what it does, and `AGENTS.md` has the code conventions. This file covers setting up, running the CLI from a checkout, testing each area, the design and observability.

## Setup

- Rust comes from `rust-toolchain.toml`; rustup installs the pinned version on the first `cargo` call.
- Install nextest: `cargo install cargo-nextest --locked`.
- Install kache, the build cache every compile goes through (`.cargo/config.toml` sets it as rustc wrapper): `cargo install kache --locked` (2.0.0 or later; CI pins 2.3.0), then `kache init` once.
- macOS (Apple Silicon and Intel), Linux with glibc (x86_64 and arm64) and Windows 11 24H2 or later (x64 and arm64) are supported.
- On Windows, also install the Visual Studio Build Tools (the "Desktop development with C++" workload) and [Git for Windows](https://gitforwindows.org). The same `cargo install kache --locked` works there. kache doesn't cache executables on Windows by default; `[cache] cache_executables = true` in its config is an optional speed-up that restores test binaries and build scripts instead of linking them again. CI's test and lint jobs turn it on with `KACHE_CACHE_EXECUTABLES=1`, and the Windows release build in `build.yml` turns it off.

Optional tools, needed only by the ignored tests that mention them: Docker (OrbStack or Docker Desktop on macOS, Docker Desktop on Windows), `node` with npm/pnpm/yarn/bun, `just`, `deno`, `uv`, `poetry`, `mise`, `direnv`, `certutil` (NSS), `dig`, `tmux`.

CI also shares kache's cache through a remote bucket. Pushes to `main` write to it, and pull requests write to a separate prefix that `main` never reads. Contributors don't need the remote; kache's local cache is enough, and connecting to a remote is an optional setting in your own `~/.config/kache/config.toml`.

## Checks

Run these before sending a change. CI runs the same ones on macOS, Linux and Windows.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo test --workspace --doc --all-features
cargo build --workspace --all-targets --all-features
```

While working on one crate, test just that crate: `cargo nextest run -p devlanes-config`.

CI skips what a change can't affect. Against a pull request's base, or on `main` the last commit whose CI passed, `.github/scripts/affected.js` picks the jobs to run and the crates whose tests run: each changed crate and every crate that depends on it, and for a `Cargo.lock` change the crates whose locked dependencies changed. Files outside the crates select nothing. When a crate's build or tests read a file outside its directory, list it under `[package.metadata.ci]` in the crate's `Cargo.toml` (`build-inputs` or `inputs`), or the change won't run its tests. Running the CI workflow by hand runs everything. Only one check, `ci`, is required to merge; it fails when any job failed.

Changes to shell scripts or workflows also have a check, which CI runs in its `packaging` job:

```sh
git ls-files -z '*.sh' | xargs -0 shellcheck
actionlint
sh packaging/scripts/check.sh   # release scripts and install.sh against fake binaries
```

Changes to Markdown files are checked with [Vale](https://vale.sh/docs/install), which CI runs in its `prose` job (CI pins 3.28.1). `.vale.ini` lists the style packages, the rules turned off and the files skipped:

```sh
vale sync   # once, downloads the style packages into .vale/styles
git ls-files -z '*.md' | xargs -0 vale
```

`install.ps1`, `package.ps1`, the npm launcher and the Windows wheel have a Windows check against a fake `devlanes.exe`, which CI runs in its `packaging-windows` job. It needs PSScriptAnalyzer (CI pins 1.25.0), both `powershell.exe` and `pwsh`, `node` and `python`. It changes your user `PATH` while it runs and writes the old value back at the end:

```powershell
pwsh packaging/scripts/check.ps1
```

## Running the CLI from a checkout

Use a separate state directory so your development daemon never touches a real install in `~/.devlanes`:

```sh
cargo build -p devlanes -p devlanes-testing
export DEVLANES_HOME="$PWD/target/dev-home"
alias dl="$PWD/target/debug/devlanes"

dl doctor
dl daemon status
```

The first command that needs the daemon starts it in the background, from the same binary (`devlanes daemon run`). After rebuilding, restart it so it runs the new code:

```sh
cargo build -p devlanes && dl daemon restart --yes
dl daemon logs -f                              # the daemon's log, $DEVLANES_HOME/logs/daemon.log
DEVLANES_LOG=debug dl daemon restart --yes     # more detail in that log
```

`DEVLANES_OUTPUT=human|plain|agent|json` forces an output mode. When you run the CLI from inside an AI agent session it detects the agent and prints agent output; `DEVLANES_AGENT=0` turns that off.

## Trying a project

`fixtures/` has small projects that use `fixture-server`, a test HTTP server from `crates/testing`, so they run without Node or Docker. Lanes are git worktrees, so copy a fixture into a temporary repo:

```sh
repo="$PWD"
work=$(mktemp -d)
cp fixtures/two-services/devlanes.yaml "$work/" && cd "$work"
git init -q -b main && git add . && git commit -qm init
git worktree add -q -b feat/x "$work-x"

export PATH="$repo/target/debug:$PATH"   # fixture-server must be on PATH
dl start                                 # lane 0, on main
(cd "$work-x" && dl start)               # lane 1, on feat/x
dl status
dl logs api --tail 20
dl ui                                    # the TUI; q quits and leaves everything running
dl stop && dl daemon stop --yes
```

`fixtures/many-services` starts 40 servers, which is useful for TUI and startup-pacing work. The `dogfood-*` fixtures are config-only (they reference compose files that don't exist) and are checked by the `devlanes-config` tests.

## HTTPS and hostnames

The daemon's proxy listens on 443 and 80, and falls back to 8443 and 8080 when it can't bind them. macOS and Windows let unprivileged processes bind both; on Linux, run `sudo setcap cap_net_bind_service=+ep target/debug/devlanes` or use the fallback ports.

```sh
dl url ls                                   # routed hostnames with their lane, unit and port
curl --cacert "$DEVLANES_HOME/ca/ca.pem" https://api.fixture-two.localhost/healthz
dl setup                                    # trusts the dev CA (asks for your password)
dl setup --remove
```

The `/etc/hosts` helper needs a release build, because its installer refuses debug builds. `setup` installs it only where this machine needs it, so force it with the user config where `*.localhost` already resolves:

```sh
cargo build --release -p devlanes-hosts-helper
dl config set hostnames.hosts_file always
DEVLANES_HOSTS_HELPER="$PWD/target/release/devlanes-hosts" dl setup   # CA + helper
dl setup --remove             # removes both
dl setup --remove --keep-ca   # removes only the helper
```

`crates/hosts-helper/README.md` explains what that installs and why. On Windows there is no helper: `*.localhost` resolves natively on Windows 11 (not on Windows Server), `setup` shows a Windows security warning instead of a password prompt, `curl.exe` needs `--ssl-revoke-best-effort`, and with the resolver strategy `setup` writes an NRPT rule after a UAC prompt ([Windows](docs/reference/windows.md)).

## Docker and compose

Docker tests are ignored by default and named `docker_*`:

```sh
cargo nextest run --workspace --run-ignored only -E 'test(/^docker_/)'
```

They create containers labelled `dev.devlanes.*` and remove them afterwards. To try infra by hand, add an `infra:` section to a fixture copy (see `fixtures/examples/devlanes.yaml`) and use `dl infra up`, `dl infra status` and `dl lane clean <lane> --yes`.

## Remote and SSH

Remote is mirroring only: a client never runs commands on a box. Most of it is tested without SSH. `crates/daemon/tests/it/remote.rs` runs a box daemon and a client daemon in one process over an in-memory transport, covering pairing, mirrored ports, proxy routes, the registry feed, sync, reconnect and failure:

```sh
cargo nextest run -p devlanes-daemon -E 'test(/remote/)'
```

The real SSH path runs against a throwaway sshd container (`e2e/ssh/README.md`):

```sh
e2e/ssh/run.sh                              # builds a Linux devlanes for the box, runs the SSH tests
DEVLANES_TEST_SSH_BIN=skip e2e/ssh/run.sh   # no box binary; only the connection test runs
```

To use `dl remote connect` by hand you need a box with `devlanes` installed (on its PATH or in `~/.devlanes/bin`) and an entry for it in `~/.ssh/config`. A Linux VM or a spare machine works best: a box on the same machine as the client is confusing, because mirroring binds the box's port numbers locally and they collide with the box's own listeners.

```sh
ssh mybox devlanes daemon pair   # a one-time code; or set remote.require_pairing: false on the box
dl remote connect mybox      # asks for the code, then streams connection status; Ctrl-C detaches, the connection stays up
dl remote ls                 # mirrored ports, routes, syncs and warnings
ssh mybox devlanes status    # lanes are managed on the box itself
dl remote disconnect mybox
```

## End-to-end tests

`e2e/` runs the real `devlanes` binary against the projects in `fixtures/`. The default nextest profile skips it (`.config/nextest.toml`); the `e2e` profile runs only it. Without `DEVLANES_E2E_BIN_DIR`, each test process on macOS and Linux builds `devlanes`, `devlanes-gui` and `fixture-server` with cargo first; set it to skip that. On Windows tests never run cargo and take the binaries next to the test binary, so the build below is required there. `devlanes-gui` builds in a cargo invocation of its own, as releases build it:

```sh
cargo build -p devlanes -p devlanes-testing
cargo build -p devlanes-gui
export DEVLANES_E2E_BIN_DIR="$PWD/target/debug"
cargo nextest run --profile e2e
cargo nextest run --profile e2e --run-ignored only -E 'test(/^docker_/)'   # needs Docker
```

`e2e/README.md` covers isolation, port slots and debugging.

## Tests that need other tools

Run a crate's ignored tests with `cargo nextest run -p <crate> --run-ignored only`.

| Crate | Needs |
|---|---|
| `devlanes-run-script-npm` | node with npm, pnpm, yarn and bun |
| `devlanes-run-script-just`, `-task`, `-cargo`, `-deno`, `-python` | just, task, cargo, deno, uv or poetry |
| `devlanes-env-mise`, `devlanes-env-direnv` | mise, direnv |
| `devlanes-trust-macos` | someone to click the keychain password prompt |
| `devlanes-trust-linux` | sudo and certutil |
| `devlanes-trust-windows` | a desktop session, to answer the two Windows security warnings for the real user root store |
| `devlanes-resolver-nrpt` | a desktop session to approve two UAC prompts, and nothing (no devlanes daemon) on `127.53.0.1:53`; it writes a rule under `HKEY_LOCAL_MACHINE` and removes it |
| `devlanes-resolver-dns` | dig |
| `devlanes-daemon` | `DEVLANES_FIXTURE_SERVER=$PWD/target/debug/fixture-server` |

The H.264 tests of `devlanes-gui-codec` are not ignored: they run with `--all-features` and skip, printing why, where the machine has no H.264. On Linux they need the distribution's OpenH264 (`libopenh264-dev` on Debian and Ubuntu, `openh264` on Arch and Alpine, and on Fedora `openh264` from the `fedora-cisco-openh264` repository); the decoding tests also need version 2.6 or later, which Debian 13 and Ubuntu 26.04 ship, and otherwise run with Cisco's build through `LD_LIBRARY_PATH`, as CI does. On Windows they use Media Foundation, which client editions have except the N editions without the Media Feature Pack. `DEVLANES_REQUIRE_H264=encode,decode` (or one of the two) turns a skip for that side into a failure.

## Design

A lane corresponds to a Git worktree. Lane 0 is the main checkout (git identity `main`, the folder holding `.git`) on any branch; only a repository without one, such as a bare clone, gives lane 0 to the worktree on the primary branch. Other worktrees keep persisted indexes, and a linked worktree that held lane 0 before the main checkout got a lane moves off it at its next idle start. Under CI whichever lane holds lane 0 keeps it. A worktree other than the main checkout gets its lane the first time a command that acts on lanes (`start`, `status`, `env` and the like, not reads such as `logs`) runs in it, and a lane whose folder is gone stays until `lane gc` cleans it, unless `lanes.removed_worktrees: clean` has the daemon clean it once nothing runs in it. Worktree identity uses the Git common directory and worktree administration entry, so moving a worktree preserves its identity. `lane new --name` adds another lane for one folder: a worktree row whose identity is the folder's plus `#lane=<name>`, which follows that folder and never takes lane 0. Non-Git projects have one lane per folder unless extra lanes are added.

A project is identified by its `project:` name, and a name belongs to one repository. One repository can hold several projects, each read from its own project file: the nearest `devlanes.yaml` above the current directory, up to the repository root, or a file registered once with `--file` and stored with the project relative to the repository root. A worktree has a lane in a project while its copy of that file names the project, so a rename or a branch switch leaves the old lane in place until `lane gc` collects it, once nothing runs there.

The daemon owns processes, ports, proxy routes and SQLite. CLI and TUI clients exchange structured requests and events with it. Closing a client does not stop services. Integration crates implement core traits and are registered by the binary; Docker containers belong to infrastructure adapters rather than process runners.

Each lane receives a non-overlapping block from the machine-wide port pool. Named service offsets persist within that block. A service must accept its assigned port; listening-socket inspection is diagnostic only. Hostnames put the lane in the registrable label, such as `api.acme2.localhost`.

Startup orders infrastructure, per-lane setup, lifecycle tasks, concurrent services, then after-services tasks. devlanes has no service dependency graph or automatic restart: a failed unit stays failed. Each lane saves the services it runs and its profiles (never the environment), so a plain start, `lane resume` or `daemon restart` from a shell brings back what ran after the daemon stopped; a whole-lane `stop` clears that. A restart starts the new daemon before it stops the old one: the new one waits for the singleton lock, with the lanes to start again and the restarting client's environment handed over in memory on its stdin, so the restart finishes even when that client goes away. Monitoring reports resource use without enforcing limits. Throttling and freezing an idle lane is neither automatic restart nor enforcement: its processes keep their state, ports and PIDs, and activity on the lane thaws them ([Throttling and freezing](docs/concepts/freezing.md)).

YAML configuration merges the files a project file `extends`, each followed by its local file, then the project file and its local file. Maps merge by key and lists/scalars replace, and templates expand after the merge. `${{ ... }}` expressions use strict undefined-variable handling. Client environments stay in daemon memory; `.env` files supply expression values, not implicit process environment injection.

The daemon reloads a lane's config when any file the last load read changes, `.env` included, and never stops or restarts a unit because of it. It keeps what each running service was spawned with in memory only, since that holds environment values, and compares it with what the new config would spawn to report per-unit drift; `reload` applies that drift on request. While units run and the file does not load, the lane's routes follow the last config that loaded.

The daemon's HTTPS proxy signs certificates for registered hosts with a per-user CA, devlanes' own or mkcert's (`tls.ca`). SSH transport mirrors registered, non-hidden TCP ports. HTTP-based GUI viewers can be ordinary services; built-in window streaming, relay transport, public DNS resolution and iOS/Android certificate trust are outside v1.

[fixtures/examples](fixtures/examples/README.md) has commented configuration files for these contracts.

## Observability

How devlanes reports what it does today, how OpenTelemetry tracing could be added on top of that, and how to benchmark the CLI and daemon. Only the first section is implemented. devlanes sends nothing anywhere; anonymous usage telemetry is not planned and would be a separate decision.

### Current instrumentation

Both processes use `tracing`. Targets are crate paths (`devlanes_client`, `devlanes_daemon::lifecycle`, `devlanes_infra_compose::cli`), so `RUST_LOG`-style filters select them.

Spans in the daemon:

| Span | Where | Fields |
| --- | --- | --- |
| `request` | every request, in `serve_client` | `id`, `method`, `session`, `tap` when verbose |
| `phase` | each start phase (`infra`, `resolve`, `setup`, `tasks`, `services`, `after-services`) | `name`, `lane` |
| `unit` | one service's paced start | `unit`, `lane` |
| `readiness` | a unit's readiness check, a child of `unit` | |

The client has `spawn_daemon` around an auto-spawn, and its events carry `timing` and `elapsed_ms` where a duration matters (connect and hello, spawn until ready, each request).

Outputs:

- `daemon.log`, filtered by `DEVLANES_LOG` (default `info`).
- `-v`/`-vv` on any command. The client prints its own events to stderr. Requests carry `verbose` in their `Context`; the daemon registers a tap for the request (`devlanes_daemon::diagnostics`), and a `tracing` layer sends every record and span close under that request's span back as `diagnostic` events before the response. The tap's filter passes nothing while no tap is active, so idle cost is one atomic load per debug call site.
- The timing table at the end of a `-v` run, built from client `timing` events and the daemon's span closes.

Rules that every new field follows: environment values are never recorded at any level, only variable names (`env_keys`); command lines are recorded for commands devlanes builds (Docker Compose), and only the argument count for a unit's own command, whose arguments can be interpolated secrets. The diagnostics layer also replaces fields named like `secret`, `password`, `token` or `value`.

Spans only follow work across `tokio::spawn` when the spawned future is instrumented. New background work that belongs to a request (a probe, a spawned step) should use `Instrument::instrument(span)` or `.in_current_span()`.

### Adding OpenTelemetry traces

The spans above map one to one onto OpenTelemetry spans through `tracing-opentelemetry`; no call site changes.

Daemon: add a `tracing_opentelemetry::layer()` next to the file and diagnostics layers in `init_logging`, with a tracer from `opentelemetry-otlp`. Build it only when `OTEL_EXPORTER_OTLP_ENDPOINT` (or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) is set, and honour `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` and `OTEL_TRACES_SAMPLER` as the SDK reads them. Use the batch span processor on the daemon's runtime and shut the tracer provider down in the daemon's shutdown path so the last spans are flushed. Resource attributes: `service.name = devlanes-daemon`, `service.version` and the build id. The daemon inherits `OTEL_*` only once they are added to the auto-spawn allowlist in `devlanes-client` (`DAEMON_ENV_PREFIXES`); a daemon installed as a login service needs them in its service definition.

Client: the same layer in `verbose::init`'s subscriber (or a subscriber of its own when `-v` is off), with `service.name = devlanes-cli`. A root span `command` with the subcommand path wraps `commands::dispatch`. The CLI is short-lived, so it must flush before exit with a bounded timeout (200 ms is a reasonable start) to keep commands fast when the collector is slow or down. A simple span processor with an HTTP exporter fits a process that lives for one request better than a batch processor.

Linking client and daemon: add `trace: Option<String>` to `devlanes_proto::Context`, holding a W3C `traceparent` (`00-<trace id>-<span id>-<flags>`). The client injects the current span's context with the `TraceContextPropagator` before `begin_request`. The daemon extracts it in `request_span` and calls `OpenTelemetrySpanExt::set_parent` on the `request` span, so one trace shows `command` (client), `request`, `phase`, `unit` and `readiness`. The field is additive: older daemons ignore it, and a daemon that receives no `trace` starts its own root. `Context` is never logged, and `traceparent` holds no secrets.

Dependency cost: `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp` and `tracing-opentelemetry`. The OTLP exporter brings either `tonic`, `prost` and `hyper` (gRPC) or an HTTP client (`reqwest` or `hyper`) with `prost` (HTTP/protobuf). The daemon already links `hyper` through the proxy, so HTTP/protobuf over the existing `hyper` adds the least. Measure before deciding: `cargo tree -e normal -p devlanes | wc -l` and the release binary size with and without the change. If the cost matters, put it behind a cargo feature (`otel`) that release builds enable, so the runtime switch stays the environment variable.

Redaction does not change: the same fields become span attributes, so the rules in the previous section apply to exported spans. Log records can be exported as span events; keep that off by default, since there are many `debug` records.

### Benchmarks

The end-to-end harness (`devlanes-e2e`) already gives each test an isolated home, repository and port slot and drives the real binary, so benchmarks belong there as tests named `bench_*`, marked `#[ignore = "benchmark"]` and run with `cargo nextest run --profile e2e --run-ignored only -E 'test(/^bench_/)'`.

Scenarios, each repeated (10 runs, then median and p90):

- Cold CLI: `devlanes --version`, which starts no daemon. Process start and argument parsing.
- Auto-spawn: the first `devlanes status` in a fresh home. The client's `daemon spawn until ready` timing separates the spawn from the request.
- Warm request: `devlanes status` with the daemon up, from the `request status` timing.
- Start to ready: `devlanes start` on `fixtures/two-services` and `fixtures/many-services` (40 services), and `docker_`-prefixed variants on `fixtures/compose-pg-redis`. The daemon's `phase` span durations give infra, setup, tasks and services separately.
- Restart of one unit in a running lane.

To read timings without parsing the human table, `-v` could write them as JSON when `DEVLANES_TIMINGS_FILE` is set: one object per row with `label`, `ms` and `source` (`client` or `daemon`). The benchmark tests set it, read the file and append one JSON line per scenario and run to `target/bench/results.jsonl` with the commit, OS and fixture.

In CI, a job on `main` (and on demand for pull requests) runs the benchmarks on one fixed runner type, uploads `results.jsonl` as an artifact and writes a Markdown summary to `GITHUB_STEP_SUMMARY`. Comparing with the latest `main` artifact and flagging a regression above a threshold (say 20 % on a median) can come later; shared runners are noisy, so it should warn rather than fail until the numbers prove stable. Docker scenarios run only on the Linux runners, like the other `docker_` tests.

## Generated files

`crates/config/schema.json` and `crates/config/user-schema.json` are rewritten by `cargo build -p devlanes` whenever the config types change. Commit them together with the change; a test fails when they're stale.

`crates/tui/screens/*.txt` and its `index.md` are every TUI state drawn against a fake daemon, as text with the styles of each cell, so a pull request shows what changed on screen. Any change to what the TUI draws changes them, including changes outside `crates/tui`. Rewrite them with `DEVLANES_BLESS=1 cargo nextest run -p devlanes-tui -E 'test(screens_are_current)'`, check the diff and commit them with the change; the test fails in CI while they're stale. `cargo run -p devlanes-tui --example screens` writes the same files plus gitignored PNG and ANSI versions for looking at the screens.

`crates/mcp-ui/ui/dist/lane.html` is the MCP Apps card, built from the pnpm package in `crates/mcp-ui/ui` and embedded in the binary, so building devlanes never needs Node. After changing anything under `crates/mcp-ui/ui`, run `pnpm install` and `pnpm build` there and commit the file; the `mcp-ui` workflow fails while it's stale ([devlanes-mcp-ui](crates/mcp-ui/README.md)).

`.gitattributes` marks these files, `Cargo.lock`, the `.snap` files and `docs/reference/` with `merge=ours`, so a merge or rebase keeps one side instead of stopping on conflict markers. Git ignores that attribute until the driver exists, so run this once per clone:

```sh
git config merge.ours.driver true
```

After resolving, regenerate before committing: `cargo build` rewrites `Cargo.lock` and the schemas, `DEVLANES_BLESS=1` rewrites the screens and `docs/reference/`, `INSTA_UPDATE=always` rewrites the snapshots, and `pnpm build` in `crates/mcp-ui/ui` rewrites the card. During a rebase "ours" is the branch you rebase onto, so your side of these files only comes back by regenerating them. `CHANGELOG.md` files use Git's built-in `merge=union` instead, because nothing can regenerate them: both sides' lines are kept without conflict markers, so check that entries sit under the right version.

## Change files

A pull request that changes something a devlanes user can notice adds a change file in `.sampo/changesets/`. [Sampo](https://github.com/bruits/sampo) reads these files to choose the next versions and write each crate's `CHANGELOG.md` in the release pull request. Refactors, tests, CI and documentation need none; say so in the pull request description. No check enforces it, so reviewers look for one.

Create a change file with `sampo add` (`cargo install --locked sampo`), without prompts:

```sh
sampo add -p cargo/devlanes -p cargo/devlanes-daemon -b patch -m 'Fixed `devlanes start` leaving services stopped after a config reload.'
```

or write it by hand under any name ending in `.md`:

```markdown
---
cargo/devlanes: patch
cargo/devlanes-daemon: patch
---

Fixed `devlanes start` leaving services stopped after a config reload.
```

Write one change file per feature or fix a user would look for in the changelog, not one per adjustment made during the work. Small tweaks and fixes to the same part of devlanes go in one entry, so these two:

```markdown
Changed the TUI monitor (`M`) to keep each project's shared infra on top and its lanes below in lane order, instead of reordering them by usage.
```

```markdown
Fixed TUI monitor (`M`) graphs drawing history as separate stripes for the time before the monitor was open.
```

belong in one file:

```markdown
Changed the TUI monitor (`M`) to keep each project's shared infra on top and its lanes in lane order, and fixed its graphs drawing separate stripes for the time before it was open.
```

Separate features keep separate files: a new daemon row in the monitor and a rescaled memory graph are two entries. A fix or tweak to something the same pull request adds needs no entry, since users never saw the earlier version. If the work grows during review, edit its change file instead of adding another, and reread all of the pull request's change files before pushing.

### Crates

List the crates whose code changed in a way their users notice, as `cargo/<package name>`. Add `cargo/devlanes` whenever someone running devlanes notices: its changelog is the one people read, and Sampo only writes an entry there when the change file names it. Don't list crates that only depend on a changed one; Sampo bumps them on its own.

### Bump

Until 1.0, use `minor` for a change that breaks something that worked: a removed or renamed command, flag or config key, a config that no longer loads, or a protocol or on-disk state change that needs a daemon restart or a migration. Use `patch` for everything else, new features included. Never use `major`: Sampo applies plain SemVer, so it would release 1.0.0.

### Wording

The first line is the changelog entry, read by someone deciding whether to upgrade. Write one sentence that:

- starts with Added, Changed, Fixed, Deprecated or Removed;
- names the command, flag or config key in backticks;
- says what changes for the user, not how the code changed. Leave out crate, module and type names unless a crate's API is the change.

Keep the line to one clause, about 100 characters, with no `;` chains or lists of effects. If it needs more, the change is two entries or the second part belongs in a body. Two entries from the 0.10.0 changelog, each followed by a shorter version:

```markdown
Changed `devlanes daemon restart` to return as soon as the new daemon answers, listing the lanes that start again.
Changed `devlanes daemon restart` to return once the new daemon is up.
```

```markdown
Fixed `devlanes attach` and the TUI's unit pane on Windows showing a `pty: true` unit's lines with gaps, and missing later resizes, after a resize that came while the unit was printing.
Fixed gaps in the output of `pty: true` units on Windows after a resize.
```

Most change files are that one line. Add a body after a blank line only when the line can't carry what a user needs to act on:

- a breaking change or removal: what stops working and what to use instead;
- a deprecation: what replaces it and when the old form goes away;
- a large feature: the smallest config or command that gets someone started, and a link to the guide in `docs/`.

Keep a body to a few lines and at most one short example. Don't restate the documentation, explain the implementation, or list every option.

```markdown
---
cargo/devlanes: minor
cargo/devlanes-config: minor
---

Removed the `services.<name>.legacy_env` config key.

Configs that still set it fail to load. Move its entries to `services.<name>.env`.
```

Lines that don't belong in a changelog:

- `Improved the daemon's handling of edge cases.` says nothing a user can check.
- `Refactored the lane registry to share state behind a lock.` describes the code, not the effect.
- `Added a powerful new way to manage ports seamlessly across lanes.` is marketing; say what was added.

The `Writing style` rules in `AGENTS.md` apply to change files too. Before committing one, cut every word a user wouldn't miss.

### Highlights

The first TUI opened after an update shows a dialog of release highlights. It lists every breaking change (a `minor` change file, by its first line), then the highlights that change files offer, and links to the release page for the rest. A release with neither shows no dialog, which is the normal case for patch releases.

A change file offers a highlight with a comment at the end:

```markdown
---
cargo/devlanes: patch
cargo/devlanes-update: patch
---

Added `devlanes update`, which installs the latest release.

<!-- highlight
title: Update in place
body: Run `devlanes update`, or press `U` in the TUI, to install the latest release the way you installed devlanes.
-->
```

Sampo copies the comment into `CHANGELOG.md`, and GitHub hides it on the release page. The title is at most 60 characters and the body at most 300, which a test over `.sampo/changesets/` checks; a longer one in a changelog is cut. A body can continue on the next line, indented. The comment adds to the changelog line and does not replace it. Write the title as a few plain words naming what the user can do, and the body as one or two sentences that name the command, flag, key or place in the TUI. Don't repeat the changelog line with fewer words.

Most change files carry no highlight, including almost every fix. Add one only for a new command or workflow a user would try, or a fix for a problem many users hit. A breaking change that has a highlight is listed once, with the highlight's words.

The dialog lists at most five non-breaking highlights across all releases since the last one the user saw, newest release first, and counts the rest. Before adding one, read the highlights already in `.sampo/changesets/` and add yours only if it matters at least as much as those. When nothing in the pull request qualifies, add none.

### Versions

Don't edit versions or `CHANGELOG.md` files by hand; the release pull request does. Crates have their own versions, and `devlanes`, `devlanes-hosts-helper` and `devlanes-gui` carry the release version (`packaging/README.md`). A new crate needs an entry in `packages.linked` of `.sampo/config.toml`, so its changes reach the `devlanes` version. Code that reports the devlanes version uses `devlanes_version::VERSION`, not its own crate's `CARGO_PKG_VERSION`.

## Releases

Merging the release pull request, which `.github/workflows/sampo.yml` keeps up to date on every push to `main`, releases everything; `packaging/README.md` describes the workflow and the Homebrew and npm packages.

### Using a merged change before it is released

Every commit on `main` that passes CI is built and published as the `next` build (`.github/workflows/next.yml`). Once your pull request is merged and its checks on `main` are green, install that build to use the change without waiting for a release. On macOS and Linux:

```sh
curl -fsSL https://devlanes.run/install.sh | sh -s -- --channel next
```

On Windows 11:

```powershell
iex "& {$(irm https://devlanes.run/install.ps1)} -Channel next"
```

`devlanes --version` then reads `X.Y.Z-next.N`, where N counts the commits since the last release, and `devlanes update` moves the install to the latest build of `main`. Only the install scripts carry `next` builds: npm, PyPI and Homebrew get releases only. [Update and uninstall](https://devlanes.run/docs/how-to/update/) says how to go back to releases.

### Symbolicating a report

Release and `next` binaries are stripped, so a stack from a user's machine (macOS `sample`, `perf`, a Windows crash dump) shows raw addresses. Each release, and the `next` prerelease, has a debug archive per target, `devlanes-VERSION-TARGET-debug.tar.gz`, built in the same job as the shipped binary, with function names and file:line tables. It holds `devlanes.dSYM`, `devlanes-hosts.dSYM` and `devlanes-gui.dSYM` on macOS, `devlanes.debug`, `devlanes-hosts.debug` and `devlanes-gui.debug` on Linux, and `devlanes.pdb` and `devlanes_gui.pdb` (Cargo writes `_` there) on Windows. Take the version and target from the report's `devlanes --version` and platform, then:

```sh
gh release download vX.Y.Z --repo devlanes-run/devlanes -p 'devlanes-X.Y.Z-aarch64-apple-darwin-debug.tar.gz'
tar -xzf devlanes-X.Y.Z-aarch64-apple-darwin-debug.tar.gz
```

For a `next` build, download from the `next` release instead; it only holds the latest build, so it matches only if the user runs that one. `SHA256SUMS` lists the debug archives, and `gh attestation verify` works on them as on the other archives.

On macOS, check that the dSYM is the one for the binary: `dwarfdump --uuid devlanes.dSYM` must print the UUID that the report's Binary Images section (or `dwarfdump --uuid "$(command -v devlanes)"` on the user's machine) shows. `sample` prints a frame as `??? (in devlanes)  load address 0x... + 0x1d3d104  [0x...]`. Pass the load address and the absolute address, or only the offset:

```sh
atos -o devlanes.dSYM -arch arm64 -l 0x104a14000 0x106751104
atos -o devlanes.dSYM -arch arm64 -offset 0x1d3d104
```

Use `-arch x86_64` for Intel Macs, and add `-i` to see inlined frames.

On Linux, `readelf -n devlanes.debug` must show the same `Build ID` as `readelf -n` on the user's `devlanes`. The binaries are position-independent, so pass the address minus the start of the first `devlanes` mapping in `/proc/PID/maps`:

```sh
addr2line -e devlanes.debug -f -C -i -p 0x1d3d104
llvm-symbolizer --obj=devlanes.debug 0x1d3d104
```

Putting `devlanes.debug` next to `devlanes` lets gdb and perf find it on their own through the binary's debug link.

On Windows, put `devlanes.pdb` next to `devlanes.exe` (or add its directory to the symbol path with `.sympath+` and `.reload` in WinDbg); WinDbg, Visual Studio and `!analyze -v` then resolve the dump's frames, and `ln devlanes+0x1d3d104` names one offset. Without a debugger, llvm-symbolizer reads the `devlanes.pdb` next to the executable and takes the offset added to the image base (`ImageBase` in `llvm-readobj --file-headers devlanes.exe`, 0x140000000 for the x64 build):

```sh
llvm-symbolizer --obj=devlanes.exe 0x141d3d104
```

## License

Contributions are licensed under Apache-2.0, the license of this repository.
