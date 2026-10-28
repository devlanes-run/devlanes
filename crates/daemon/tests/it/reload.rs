//! Drift between a running lane and its edited project file, `reload`, which applies
//! only the difference, and `restart` without units, which restarts the whole lane.

use devlanes_proto::{LaneSummary, Method, Page, StartResult, UnitDrift};
use serde_json::json;

use crate::{LIMIT, Setup, cx, harness, repo, temp};

const BEFORE: &str = "project: demo\nservices:\n  web: { run: \"echo one; sleep\", ready: started }\n  api: { run: sleep, ready: started }\n";
const AFTER: &str = "project: demo\nservices:\n  web: { run: \"echo two; sleep\", ready: started }\n  worker: { run: sleep, ready: started }\n";

/// The lane at `cwd`.
async fn lane(h: &crate::Harness, cwd: &std::path::Path) -> LaneSummary {
    let value = h
        .call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status");
    let page: Page<LaneSummary> = serde_json::from_value(value).expect("page");
    page.items.into_iter().next().expect("lane")
}

/// Each unit's drift, sorted by name.
fn drift(lane: &LaneSummary) -> Vec<(String, UnitDrift)> {
    let mut drift: Vec<(String, UnitDrift)> = lane
        .units
        .iter()
        .filter_map(|unit| Some((unit.name.clone(), unit.drift.clone()?)))
        .collect();
    drift.sort_by(|a, b| a.0.cmp(&b.0));
    drift
}

#[tokio::test(flavor = "multi_thread")]
/// An edit shows per unit (changed with what, removed, added) without touching
/// anything; `reload` stops the removed unit, restarts the changed one and starts
/// the added one, and a second reload has nothing to do.
async fn reload_applies_only_what_the_file_changed() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", BEFORE);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        assert!(drift(&lane(&h, &main).await).is_empty());
        std::fs::write(main.join("devlanes.yaml"), AFTER).expect("edit");
        let edited = lane(&h, &main).await;
        assert!(edited.config_changed);
        assert_eq!(
            drift(&edited),
            [
                (
                    "api".to_owned(),
                    UnitDrift {
                        kind: "removed".into(),
                        fields: Vec::new()
                    }
                ),
                (
                    "web".to_owned(),
                    UnitDrift {
                        kind: "changed".into(),
                        fields: vec!["run".into()]
                    }
                ),
                (
                    "worker".to_owned(),
                    UnitDrift {
                        kind: "added".into(),
                        fields: Vec::new()
                    }
                ),
            ]
        );
        let journal = h.runner.journal.entries().len();
        let value = h
            .call(Method::Reload, json!({}), cx(&main))
            .await
            .expect("reload");
        let result: StartResult = serde_json::from_value(value).expect("result");
        let mut started = result.started.clone();
        started.sort();
        assert_eq!(started, ["demo@0:web", "demo@0:worker"]);
        assert!(
            result.warnings.iter().any(|w| w.code == "units_removed"),
            "{result:?}"
        );
        let mut spawned = h.runner.journal.entries()[journal..].to_vec();
        spawned.sort();
        assert_eq!(spawned, ["echo two; sleep", "sleep"]);
        let reloaded = lane(&h, &main).await;
        assert!(!reloaded.config_changed, "{reloaded:?}");
        assert!(drift(&reloaded).is_empty());
        let api = reloaded.units.iter().find(|unit| unit.name == "api");
        assert!(api.is_none_or(|api| api.state != "ready"), "{api:?}");

        let again: StartResult = serde_json::from_value(
            h.call(Method::Reload, json!({}), cx(&main))
                .await
                .expect("reload again"),
        )
        .expect("result");
        assert!(again.started.is_empty());
        assert!(again.warnings.iter().any(|w| w.code == "nothing_to_reload"));
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Raising `replicas` adds the new replicas, which `reload` starts next to the
/// running ones.
async fn raised_replicas_are_added() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let file = |count: u8| {
            format!("project: demo\nservices:\n  api: {{ run: sleep, ready: started, replicas: {count} }}\n")
        };
        let main = repo(&tmp.path, "r", &file(2));
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        std::fs::write(main.join("devlanes.yaml"), file(3)).expect("edit");
        assert_eq!(
            drift(&lane(&h, &main).await),
            [(
                "api.3".to_owned(),
                UnitDrift {
                    kind: "added".into(),
                    fields: Vec::new()
                }
            )]
        );
        let result: StartResult = serde_json::from_value(
            h.call(Method::Reload, json!({}), cx(&main))
                .await
                .expect("reload"),
        )
        .expect("result");
        assert_eq!(result.started, ["demo@0:api.3"]);
        assert!(drift(&lane(&h, &main).await).is_empty());
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A `.env` value a service reads counts as a change of that variable, named
/// without its value.
async fn an_env_file_edit_is_drift_by_name() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = "project: demo\nservices:\n  web: { run: sleep, ready: started, env: { TOKEN: \"${{ env.TOKEN }}\" } }\n";
        let main = repo(&tmp.path, "r", config);
        std::fs::write(main.join(".env"), "TOKEN=first\n").expect("env");
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        std::fs::write(main.join(".env"), "TOKEN=second-value\n").expect("edit env");
        let edited = lane(&h, &main).await;
        assert_eq!(
            drift(&edited),
            [(
                "web".to_owned(),
                UnitDrift {
                    kind: "changed".into(),
                    fields: vec!["env TOKEN".into()]
                }
            )]
        );
        let json = serde_json::to_string(&edited).expect("json");
        assert!(!json.contains("second-value"), "values never leave the daemon");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A start from another shell replaces the lane's client environment; its variables
