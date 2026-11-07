//! Plan M7 acceptance against `fixtures/compose-pg-redis`: a Postgres per lane, one
//! shared Redis, and `lane clean` removing only its own lane's containers and data.

use std::{path::Path, time::Duration};

use devlanes_e2e::{Lab, Output, Result};

/// Shared Compose project, named in the fixture's `infra.project_name`.
const SHARED: &str = "devlanes-fixture-pg-redis";
/// Per-lane Compose projects, `<project>-lane<N>`.
const LANE_ZERO: &str = "fixture-pg-redis-lane0";
const LANE_ONE: &str = "fixture-pg-redis-lane1";

/// `docker <args>`, required to succeed; returns trimmed stdout.
fn docker(lab: &Lab, args: &[&str]) -> Result<String> {
    let output: Output = lab.docker_cli(args)?;
    output.expect_code(0)?;
    Ok(output.stdout.trim().to_owned())
}

/// Runs SQL in the `app` database of a lane's own Postgres.
fn sql(lab: &Lab, project: &str, statement: &str) -> Result<String> {
    docker(
        lab,
        &[
            "compose",
            "-p",
            project,
            "exec",
            "-T",
            "postgres",
            "psql",
            "-U",
            "postgres",
            "-d",
            "app",
            "-v",
            "ON_ERROR_STOP=1",
            "-tAc",
            statement,
        ],
    )
}

/// Container IDs of a Compose project, stopped ones included.
fn containers(lab: &Lab, project: &str) -> Result<Vec<String>> {
    let ids = docker(
        lab,
        &[
            "ps",
            "-aq",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ],
    )?;
    Ok(ids.lines().map(str::to_owned).collect())
}

/// Named volumes of a Compose project.
fn volumes(lab: &Lab, project: &str) -> Result<Vec<String>> {
    let names = docker(
        lab,
        &[
            "volume",
            "ls",
            "-q",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ],
    )?;
    Ok(names.lines().map(str::to_owned).collect())
}

/// `start` in a worktree, with room for image pulls.
fn start(lab: &Lab, cwd: &Path) -> Result<()> {
    start_units(lab, cwd, &[])
}

/// `start <units>` in a worktree, with room for image pulls.
fn start_units(lab: &Lab, cwd: &Path, units: &[&str]) -> Result<()> {
    let mut args = vec!["--timeout", "5m", "start"];
    args.extend(units);
    let command = lab.command(cwd, &args)?;
    let output = lab.run(command, Duration::from_secs(360))?;
    let started = output.expect_code(0)?.json()?;
    assert_eq!(
        started["ready"],
        true,
        "start in {}: {output}",
        cwd.display()
    );
    Ok(())
}

/// Two lanes each get their own Postgres and data; `lane clean 1` removes lane 1's
/// containers and volume and leaves lane 0's data and the shared Redis running.
#[test]
#[ignore = "needs docker"]
fn docker_lanes_get_their_own_postgres_and_clean_drops_only_theirs() -> Result<()> {
    let lab = Lab::docker_projects(
        "compose-pg-redis",
        &[SHARED, LANE_ZERO, LANE_ONE],
        Duration::from_secs(900),
    )?;
    let feature = lab.add_worktree("feat/pg")?;
    start(&lab, lab.repo())?;
    start(&lab, &feature)?;

    // The setup entry created the table in each lane's own database.
    sql(&lab, LANE_ZERO, "INSERT INTO notes VALUES ('lane-zero')")?;
    sql(&lab, LANE_ONE, "INSERT INTO notes VALUES ('lane-one')")?;
    assert_eq!(sql(&lab, LANE_ZERO, "SELECT body FROM notes")?, "lane-zero");
    assert_eq!(sql(&lab, LANE_ONE, "SELECT body FROM notes")?, "lane-one");

    // `infra logs` reads Docker's logs of a lane's own service and of a shared one.
    for service in ["postgres", "redis"] {
        let page = lab.json(
            &feature,
            &["infra", "logs", service, "--tail", "5", "--json"],
        )?;
        let items = page["items"].as_array().cloned().unwrap_or_default();
        assert!(
            !items.is_empty() && items.len() <= 5,
            "{service} logs: {page}"
        );
        assert!(
            items
                .iter()
                .all(|line| line["unit"] == service && line["timestamp_ms"].as_u64() > Some(0)),
            "{service} logs: {page}"
        );
    }

    let zero_containers = containers(&lab, LANE_ZERO)?;
    let one_containers = containers(&lab, LANE_ONE)?;
    assert_eq!(zero_containers.len(), 1, "one Postgres in lane 0");
    assert_eq!(one_containers.len(), 1, "one Postgres in lane 1");
    assert_ne!(zero_containers, one_containers);
    assert!(!volumes(&lab, LANE_ZERO)?.is_empty(), "lane 0 has a volume");
    assert!(!volumes(&lab, LANE_ONE)?.is_empty(), "lane 1 has a volume");

    // Redis is shared: one container outside the lane projects, written by lane 0.
    let redis = containers(&lab, SHARED)?;
    assert_eq!(
        redis.len(),
        1,
        "only Redis runs in the shared project: {redis:?}"
    );
    let set = docker(
        &lab,
        &[
            "compose",
            "-p",
            SHARED,
            "exec",
            "-T",
            "redis",
            "redis-cli",
            "SET",
            "shared-key",
            "kept",
        ],
    )?;
    assert_eq!(set, "OK");

    let cleaned = lab.devlanes(lab.repo(), &["lane", "clean", "1", "--yes"])?;
    cleaned.expect_code(0)?;

    assert!(
        containers(&lab, LANE_ONE)?.is_empty(),
        "lane 1's Postgres container is gone"
    );
    assert!(
        volumes(&lab, LANE_ONE)?.is_empty(),
        "lane 1's volume is gone"
    );
    assert_eq!(containers(&lab, LANE_ZERO)?, zero_containers);
    assert!(
        !volumes(&lab, LANE_ZERO)?.is_empty(),
        "lane 0 keeps its volume"
    );
    assert_eq!(
        sql(&lab, LANE_ZERO, "SELECT body FROM notes")?,
        "lane-zero",
        "lane 0 keeps its data"
    );
    assert_eq!(containers(&lab, SHARED)?, redis, "Redis was not recreated");
    let running = docker(&lab, &["inspect", "-f", "{{.State.Running}}", &redis[0]])?;
    assert_eq!(running, "true", "shared Redis keeps running");
    let kept = docker(
        &lab,
        &[
            "compose",
            "-p",
            SHARED,
            "exec",
            "-T",
            "redis",
            "redis-cli",
            "GET",
            "shared-key",
        ],
    )?;
    assert_eq!(kept, "kept");
    Ok(())
}

