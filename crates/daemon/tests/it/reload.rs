//! Drift between a running lane and its edited project file, `reload`, which applies
//! only the difference, and `restart` without units, which restarts the whole lane.

use std::time::Duration;

use devlanes_proto::{LaneSummary, Method, Page, StartResult, UnitDrift};
use serde_json::json;

use crate::{LIMIT, Setup, cx, eventually, harness, repo, temp};

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
        assert!(
            reloaded.units.iter().all(|unit| unit.name != "api"),
            "{reloaded:?}"
        );

        let again: StartResult = serde_json::from_value(
            h.call(Method::Reload, json!({}), cx(&main))
                .await
                .expect("reload again"),
        )
        .expect("result");
        assert!(again.started.is_empty());
        assert!(again.warnings.iter().any(|w| w.code == "nothing_to_reload"));

        // A service removed and stopped, then put back, is added again.
        std::fs::write(main.join("devlanes.yaml"), BEFORE).expect("put api back");
        let restored = drift(&lane(&h, &main).await);
        assert!(
            restored
                .iter()
                .any(|(unit, drift)| unit == "api" && drift.kind == "added"),
            "{restored:?}"
        );
        let back: StartResult = serde_json::from_value(
            h.call(Method::Reload, json!({}), cx(&main))
                .await
                .expect("reload api back"),
        )
        .expect("result");
        assert!(back.started.contains(&"demo@0:api".to_owned()), "{back:?}");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Raising `replicas` adds the new replicas, which `reload` of the service's tag
/// starts next to the running ones.
async fn raised_replicas_are_added() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let file = |count: u8| {
            format!("project: demo\nservices:\n  api: {{ run: sleep, ready: started, replicas: {count}, tags: [back] }}\n")
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
            h.call(Method::Reload, json!({"units": ["+back"]}), cx(&main))
                .await
                .expect("reload"),
        )
        .expect("result");
        assert_eq!(result.started, ["demo@0:api.3"], "{result:?}");
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
/// An `infra.external` entry added in `devlanes.local.yaml` is routed at once, so
/// it is no drift and `reload` does not restart the whole lane for it.
async fn an_added_external_entry_is_not_drift() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", BEFORE);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        std::fs::write(
            main.join("devlanes.local.yaml"),
            "infra:\n  external:\n    docs: { address: '127.0.0.1:8000' }\n",
        )
        .expect("local file");
        let edited = lane(&h, &main).await;
        assert!(
            edited.units.iter().any(|unit| unit.name == "docs"),
            "{edited:?}"
        );
        assert!(
            !edited.infra_changed && !edited.config_changed,
            "{edited:?}"
        );
        let result: StartResult = serde_json::from_value(
            h.call(Method::Reload, json!({}), cx(&main))
                .await
                .expect("reload"),
        )
        .expect("result");
        assert!(!result.whole_lane, "{result:?}");
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.code == "nothing_to_reload"),
            "{result:?}"
        );
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A request replaces the lane's `--env` values only when it carries `env`, with
/// or without a shell environment: the TUI's restart keeps them, `devlanes mcp`'s
/// `restart <unit>` (no shell environment) drops them. A whole-lane `stop`
/// forgets them, so a later start without `env` does not bring them back.
async fn restart_replaces_env_overrides_only_when_asked() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", BEFORE);
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                user: crate::pool("11000-11249"),
                ..Setup::default()
            },
        )
        .await;
        h.call(
            Method::Start,
            json!({"units": ["web"], "env": {"EXTRA": "one"}}),
            cx(&main),
        )
        .await
        .expect("start");
        let extra = || async {
            let value = h
                .call(Method::Env, json!({"unit": "web"}), cx(&main))
                .await
                .expect("env");
            value["env"]["EXTRA"].as_str().map(str::to_owned)
        };
        let no_shell = || devlanes_proto::Context {
            env: None,
            ..cx(&main)
        };
        h.call(Method::Restart, json!({"units": ["web"]}), no_shell())
            .await
            .expect("the TUI's restart");
        assert_eq!(extra().await.as_deref(), Some("one"));
        h.call(
            Method::Restart,
            json!({"units": ["web"], "env": {}}),
            no_shell(),
        )
        .await
        .expect("devlanes mcp's restart");
        assert_eq!(extra().await, None);

        h.call(
            Method::Start,
            json!({"units": ["web"], "env": {"EXTRA": "one"}}),
            cx(&main),
        )
        .await
        .expect("start with --env again");
        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop the lane");
        h.call(Method::Start, json!({"units": ["web"]}), no_shell())
            .await
            .expect("start without env");
        assert_eq!(extra().await, None);
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
/// `restart` without units restarts the whole lane and keeps what it ran; the
/// exec units it stopped are named in an `exec_stopped` warning.
async fn restart_without_units_restarts_the_whole_lane() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", BEFORE);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({"units": ["web"]}), cx(&main))
            .await
            .expect("start web");
        h.call(
            Method::Exec,
            json!({"name": "job", "command": ["sleep"]}),
            cx(&main),
        )
        .await
        .expect("exec");
        let restart = || async {
            let result: StartResult = serde_json::from_value(
                h.call(Method::Restart, json!({}), cx(&main))
                    .await
                    .expect("restart"),
            )
            .expect("result");
            result
        };
        let result = restart().await;
        assert!(result.whole_lane, "{result:?}");
        assert_eq!(result.started, ["demo@0:web"]);
        let exec_stopped: Vec<_> = result
            .warnings
            .iter()
            .filter(|w| w.code == "exec_stopped")
            .collect();
        assert_eq!(exec_stopped.len(), 1, "{result:?}");
        assert_eq!(exec_stopped[0].data["units"], json!(["job"]));
        let again = restart().await;
        assert!(
            again.warnings.iter().all(|w| w.code != "exec_stopped"),
            "{again:?}"
        );
        h.close().await;
    })
    .await
    .expect("timeout");
}