/// are not the file's, so an edit that changes nothing else shows no drift.
async fn another_shells_environment_is_not_drift() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", BEFORE);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let mut env = crate::client_env();
        env.insert("OTHER_SHELL".into(), "yes".into());
        let other = devlanes_proto::Context {
            env: Some(env),
            ..cx(&main)
        };
        h.call(Method::Start, json!({}), other)
            .await
            .expect("start from another shell");
        std::fs::write(main.join("devlanes.yaml"), format!("{BEFORE}# a comment\n")).expect("edit");
        let edited = lane(&h, &main).await;
        assert!(drift(&edited).is_empty(), "{:?}", drift(&edited));
        assert!(!edited.config_changed);
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A whole-lane restart with a file that does not load refuses before stopping
/// anything; `reload` of a unit the lane does not have is `not_found`, and of a
/// stopped lane `not_running`.
async fn restart_and_reload_refuse_before_touching_the_lane() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", BEFORE);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let typo = h
            .call(Method::Reload, json!({"units": ["wbe"]}), cx(&main))
            .await
            .expect_err("typo");
        assert_eq!(typo.code, "not_found");
        std::fs::write(main.join("devlanes.yaml"), "project: demo\n<<<<<<< HEAD\n").expect("break");
        let refused = h
            .call(Method::Restart, json!({}), cx(&main))
            .await
            .expect_err("broken file");
        assert_eq!(refused.code, "config_error");
        let still = lane(&h, &main).await;
        assert_eq!(still.state, "running", "{still:?}");
        std::fs::write(main.join("devlanes.yaml"), BEFORE).expect("fix");
        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop");
        let stopped = h
            .call(Method::Reload, json!({}), cx(&main))
            .await
            .expect_err("nothing runs");
        assert_eq!(stopped.code, "not_running");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `restart` without units restarts the whole lane and keeps what it ran.
async fn restart_without_units_restarts_the_whole_lane() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", BEFORE);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({"units": ["web"]}), cx(&main))
            .await
            .expect("start web");
        let result: StartResult = serde_json::from_value(
            h.call(Method::Restart, json!({}), cx(&main))
                .await
                .expect("restart"),
        )
        .expect("result");
        assert!(result.whole_lane, "{result:?}");
        assert_eq!(result.started, ["demo@0:web"]);
        h.close().await;
    })
    .await
    .expect("timeout");
}