/// Setup entries backed by the lane's Postgres and the shared Redis, each
/// appending its name to `{runs}`, and two services.
const BACKED: &str = r#"project: fixture-pg-redis
infra:
  compose: compose.yaml
  project_name: devlanes-fixture-pg-redis
  per_lane: [postgres]
  setup:
    schema: { run: "echo schema >> '{runs}'", container: postgres }
    cache: { run: "echo cache >> '{runs}'", container: redis }
services:
  api:
    run: [fixture-server, --log-line, api-ready]
    ready: { http: { path: /healthz } }
  web:
    run: [fixture-server, --log-line, web-ready]
    ready: { http: { path: /healthz } }
"#;

/// Entries run on their own record their container's start time, so a later
/// start of one service runs neither again; it reads both containers' start
/// times from Docker.
#[test]
#[ignore = "needs docker"]
fn docker_setup_entries_run_by_hand_stay_done() -> Result<()> {
    let lab = Lab::docker_projects(
        "compose-pg-redis",
        &[SHARED, LANE_ZERO],
        Duration::from_secs(600),
    )?;
    let runs = lab.home().join("setup-runs");
    lab.write(
        "devlanes.yaml",
        &BACKED.replace("{runs}", &runs.to_string_lossy()),
    )?;
    let ran = || -> Vec<String> {
        let mut lines: Vec<String> = std::fs::read_to_string(&runs)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        lines.sort();
        lines
    };
    start(&lab, lab.repo())?;
    assert_eq!(ran(), ["cache", "schema"]);

    for entry in ["infra.setup.schema", "infra.setup.cache"] {
        lab.devlanes(lab.repo(), &["run", entry])?.expect_code(0)?;
    }
    assert_eq!(ran(), ["cache", "cache", "schema", "schema"]);

    lab.devlanes(lab.repo(), &["stop", "web"])?.expect_code(0)?;
    start_units(&lab, lab.repo(), &["web"])?;
    assert_eq!(
        ran(),
        ["cache", "cache", "schema", "schema"],
        "the start found both entries done"
    );
    Ok(())
}

/// A setup entry backed by the shared Redis that appends Redis's health to
/// `{runs}`, and a service that reads Redis's published port.
const RESTARTED: &str = r#"project: fixture-pg-redis
infra:
  compose: compose.yaml
  project_name: devlanes-fixture-pg-redis
  per_lane: [postgres]
  setup:
    cache:
      run: "docker inspect -f '{{.State.Health.Status}}' devlanes-fixture-pg-redis-redis-1 >> '{runs}'"
      container: redis
services:
  api:
    run: [fixture-server, --log-line, api-ready]
    env:
      REDIS_URL: redis://127.0.0.1:${{ infra.redis.port }}/0
    ready: { http: { path: /healthz } }
  web:
    run: [fixture-server, --log-line, web-ready]
    ready: { http: { path: /healthz } }