/// A lane with a per-lane container and one service.
const INFRA: &str = "project: demo\ninfra:\n  containers:\n    db: { image: \"postgres:16\", ports: { pg: 5432 }, per_lane: true }\nservices:\n  api: { run: sleep, ready: started }\n";

#[tokio::test(flavor = "multi_thread")]
/// An edit while `infra up` holds an idle lane changes nothing that runs, so
/// the lane has no `config changed` note once an exec unit runs there.
async fn an_edit_while_infra_comes_up_leaves_no_note() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", INFRA);
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                infra: true,
                ..Setup::default()
            },
        )
        .await;
        let gate = std::sync::Arc::new(tokio::sync::Notify::new());
        *h.infra.up_gate.lock().expect("lock") = Some(gate.clone());
        let edit = async {
            eventually(Duration::from_secs(5), || async {
                h.infra
                    .calls
                    .entries()
                    .contains(&"up 0".to_owned())
                    .then_some(())
            })
            .await
            .expect("infra up waits in the provider");
            std::fs::write(
                main.join("devlanes.yaml"),
                INFRA.replace("run: sleep", "run: \"echo two; sleep\""),
            )
            .expect("edit");
            // Reloads the file while `infra up` holds the lane.
            lane(&h, &main).await;
            gate.notify_one();
        };
        let (up, ()) = tokio::join!(h.call(Method::InfraUp, json!({}), cx(&main)), edit);
        up.expect("infra up");
        h.call(
            Method::Exec,
            json!({"name": "job", "command": ["sleep"]}),
            cx(&main),
        )
        .await
        .expect("exec");
        let running = lane(&h, &main).await;
        assert_eq!(running.state, "running", "{running:?}");
        assert!(!running.config_changed, "{running:?}");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A service dropped from the file shows `removed` while it runs and leaves
