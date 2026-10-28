//! Timing of a large lane resolve. Ignored by default; run it in release mode:
//! `cargo test --release -p devlanes-resolve resolve_timing -- --ignored --nocapture`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    time::{Duration, Instant},
};

use devlanes_resolve::{ResolveInput, resolve_lane};

use crate::support::*;

/// Services in the synthetic config, close to the project the target was measured on.
const SERVICES: usize = 45;
/// `global_env` entries in the synthetic config.
const GLOBALS: usize = 60;
/// Resolves per measurement.
const ROUNDS: usize = 5;

/// A config with dashed service names, `global_env` entries that read service
/// ports, `lane`, `infra` and `env`, and two that read `self`.
fn config() -> String {
    let mut yaml = String::from(
        "project: acme\ninfra:\n  containers:\n    postgres:\n      image: postgres:16\n      ports: { default: 5432 }\nglobal_env:\n",
    );
    for i in 0..GLOBALS {
        let other = i % SERVICES;
        let value = match i % 5 {
            0 => format!("http://127.0.0.1:${{{{ services.svc_{other:02}.port }}}}"),
            1 => format!("${{{{ services['svc-{other:02}'].url }}}}/api"),
            2 => "${{ lane.name }}-${{ lane.index }}".to_owned(),
            3 => "postgres://localhost:${{ infra.postgres.port }}/acme".to_owned(),
            _ => "${{ env.HOME }}/cache".to_owned(),
        };
        writeln!(yaml, "  G_{i:02}: \"{value}\"").expect("write");
    }
    yaml.push_str("  SELF_NAME: \"${{ self.name }}\"\n");
    yaml.push_str("  SELF_REPLICA: \"${{ replica.index }}\"\n");
    yaml.push_str("services:\n");
    for i in 0..SERVICES {
        let next = (i + 1) % SERVICES;
        writeln!(yaml, "  svc-{i:02}:").expect("write");
        writeln!(yaml, "    run: server --port ${{{{ self.port }}}}").expect("write");
        writeln!(yaml, "    env:").expect("write");
        writeln!(
            yaml,
            "      UPSTREAM: \"http://127.0.0.1:${{{{ services.svc_{next:02}.port }}}}\""
        )
        .expect("write");
        writeln!(yaml, "      OWN: \"${{{{ self.url }}}}\"").expect("write");
    }
    yaml
}

/// A client environment about the size of a developer shell's.
fn client() -> BTreeMap<String, String> {
    let mut env = client_env();
    for i in 0..80 {
        env.insert(format!("SHELL_VAR_{i}"), "x".repeat(40));
    }
    env
}

/// The first and the fastest of [`ROUNDS`] resolves, planning `only` or every unit.
/// The first one compiles expressions no earlier resolve in the process has seen.
async fn timings(only: Option<&BTreeSet<String>>) -> (Duration, Duration) {
    let config = parse(&config());
    let facts = facts(&config.project, 1, Some("main"));
    let (block, offsets) = allocation(&config, 21_000, &[]);
    let infra = infra();
    let client = client();
    let mut first = None;
    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        let mut input = ResolveInput::new(&config, &facts, block, &offsets);
        input.infra = &infra;
        input.client_env = &client;
        input.only = only;
        let started = Instant::now();
        let lane = resolve_lane(input).await.expect("resolves");
        let elapsed = started.elapsed();
        first.get_or_insert(elapsed);
        best = best.min(elapsed);
        assert!(lane.errors.is_empty(), "{:?}", lane.errors);
    }
    (first.unwrap_or(best), best)
}

#[tokio::test]
#[ignore = "timing; run in release mode"]
/// Prints how long a whole-lane resolve and a single-unit plan take.
async fn resolve_timing() {
    let (first, lane) = timings(None).await;
    let only = BTreeSet::from(["services.svc-07".to_owned()]);
    let (_, single) = timings(Some(&only)).await;
    println!("whole lane ({SERVICES} services, {GLOBALS} global_env): {lane:?}, first {first:?}");
    println!("single unit: {single:?}");
}