"#;

/// Redis's published port in `status`.
fn status_redis_port(lab: &Lab) -> Result<Option<u64>> {
    Ok(lab.lanes(lab.repo())?.first().and_then(|lane| {
        lane["containers"]
            .as_array()?
            .iter()
            .find(|container| container["name"] == "redis")?["ports"]["6379"]
            .as_u64()
    }))
}

/// After `docker restart` of the shared Redis, which gets a new ephemeral port,
/// the daemon hears of it from the engine and `status` shows the new port; the
/// next start reruns the Redis-backed setup entry once Redis is healthy, says
/// so in `ran`, and a restarted service reads the new port.
#[test]
#[ignore = "needs docker"]
fn docker_restarted_shared_redis_is_settled_and_gets_its_new_port() -> Result<()> {
    let lab = Lab::docker_projects(
        "compose-pg-redis",
        &[SHARED, LANE_ZERO],
        Duration::from_secs(600),
    )?;
    let runs = lab.home().join("setup-runs");
    // Forward slashes, since a Windows path's backslashes are escapes in a
    // double-quoted YAML string.
    lab.write(
        "devlanes.yaml",
        &RESTARTED.replace("{runs}", &runs.to_string_lossy().replace('\\', "/")),
    )?;
    let last_run = || {
        std::fs::read_to_string(&runs)
            .unwrap_or_default()
            .lines()
            .last()
            .map(str::to_owned)
    };
    start(&lab, lab.repo())?;
    assert_eq!(last_run().as_deref(), Some("healthy"));

    let redis = "devlanes-fixture-pg-redis-redis-1";
    docker(&lab, &["restart", redis])?;
    let published = docker(&lab, &["port", redis, "6379/tcp"])?;
    let port: u64 = published
        .lines()
        .next()
        .and_then(|line| line.rsplit(':').next())
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("docker port: {published}"));
    // The engine's events reach the daemon, which reads the port again.
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while status_redis_port(&lab)? != Some(port) {
        assert!(
            std::time::Instant::now() < deadline,
            "status keeps the old port: {:?}",
            status_redis_port(&lab)?
        );
        std::thread::sleep(Duration::from_millis(250));
    }

    let command = lab.command(lab.repo(), &["--timeout", "2m", "start", "web"])?;
    let output = lab.run(command, Duration::from_secs(180))?;
    let started = output.expect_code(0)?.json()?;
    assert_eq!(
        started["ran"],
        serde_json::json!(["fixture-pg-redis@0:setup.cache"]),
        "{output}"
    );
    assert_eq!(last_run().as_deref(), Some("healthy"));

    // `env show` of a running unit is the env it runs with, so `api` gets the
    // new port only from its restart.
    let command = lab.command(lab.repo(), &["--timeout", "2m", "restart", "api"])?;
    lab.run(command, Duration::from_secs(180))?.expect_code(0)?;
    let env = lab.json(lab.repo(), &["env", "show", "api"])?;
    assert_eq!(
        env["env"]["REDIS_URL"],
        format!("redis://127.0.0.1:{port}/0"),
        "{env}"
    );
    lab.devlanes(lab.repo(), &["stop"])?.expect_code(0)?;
    Ok(())
}

/// `State.Paused` of each container.
fn paused(lab: &Lab, ids: &[String]) -> Result<Vec<String>> {
    ids.iter()
        .map(|id| docker(lab, &["inspect", "-f", "{{.State.Paused}}", id]))
        .collect()
}

/// Freezing a lane pauses its own Postgres and leaves the shared Redis running;
/// thawing unpauses it and the data is reachable again.
#[test]
#[ignore = "needs docker"]
fn docker_freeze_pauses_only_the_lanes_own_containers() -> Result<()> {
    let lab = Lab::docker_projects(
        "compose-pg-redis",
        &[SHARED, LANE_ZERO],
        Duration::from_secs(600),
    )?;
    start(&lab, lab.repo())?;
    let own = containers(&lab, LANE_ZERO)?;
    let shared = containers(&lab, SHARED)?;

    lab.json(lab.repo(), &["debug", "freeze"])?;
    assert_eq!(
        paused(&lab, &own)?,
        ["true"],
        "the lane's Postgres is paused"
    );
    assert_eq!(
        paused(&lab, &shared)?,
        ["false"],
        "shared Redis keeps running"
    );

    lab.json(lab.repo(), &["debug", "thaw"])?;
    assert_eq!(paused(&lab, &own)?, ["false"]);
    sql(&lab, LANE_ZERO, "INSERT INTO notes VALUES ('after-thaw')")?;
    assert_eq!(
        sql(&lab, LANE_ZERO, "SELECT body FROM notes")?,
        "after-thaw"
    );
    lab.devlanes(lab.repo(), &["stop"])?.expect_code(0)?;
    Ok(())
}