/// status once it exited on its own, with no note left; its logs stay. Put back,
/// it is `added` and `reload` starts it. A stop's effect on drift shows in the
/// next status at once.
async fn a_removed_service_comes_and_goes_with_its_runs() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let before = "project: demo\nservices:\n  web: { run: sleep, ready: started }\n  worker: { run: sleep, ready: started }\n  api: { run: \"echo api; hold\", ready: started }\n";
        let edited = "project: demo\nservices:\n  web: { run: sleep, ready: started }\n  worker: { run: \"echo two; sleep\", ready: started }\n";
        let main = repo(&tmp.path, "r", before);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        std::fs::write(main.join("devlanes.yaml"), edited).expect("drop api");
        let kinds = |lane: &LaneSummary| {
            drift(lane)
                .into_iter()
                .map(|(unit, drift)| (unit, drift.kind))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(&lane(&h, &main).await),
            [
                ("api".to_owned(), "removed".to_owned()),
                ("worker".to_owned(), "changed".to_owned())
            ]
        );
        h.call(Method::Stop, json!({"units": ["worker"]}), cx(&main))
            .await
            .expect("stop worker");
        let stopped = lane(&h, &main).await;
        assert_eq!(
            kinds(&stopped),
            [("api".to_owned(), "removed".to_owned())],
            "{stopped:?}"
        );

        h.runner.release();
        let exited = eventually(Duration::from_secs(5), || async {
            let lane = lane(&h, &main).await;
            lane.units
                .iter()
                .all(|unit| unit.name != "api")
                .then_some(lane)
        })
        .await
        .expect("api leaves status once it exited");
        assert!(!exited.config_changed, "{exited:?}");
        let logs = h
            .call(Method::Logs, json!({"units": ["api"]}), cx(&main))
            .await
            .expect("logs of the removed service");
        assert_eq!(logs["items"][0]["text"], "api", "{logs}");

        std::fs::write(
            main.join("devlanes.yaml"),
            format!("{edited}  api: {{ run: sleep, ready: started }}\n"),
        )
        .expect("put api back");
        assert_eq!(
            kinds(&lane(&h, &main).await),
            [("api".to_owned(), "added".to_owned())]
        );
        let back: StartResult = serde_json::from_value(
            h.call(Method::Reload, json!({}), cx(&main))
                .await
                .expect("reload"),
        )
        .expect("result");
        assert_eq!(back.started, ["demo@0:api"], "{back:?}");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A value a service reads from the client's shell through `${{ env.NAME }}` is
/// drift after a start from a shell where it differs, with no edit.
async fn a_shell_value_the_file_reads_is_drift() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = "project: demo\nservices:\n  web: { run: sleep, ready: started, env: { GREETING: \"${{ env.GREETING }}\" } }\n";
        let main = repo(&tmp.path, "r", config);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let shell = |greeting: &str| {
            let mut env = crate::client_env();
            env.insert("GREETING".into(), greeting.into());
            devlanes_proto::Context {
                env: Some(env),
                ..cx(&main)
            }
        };
        h.call(Method::Start, json!({}), shell("one"))
            .await
            .expect("start");
        assert!(drift(&lane(&h, &main).await).is_empty());
        h.call(Method::Start, json!({}), shell("two"))
            .await
            .expect("start from another shell");
        assert_eq!(
            drift(&lane(&h, &main).await),
            [(
                "web".to_owned(),
                UnitDrift {
                    kind: "changed".into(),
                    fields: vec!["env GREETING".into()]
                }
            )]
        );
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Drift plans a running unit again only when one of its plan's inputs changed:
/// a stop, or another project's lane starting when the file reads no
/// `projects.*`, calls no contributor. An edit plans the edited service once.
async fn stops_and_exits_plan_nothing() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", BEFORE);
        let other = repo(
            &tmp.path,
            "o",
            "project: other\nservices:\n  site: { run: sleep, ready: started }\n",
        );
        let journal = crate::PlanJournal::default();
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                contributors: vec![std::sync::Arc::new(journal.clone())],
                ..Setup::default()
            },
        )
        .await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        lane(&h, &main).await;
        let planned = journal.planned.entries().len();
        h.call(Method::Stop, json!({"units": ["api"]}), cx(&main))
            .await
            .expect("stop api");
        lane(&h, &main).await;
        assert_eq!(
            journal.planned.entries().len(),
            planned,
            "a stop plans nothing"
        );
        h.call(Method::Start, json!({}), cx(&other))
            .await
            .expect("start the other project");
        let planned = journal.planned.entries().len();
        lane(&h, &main).await;
        assert_eq!(
            journal.planned.entries().len(),
            planned,
            "another project's lane is no input of this one"
        );
        std::fs::write(
            main.join("devlanes.yaml"),
            BEFORE.replace("echo one", "echo three"),
        )
        .expect("edit");
        let edited = lane(&h, &main).await;
        assert_eq!(&journal.planned.entries()[planned..], ["Service web"]);
        assert_eq!(
            drift(&edited),
            [(
                "web".to_owned(),
                UnitDrift {
                    kind: "changed".into(),
                    fields: vec!["run".into()]
                }
            )]
        );
        h.close().await;
    })
    .await
    .expect("timeout");
}
