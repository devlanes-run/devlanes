//! `lane.clean.plan` (`lane clean --dry-run`): what a clean would do, with
//! nothing stopped, run, removed or deleted, and no lane given to a worktree.

use std::collections::BTreeSet;

use devlanes_daemon::integrations::InfraError;
use devlanes_proto::{LaneSummary, Method, Page};
use serde_json::{Value, json};

use crate::{LIMIT, Setup, cx, harness, repo, temp, worktree};

const CONFIG: &str = r#"project: tidy
infra:
  containers:
    db: { image: "postgres:16", ports: { pg: 5432 }, per_lane: true }
  teardown:
    database: { run: "dropdb tidy_${{ lane.index }}", container: db }
tasks:
  drop-cache: { run: [redis-cli, FLUSHDB], when: lane.clean }
  seed: { run: "echo seed" }
services:
  api: { run: sleep, ready: started }
"#;

/// The lanes `status` lists from the repository.
async fn lanes(h: &crate::Harness, cwd: &std::path::Path) -> Vec<LaneSummary> {
    let value = h
        .call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status");
    serde_json::from_value::<Page<LaneSummary>>(value)
        .expect("page")
        .items
}

#[tokio::test(flavor = "multi_thread")]
/// The plan lists the running unit, the `lane.clean` task, the teardown entry,
/// the lane's containers and volumes, its data folder and logs with their sizes
/// and its ports, in the documented shape, without `yes`; afterwards the unit
/// still runs, no task or entry ran, no container was removed and the data
/// folder is still there.
async fn a_plan_reports_the_clean_and_changes_nothing() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                infra: true,
                ..Setup::default()
            },
        )
        .await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let journal = h.runner.journal.entries();
        let calls = h.infra.calls.entries();

        let first = h
            .call(Method::LaneCleanPlan, json!({}), cx(&main))
            .await
            .expect("plan");
        let data_dir = std::path::PathBuf::from(first["data_dir"]["path"].as_str().expect("path"));
        std::fs::write(data_dir.join("dump.sql"), "0123456789").expect("data file");
        let plan = h
            .call(Method::LaneCleanPlan, json!({"lane": "tidy@0"}), cx(&tmp.path))
            .await
            .expect("plan by reference");

        let keys: BTreeSet<&str> = plan
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from([
                "lane",
                "project",
                "index",
                "branch",
                "path",
                "worktree",
                "running",
                "tasks",
                "teardown",
                "hooks_skipped",
                "containers",
                "compose_project",
                "volumes",
                "infra_error",
                "data_dir",
                "logs_bytes",
                "ports",
            ]),
            "{plan}"
        );
        assert_eq!(plan["lane"], "tidy@0");
        assert_eq!(plan["project"], "tidy");
        assert_eq!(plan["index"], 0);
        assert_eq!(plan["branch"], "main");
        assert_eq!(plan["worktree"], "present");
        assert_eq!(
            plan["running"],
            json!([{"unit": "tidy@0:api", "actor": {"kind": "human"}}])
        );
        assert_eq!(
            plan["tasks"],
            json!([{"name": "drop-cache", "run": "redis-cli FLUSHDB"}]),
            "only the lane.clean task"
        );
        assert_eq!(
            plan["teardown"],
            json!([{"name": "database", "run": "dropdb tidy_${{ lane.index }}", "container": "db"}]),
            "as the config writes it"
        );
        assert_eq!(plan["hooks_skipped"], Value::Null);
        assert_eq!(
            plan["containers"],
            json!([{"name": "tidy-lane0-db-1", "kind": "compose"}])
        );
        assert_eq!(plan["compose_project"], "tidy-lane0");
        assert_eq!(
            plan["volumes"],
            json!([{"name": "tidy-lane0_pgdata", "bytes": 4096}])
        );
        assert_eq!(plan["infra_error"], Value::Null);
        assert!(
            plan["data_dir"]["bytes"].as_u64().is_some_and(|bytes| bytes >= 10),
            "{plan}"
        );
        assert!(plan["logs_bytes"].is_u64(), "{plan}");
        assert!(plan["ports"]["start"].is_u64(), "{plan}");
        assert!(plan["ports"]["size"].as_u64().is_some_and(|size| size > 0), "{plan}");

        assert_eq!(h.runner.journal.entries(), journal, "nothing ran");
        assert_eq!(h.infra.calls.entries(), calls, "no container changed");
        assert!(data_dir.join("dump.sql").is_file(), "the data folder stays");
        let after = lanes(&h, &main).await;
        assert_eq!(after.len(), 1);
        let api = after[0].units.iter().find(|unit| unit.name == "api").expect("api");
        assert_eq!(api.state, "ready", "the unit still runs");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A plan lists the `lane.clean` tasks and teardown without asking any
