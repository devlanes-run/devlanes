# devlanes-infra-compose

Runs a project's Docker Compose infra for devlanes: one shared project from the primary worktree, plus a `<project>-lane<N>` project per lane for `infra.per_lane` services. The daemon calls it through the `docker compose` CLI and adapts it to the core `Infra` trait; it does not depend on `devlanes-core`.

## Usage

```rust
use std::collections::{BTreeMap, BTreeSet};

use devlanes_infra_compose::{
    Compose, ComposeCli, Error, LaneInfra, LanePorts, PortBlock, SharedInfra, UpOptions, setup_env,
};

async fn start_lane() -> Result<(), Error> {
    let compose = Compose::new(ComposeCli::new());
    let shared = SharedInfra {
        project_name: Some("acme".into()),
        primary_dir: "/work/acme".into(),
        files: vec!["docker-compose.yml".into()],
        env_file: Some(".env".into()),
        profiles: Vec::new(),
        exclude: BTreeSet::from(["caddy".into()]),
        per_lane: BTreeSet::from(["mailpit".into()]),
        env: BTreeMap::new(),
    };
    let report = compose.shared_up(&shared, UpOptions::default()).await?;
    let postgres = report.endpoints["postgres"].ports[&5432];

    let lane = LaneInfra {
        project: "acme".into(),
        lane: 2,
        lane_dir: "/work/acme-lanes/feature".into(),
        files: shared.files.clone(),
        env_file: Some(".env".into()),
        profiles: Vec::new(),
        exclude: shared.exclude.clone(),
        per_lane: shared.per_lane.clone(),
        env: BTreeMap::new(),
        // The daemon passes `compose` in the lane data folder.
        state_dir: "/home/me/.devlanes/data/acme/7/compose".into(),
    };
    let block = PortBlock::new(21000, 10)?;
    let mut ports = LanePorts::new();
    for (request, host) in compose.lane_port_requests(&lane).await?.iter().zip(block.first..) {
        ports.entry(request.service.clone()).or_default().insert(request.target, host);
    }
    let lane_report = compose.lane_up(&lane, &ports, block, UpOptions::default()).await?;

    // Variables for infra.setup entries that call `docker compose exec`.
    let (project, _) = compose.shared_project(&shared).await?;
    let env = setup_env(&project, Some(&lane_report.project))?;
    println!("postgres on {postgres}, setup env {env:?}");
    Ok(())
}
```

In the daemon, host ports come from the persisted offsets (`PortRequest::key()` gives a stable name such as `mailpit/8025`).

## API

- `Compose`: `shared_up`, `shared_status`, `shared_health` / `lane_health` (one `ps` for progress polls), `shared_stop`, `lane_port_requests`, `lane_up`, `lane_status`, `lane_stop`, `lane_down` (lane clean, removes named volumes).
- `Compose::lane_pause` / `lane_unpause`: pause or unpause a frozen lane's `<project>-lane<N>` containers and return how many changed; the shared project is never touched.
- `SharedReport` / `LaneReport`: `endpoints` (first published port and host ports keyed by container port) and `health` per service.
- `ComposeModel`: the parsed output of `docker compose config --format json`.
- `render_override`: the per-lane override file as text, for tests and diagnostics.
- `setup_env`: `COMPOSE_PROJECT_NAME`, `COMPOSE_FILE`, `COMPOSE_PROFILES` and `DEVLANES_COMPOSE_LANE_PROJECT` for setup and teardown entries.

## Config example

```yaml
infra:
  # Compose file, or list of files; its services are shared by every lane.
  compose: docker-compose.yml
  # Compose services devlanes never starts.
  exclude: [caddy]
  # Compose services started once per lane instead of shared.
  per_lane: [mailpit]
  # Commands run once per lane after its containers are healthy.
  setup:
    database: docker compose exec -T postgres createdb -U postgres acme_${{ lane.name }}
```

Postgres runs once in the primary worktree's Compose project, and each lane gets its own `acme-lane<N>` project with a `mailpit` on ports from that lane's block. `docker compose exec` in setup reaches the shared project through the variables from `setup_env`.

## Behaviour

Per-lane services need Compose 2.24.4 or later. The generated override uses `!override` for ports and `!reset` for `container_name`; `lane_up` fails with an upgrade message on older releases.

Lane projects start with `--no-deps`, so a lane never starts its own copy of a shared service. A shared service that depends on an excluded or per-lane service is an error.

A shared service whose `container_name` already exists is adopted when the container belongs to the same Compose project. Otherwise `shared_up` fails and names the command that frees the name.

Docker marks an unpaused container with a healthcheck `unhealthy` until its next check runs, and `up --wait` fails on that. `lane_up` unpauses the lane's paused containers first. A service whose containers were healthy before the pause, with a failing streak below the healthcheck's retries, comes up without `--wait`, unless Compose replaces its container. One paused before its first passing check is waited for until its next check, up to the wait timeout.

`Compose` keeps the parsed `config` output per invocation and reuses it until a compose file, a `.env` file or a file the compose files name changes size or modification time, or an interpolation variable changes. The named files come from parsing the compose files: `include` (with the included files and their `env_file` or default `.env`), `extends.file`, and each service's `env_file` and `label_file`, followed four levels deep. A project is read again on every call when a compose file does not parse, a named path uses `$` interpolation, starts with `~` or is remote, or the nesting goes deeper. `shared_up` and `lane_up` run `ps` first and leave out of `up` every service whose containers already run healthy with the config hash `docker compose config --hash` prints now; when that is every service, `up` does not run. A tag pulled again since the container was created does not count as a change. Remove those containers with `docker compose -p <project> rm -sf <service>`, which needs only the project name, and the next `shared_up` or `lane_up` creates them from the pulled image.

Operations on one Compose project run one at a time, except `lane_unpause`, so a thaw never waits behind a long `up`. `ps`, `stop` and `down` need only the project name, so they still work after a worktree is removed.

## License

Apache-2.0