/// contributor, such as a secret store, and without creating the lane's data
/// folder.
async fn a_plan_asks_no_contributor_and_creates_nothing() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let journal = crate::PlanJournal::default();
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                infra: true,
                contributors: vec![std::sync::Arc::new(journal.clone())],
                ..Setup::default()
            },
        )
        .await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let first = h
            .call(Method::LaneCleanPlan, json!({}), cx(&main))
            .await
            .expect("plan");
        let data_dir = std::path::PathBuf::from(first["data_dir"]["path"].as_str().expect("path"));
        std::fs::remove_dir_all(&data_dir).expect("remove the data folder");
        journal.planned.0.lock().expect("lock").clear();
        let plan = h
            .call(Method::LaneCleanPlan, json!({}), cx(&main))
            .await
            .expect("plan");
        assert_eq!(plan["tasks"][0]["name"], "drop-cache", "{plan}");
        assert_eq!(plan["teardown"][0]["name"], "database", "{plan}");
        assert_eq!(plan["teardown"][0]["container"], "db", "{plan}");
        assert_eq!(
            journal.planned.entries(),
            Vec::<String>::new(),
            "no contributor ran"
        );
        assert!(!data_dir.exists(), "the plan created the data folder");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// When Docker cannot list the containers, the plan still answers: a project
/// that declares containers says they are unknown, one that declares none
/// says nothing about them.
async fn a_plan_without_docker_still_answers() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let with_infra = repo(&tmp.path, "r", CONFIG);
        let plain = repo(
            &tmp.path,
            "p",
            "project: plain\nservices:\n  api: { run: sleep, ready: started }\n",
        );
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                infra: true,
                ..Setup::default()
            },
        )
        .await;
        *h.infra.fail_resources.lock().expect("lock") = Some(InfraError::new(
            "docker_unavailable",
            "no Docker engine is reachable",
        ));
        for cwd in [&with_infra, &plain] {
            h.call(Method::Start, json!({}), cx(cwd))
                .await
                .expect("start");
        }
        let plan = h
            .call(Method::LaneCleanPlan, json!({}), cx(&with_infra))
            .await
            .expect("plan");
        assert_eq!(
            plan["infra_error"], "no Docker engine is reachable",
            "{plan}"
        );
        assert_eq!(plan["containers"], json!([]), "{plan}");
        assert_eq!(plan["tasks"][0]["name"], "drop-cache", "{plan}");
        let plan = h
            .call(Method::LaneCleanPlan, json!({}), cx(&plain))
            .await
            .expect("plan");
        assert_eq!(plan["infra_error"], Value::Null, "{plan}");
        assert!(plan["data_dir"]["bytes"].is_u64(), "{plan}");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A plan from a worktree devlanes never ran in does not make it a lane, as a
/// plain command there would: it reports no record, runs no hooks and the
/// worktree stays out of `status`.
async fn a_plan_does_not_give_a_worktree_a_lane() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let other = tmp.path.join("r-feat");
        worktree(&main, &other, "feat/x");
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                infra: true,
                ..Setup::default()
            },
        )
        .await;
        assert_eq!(lanes(&h, &main).await.len(), 1);
        let plan = h
            .call(Method::LaneCleanPlan, json!({}), cx(&other))
            .await
            .expect("plan");
        assert_eq!(plan["index"], Value::Null, "{plan}");
        assert_eq!(plan["hooks_skipped"], "unassigned", "{plan}");
        assert_eq!(plan["data_dir"], Value::Null, "{plan}");
        assert_eq!(plan["ports"], Value::Null, "{plan}");
        let references: Vec<String> = lanes(&h, &main)
            .await
            .into_iter()
            .map(|lane| lane.reference)
            .collect();
        assert_eq!(references, ["tidy@0"], "the worktree is still no lane");
        h.close().await;
    })
    .await
    .expect("timeout");
}
