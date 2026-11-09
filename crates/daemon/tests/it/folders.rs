//! Which folders are lanes: worktrees become lanes when devlanes runs in them,
//! a folder can hold several lanes, and per-unit facts clients render
//! (display name, readiness check, terminal).

use std::path::Path;

use devlanes_proto::{Context, LaneSummary, Method, Page, StartResult};
use serde_json::{Value, json};

use crate::{LIMIT, Setup, cx, git, harness, repo, temp, worktree};

const CONFIG: &str = r#"project: demo
services:
  web:
    name: Web shop
    run: serve
    hosts: ["web.${{ lane.domain }}"]
  worker:
    run: sleep
    pty: false
"#;

/// Every lane visible from `cwd`.
async fn status(h: &crate::Harness, context: Context) -> Vec<LaneSummary> {
    let value = h
        .call(Method::Status, json!({}), context)
        .await
        .expect("status");
    serde_json::from_value::<Page<LaneSummary>>(value)
        .expect("page")
        .items
}

/// `cx(cwd)` aimed at `lane`.
fn on_lane(cwd: &Path, lane: &str) -> Context {
    Context {
        lane: Some(lane.into()),
        ..cx(cwd)
    }
}

/// Starts with `context`.
async fn start(h: &crate::Harness, context: Context, params: Value) -> StartResult {
    let value = h.call(Method::Start, params, context).await.expect("start");
    serde_json::from_value(value).expect("start result")
}

#[tokio::test(flavor = "multi_thread")]
/// A CLI from before `lane new <branch>` was removed still sends `branch`; the
/// daemon refuses it instead of adding an extra lane for the folder.
async fn lane_new_with_a_branch_is_refused() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let error = h
            .call(Method::LaneNew, json!({"branch": "feat/x"}), cx(&main))
            .await
            .expect_err("branch refused");
        assert_eq!((error.code.as_str(), error.exit_code), ("bad_request", 2));
        let message = error.data["message"].as_str().unwrap_or_default();
        assert!(message.contains("git worktree add"), "{message}");
        let lanes = h
            .call(Method::LaneList, json!({}), cx(&main))
            .await
            .expect("lane list");
        assert_eq!(lanes["items"].as_array().map(Vec::len), Some(1), "{lanes}");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `lane new --name` adds a second lane for the cwd's folder with its own index
/// and domain; `--lane <name>` reaches it, the folder's commands keep using its
/// first lane, names are unique, and the extra lane survives a daemon restart.
async fn a_folder_can_hold_several_lanes() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let home = tmp.path.join("home");
        let h = harness(&home, Setup::default()).await;
        let created = h
            .call(Method::LaneNew, json!({"name": "review"}), cx(&main))
            .await
            .expect("lane new");
        assert_eq!(created["name"], "review");
        assert_eq!(created["lane"]["reference"], "demo@1", "{created}");
        assert_eq!(created["lane"]["domain"], "demo1.localhost");
        // Clients label it by its name: its branch is the folder's first lane's.
        assert_eq!(created["lane"]["name"], "review");

        let first = start(&h, cx(&main), json!({"units": ["web"]})).await;
        assert_eq!(
            first.lane, "demo@0",
            "the folder's first lane stays its default"
        );
        // Clients that complete a command there are told the same lane.
        let here = h
            .call(
                Method::Status,
                json!({"all": true, "here_for": "logs"}),
                cx(&main),
            )
            .await
            .expect("status");
        assert_eq!(here["here"], "demo@0", "{here}");
        let second = start(&h, on_lane(&main, "review"), json!({"units": ["web"]})).await;
        assert_eq!(second.lane, "demo@1");
        assert_eq!(second.started, ["demo@1:web"]);
        let lanes = status(&h, cx(&main)).await;
        let ports: Vec<u16> = lanes
            .iter()
            .map(|lane| {
                lane.units
                    .iter()
                    .find(|u| u.name == "web")
                    .expect("web")
                    .ports["default"]
            })
            .collect();
        assert_eq!(lanes.len(), 2);
        assert_ne!(ports[0], ports[1], "each lane has its own ports");
        assert!(lanes.iter().all(|lane| lane.path == main));
        let names: Vec<Option<&str>> = lanes.iter().map(|lane| lane.name.as_deref()).collect();
        assert_eq!(
            names,
            [None, Some("review")],
            "the worktree's own lane has none"
        );

        let taken = h
            .call(Method::LaneNew, json!({"name": "review"}), cx(&main))
            .await
            .expect_err("taken");
        assert_eq!(taken.code, "name_taken");
        assert_eq!(
            (&taken.data["kind"], &taken.data["lane"]),
            (&json!("lane"), &json!("demo@1"))
        );
        // Lanes without a `lane new` name answer to these as references.
        for reserved in ["main", "lane7"] {
            let refused = h
                .call(Method::LaneNew, json!({"name": reserved}), cx(&main))
                .await
                .expect_err(reserved);
            assert_eq!(refused.code, "invalid_reference", "{reserved}");
        }
        let unnamed = h
            .call(Method::LaneNew, json!({}), cx(&main))
            .await
            .expect("default name");
        assert_eq!(unnamed["name"], "r-2");
        assert_eq!(unnamed["lane"]["reference"], "demo@2");

        h.call(Method::Stop, json!({}), on_lane(&main, "review"))
            .await
            .expect("stop review");
        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop main");
        h.close().await;
        drop(h);

        let h = harness(&home, Setup::default()).await;
        let lanes = status(&h, cx(&main)).await;
        let references: Vec<&str> = lanes.iter().map(|l| l.reference.as_str()).collect();
        assert_eq!(references, ["demo@0", "demo@1", "demo@2"]);
        let again = start(&h, on_lane(&main, "review"), json!({"units": ["web"]})).await;
        assert_eq!(again.lane, "demo@1");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Two lanes of one folder get different data folders under the devlanes home; each
/// exists once its lane does, and `lane clean` deletes only the cleaned lane's.
async fn lanes_of_one_folder_have_their_own_data_folders() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = "project: demo\nservices:\n  worker:\n    run: \"echo ${{ lane.data_path }}; sleep\"\n    pty: false\n";
        let main = repo(&tmp.path, "r", config);
        let home = tmp.path.join("home");
        let h = harness(&home, Setup::default()).await;
        h.call(Method::LaneNew, json!({"name": "review"}), cx(&main))
            .await
            .expect("lane new");
        start(&h, cx(&main), json!({})).await;
        start(&h, on_lane(&main, "review"), json!({})).await;
        let paths: Vec<std::path::PathBuf> = h
            .runner
            .journal
            .entries()
            .iter()
            .filter_map(|script| script.strip_prefix("echo "))
            .filter_map(|rest| rest.split(';').next())
            .map(std::path::PathBuf::from)
            .collect();
        assert_eq!(paths.len(), 2, "{paths:?}");
        assert_ne!(paths[0], paths[1], "one folder each");
        for path in &paths {
            assert!(path.starts_with(home.join("data/demo")), "{path:?}");
            assert!(path.is_dir(), "{path:?} exists");
            // `/` only, so the path survives interpolation into `sh` strings.
            #[cfg(windows)]
            assert!(!path.to_string_lossy().contains('\\'), "{path:?}");
        }

        h.call(Method::Stop, json!({}), on_lane(&main, "review"))
            .await
            .expect("stop review");
        // Each lane's logs live under its index.
        let log_lanes = || -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(home.join("logs").join("demo"))
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        };
        assert_eq!(log_lanes(), ["0", "1"]);
        h.call(
            Method::LaneClean,
            json!({}),
            Context {
                yes: true,
                ..on_lane(&main, "review")
            },
        )
        .await
        .expect("clean review");
        assert!(!paths[1].exists(), "the cleaned lane's folder is gone");
        assert!(paths[0].is_dir(), "the other lane keeps its folder");
        assert_eq!(log_lanes(), ["0"], "the cleaned lane's logs are gone");
        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop main");
        h.close().await;
    })
    .await
    .expect("timeout");
}

/// `DEVLANES_LANE_DATA` of the lane `context` names.
async fn data_path(h: &crate::Harness, context: Context) -> std::path::PathBuf {
    let env = h
        .call(Method::Env, json!({"lane_env": true}), context)
        .await
        .expect("env");
    env["env"]["DEVLANES_LANE_DATA"]
        .as_str()
        .expect("data path")
        .into()
}

/// Whether `env --explain` for `worker` on the lane `context` names lists `key`.
async fn explains(h: &crate::Harness, context: Context, key: &str) -> bool {
    let explained = h
        .call(
            Method::Env,
            json!({"unit": "worker", "explain": true}),
            context,
        )
        .await
        .expect("explain");
    explained["items"]
        .as_array()
        .expect("items")
        .iter()
        .any(|item| item["key"] == key)
}

/// `lane clean --yes` of the lane `context` names, or of `lane` when given.
async fn clean(
    h: &crate::Harness,
    context: Context,
    lane: Option<&str>,
) -> Result<Value, devlanes_proto::RpcError> {
    let params = lane.map_or_else(|| json!({}), |lane| json!({"lane": lane}));
    h.call(
        Method::LaneClean,
        params,
        Context {
            yes: true,
            ..context
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
/// A cleaned lane comes back as a new lane, whatever its kind: a new data
/// folder and none of the old lane's `--env` overrides. Cleaning the primary
/// lane assigns it nothing until a command claims it, a `lane new` lane that
/// waits for `lanes.max` can be cleaned by its name, and cleaning the lanes
/// around a waiting lane keeps the project and the waiting lane.
async fn a_cleaned_lane_comes_back_as_a_new_lane() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = CONFIG.replacen("project: demo\n", "project: demo\nlanes: { max: 2 }\n", 1);
        let main = repo(&tmp.path, "r", &config);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::LaneNew, json!({"name": "review"}), cx(&main))
            .await
            .expect("lane new");
        let references = |lanes: Vec<LaneSummary>| -> Vec<String> {
            lanes.into_iter().map(|lane| lane.reference).collect()
        };

        // The primary lane.
        let flag = json!({"units": ["worker"], "env": {"FROM_FLAG": "old"}});
        start(&h, cx(&main), flag.clone()).await;
        let old = data_path(&h, cx(&main)).await;
        clean(&h, cx(&main), None).await.expect("clean lane 0");
        // Reads the files the way the config poller does, without a claim.
        h.call(Method::ConfigShow, json!({}), cx(&main))
            .await
            .expect("config show");
        assert_eq!(references(status(&h, cx(&tmp.path)).await), ["demo@1"]);
        assert_ne!(data_path(&h, cx(&main)).await, old);
        assert!(!explains(&h, cx(&main), "FROM_FLAG").await);

        // A `lane new` lane.
        start(&h, on_lane(&main, "review"), flag).await;
        let old = data_path(&h, on_lane(&main, "review")).await;
        clean(&h, cx(&main), Some("review")).await.expect("clean");
        h.call(Method::LaneNew, json!({"name": "review"}), cx(&main))
            .await
            .expect("the name is free again");
        assert_ne!(data_path(&h, on_lane(&main, "review")).await, old);
        assert!(!explains(&h, on_lane(&main, "review"), "FROM_FLAG").await);

        // A `lane new` lane waiting for `lanes.max`.
        let new_extra = || h.call(Method::LaneNew, json!({"name": "extra"}), cx(&main));
        let waiting = new_extra().await.expect_err("lanes.max");
        assert_eq!(waiting.code, "unassigned", "{waiting:?}");
        assert_eq!(waiting.data["lane"], "demo@extra", "{waiting:?}");
        assert_eq!(new_extra().await.expect_err("taken").code, "name_taken");
        clean(&h, cx(&main), Some("extra"))
            .await
            .expect("clean a waiting lane");
        assert_eq!(new_extra().await.expect_err("waits").code, "unassigned");

        // Cleaning the assigned lanes keeps the waiting one and its project.
        clean(&h, cx(&main), Some("review")).await.expect("clean");
        clean(&h, cx(&main), Some("0")).await.expect("clean");
        let extra = status(&h, cx(&main))
            .await
            .into_iter()
            .find(|lane| lane.name.as_deref() == Some("extra"))
            .expect("extra listed");
        assert!(extra.index.is_some(), "{extra:?}");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `lane new` answers with the project's folder: the worktree root without a
/// trailing separator, or the subfolder holding the project file.
async fn lane_new_answers_with_the_project_folder() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let sub = main.join("sub");
        std::fs::create_dir_all(&sub).expect("sub");
        std::fs::write(sub.join("devlanes.yaml"), CONFIG.replace("demo", "other"))
            .expect("sub config");
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        for folder in [&main, &sub] {
            let created = h
                .call(Method::LaneNew, json!({}), cx(folder))
                .await
                .expect("lane new");
            assert_eq!(created["folder"], json!(folder));
        }
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A lone unit selector that matches no unit but is a lane number or a `lane
/// new` name names that lane in the error; a branch or folder name does not,
/// since those are often a mistyped unit of the current lane.
async fn a_unit_selector_that_names_a_lane_says_so() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::LaneNew, json!({"name": "review"}), cx(&main))
            .await
            .expect("lane new");
        let cases = [
            (Method::Stop, "1", Some("demo@1")),
            (Method::Stop, "review", Some("demo@1")),
            (Method::Start, "review", Some("demo@1")),
            (Method::Reload, "1", Some("demo@1")),
            (Method::Stop, "main", None),
            (Method::Stop, "r", None),
            (Method::Stop, "nope", None),
        ];
        for (method, unit, lane) in cases {
            let error = h
                .call(method, json!({"units": [unit]}), cx(&main))
                .await
                .expect_err(unit);
            assert_eq!(error.code, "not_found", "{unit}");
            assert_eq!(
                error.data.get("names_lane").and_then(Value::as_str),
                lane,
                "{unit}"
            );
        }
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
/// A project named like a DOS device gets an encoded data folder, which Windows
/// can create, and `lane clean` removes it.
async fn windows_device_project_names_get_encoded_folders() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = "project: aux\nservices:\n  worker:\n    run: \"echo ${{ lane.data_path }}; sleep\"\n    pty: false\n";
        let main = repo(&tmp.path, "r", config);
        let home = tmp.path.join("home");
        let h = harness(&home, Setup::default()).await;
        start(&h, cx(&main), json!({})).await;
        let path: std::path::PathBuf = h
            .runner
            .journal
            .entries()
            .iter()
            .filter_map(|script| script.strip_prefix("echo "))
            .filter_map(|rest| rest.split(';').next())
            .map(std::path::PathBuf::from)
            .next()
            .expect("the data path");
        assert!(path.starts_with(home.join("data").join("%61ux")), "{path:?}");
        assert!(path.is_dir(), "{path:?} exists");
        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop");
        h.call(
            Method::LaneClean,
            json!({}),
            Context {
                yes: true,
                ..cx(&main)
            },
        )
        .await
        .expect("clean");
        assert!(!home.join("data").join("%61ux").exists());
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Cleaning every lane of a project forgets it: a restarted daemon does not bring
/// it back, reads in its folder leave it forgotten, and `status` there registers it
/// again.
async fn a_project_whose_lanes_were_all_cleaned_is_forgotten() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let home = tmp.path.join("home");
        let h = harness(&home, Setup::default()).await;
        start(&h, cx(&main), json!({"units": ["worker"]})).await;
        h.call(
            Method::LaneClean,
            json!({}),
            Context {
                yes: true,
                ..cx(&main)
            },
        )
        .await
        .expect("clean");
        assert!(
            !home.join("data/demo").exists(),
            "the project's data folder goes with its last lane"
        );
        // Outside any project, status lists every project the daemon knows.
        assert!(status(&h, cx(&tmp.path)).await.is_empty());
        h.close().await;
        drop(h);

        let h = harness(&home, Setup::default()).await;
        assert!(
            status(&h, cx(&tmp.path)).await.is_empty(),
            "a restart does not bring it back"
        );
        for (method, params) in [(Method::Hosts, json!({})), (Method::Metrics, json!({}))] {
            h.call(method, params, cx(&main)).await.expect("a read");
        }
        let shown = h
            .call(Method::ConfigShow, json!({}), cx(&main))
            .await
            .expect("the file alone");
        assert_eq!(
            (&shown["project"], &shown["lane"]),
            (&json!("demo"), &Value::Null)
        );
        for unit in ["worker", "demo@0:worker"] {
            let error = h
                .call(Method::Logs, json!({"units": [unit]}), cx(&main))
                .await
                .expect_err(unit);
            assert_eq!(error.code, "project_not_started", "{unit}");
        }
        let stopped = h
            .call(Method::Stop, json!({"units": ["worker"]}), cx(&main))
            .await
            .expect("nothing to stop");
        assert_eq!(stopped["items"], json!([]));
        // The `logs` picker asks which lane `logs` would use.
        let picker = h
            .call(
                Method::Status,
                json!({"all": true, "here_for": "logs"}),
                cx(&main),
            )
            .await
            .expect("picker status");
        assert_eq!(
            picker["here_error"]["code"], "project_not_started",
            "{picker}"
        );
        assert!(
            status(&h, cx(&tmp.path)).await.is_empty(),
            "reads in the folder do not bring it back"
        );
        assert!(!home.join("data/demo").exists());
        let lanes = status(&h, cx(&main)).await;
        assert_eq!(lanes.len(), 1, "status in the folder does");
        assert_eq!(lanes[0].reference, "demo@0");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Starting another project's lane from a cleaned project's folder leaves that
/// project forgotten, whether `--lane` or a unit names the lane, and so does
/// naming a project no file there names; naming the folder's own project still
/// registers it again.
async fn naming_another_projects_lane_leaves_a_cleaned_project_forgotten() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let root = tmp.path.join("r");
        for (dir, name) in [("apps/a", "alpha"), ("apps/b", "beta")] {
            let folder = root.join(dir);
            std::fs::create_dir_all(&folder).expect("mkdir");
            std::fs::write(
                folder.join("devlanes.yaml"),
                format!("project: {name}\nservices:\n  web: {{ run: sleep, ready: started }}\n"),
            )
            .expect("project file");
        }
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let (a, b) = (root.join("apps/a"), root.join("apps/b"));
        let home = tmp.path.join("home");
        let h = harness(&home, Setup::default()).await;
        start(&h, cx(&a), json!({"units": ["web"]})).await;
        start(&h, cx(&b), json!({"units": ["web"]})).await;
        h.call(
            Method::LaneClean,
            json!({}),
            Context {
                yes: true,
                ..cx(&a)
            },
        )
        .await
        .expect("clean");

        let by_lane = start(&h, on_lane(&a, "beta@0"), json!({})).await;
        let by_unit = start(&h, cx(&a), json!({"units": ["beta@0:web"]})).await;
        assert_eq!(
            (by_lane.lane.as_str(), by_unit.lane.as_str()),
            ("beta@0", "beta@0")
        );
        // A project that no file here names is not found, and registers nothing.
        let error = h
            .call(Method::Status, json!({}), on_lane(&a, "gamma@0"))
            .await
            .expect_err("unknown project");
        assert_eq!(error.code, "not_found");
        let lanes: Vec<String> = status(&h, cx(&tmp.path))
            .await
            .into_iter()
            .map(|lane| lane.reference)
            .collect();
        assert_eq!(lanes, ["beta@0"]);
        assert!(!home.join("data/alpha").exists());

        let own = start(&h, on_lane(&a, "alpha@0"), json!({})).await;
        assert_eq!(own.lane, "alpha@0");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A worktree that only exists is no lane; running devlanes in it makes one, and
/// a reference by its directory name reaches a worktree that has none.
async fn worktrees_become_lanes_when_devlanes_runs_in_them() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let wt = tmp.path.join("wt-feature");
        worktree(&main, &wt, "feat/x");
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let lanes = status(&h, cx(&main)).await;
        assert_eq!(lanes.len(), 1, "{lanes:?}");
        assert_eq!(lanes[0].index, Some(0));

        let stopped = h
            .call(Method::Stop, json!({}), on_lane(&main, "demo@wt-feature"))
            .await
            .expect("a worktree without a lane can be named");
        assert_eq!(stopped["items"], json!([]));
        h.call(Method::Status, json!({}), on_lane(&main, "demo@wt-feature"))
            .await
            .expect("status of a worktree without a lane");
        assert_eq!(
            status(&h, cx(&main)).await.len(),
            1,
            "naming it made no lane"
        );

        let result = start(&h, cx(&wt), json!({"units": ["worker"]})).await;
        assert_eq!(result.lane, "demo@1");
        assert_eq!(status(&h, cx(&main)).await.len(), 2);
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Summaries carry the display name, the readiness check and whether the unit
/// has a terminal; resizing a pipe unit is a silent no-op; starting units that
/// run already changes nothing.
async fn summaries_and_pipe_units() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let idle = status(&h, cx(&main)).await;
        let web = idle[0].units.iter().find(|u| u.name == "web").expect("web");
        assert_eq!(web.display_name.as_deref(), Some("Web shop"));
        assert_eq!(
            web.public_url.as_deref(),
            Some("https://web.demo.localhost")
        );
        let orders: Vec<(&str, Option<u32>)> = idle[0]
            .units
            .iter()
            .map(|u| (u.name.as_str(), u.order))
            .collect();
        assert_eq!(orders, [("web", Some(0)), ("worker", Some(1))]);

        let result = start(&h, cx(&main), json!({})).await;
        assert!(result.ready, "{result:?}");
        let units = &status(&h, cx(&main)).await[0].units;
        let web = units.iter().find(|u| u.name == "web").expect("web");
        let worker = units.iter().find(|u| u.name == "worker").expect("worker");
        assert_eq!(web.label(), "Web shop");
        assert_eq!(worker.label(), "worker");
        assert_eq!(web.ready_check.as_deref(), Some("tcp"));
        assert_eq!((web.state.as_str(), web.shown_state()), ("ready", "ready"));
        assert_eq!(worker.ready_check.as_deref(), Some("started"));
        assert_eq!(
            (worker.state.as_str(), worker.shown_state()),
            ("ready", "running"),
            "alive is not a passed check"
        );
        assert!(web.pty && !worker.pty);
        assert_eq!(web.urls.get(1), web.public_url.as_ref(), "{:?}", web.urls);
        assert_eq!((web.order, worker.order), (Some(0), Some(1)));
        assert_eq!(worker.public_url, None);

        let (session, _frames) = h.session();
        let attached = h
            .call_on(
                Method::Attach,
                json!({"unit": "worker", "interactive": true, "columns": 90, "rows": 20}),
                cx(&main),
                &session,
            )
            .await
            .expect("attach");
        let resized = h
            .call_on(
                Method::Resize,
                json!({"stream_id": attached["stream_id"], "columns": 100, "rows": 30}),
                cx(&main),
                &session,
            )
            .await
            .expect("a pipe resize is no error");
        assert_eq!(resized["applied"], true);
        let worker_pid = worker.pid.expect("pid");
        assert!(
            !h.runner
                .resizes
                .entries()
                .iter()
                .any(|entry| entry.starts_with(&format!("{worker_pid} "))),
            "a pipe unit's runner is never asked to resize"
        );
        session.close();

        let spawned = h.runner.journal.entries().len();
        let again = start(&h, cx(&main), json!({})).await;
        assert!(again.started.is_empty() && !again.whole_lane, "{again:?}");
        assert_eq!(again.already_running.len(), 2);
        let one = start(&h, cx(&main), json!({"units": ["web"]})).await;
        assert_eq!(one.already_running, ["demo@0:web"]);
        assert!(one.started.is_empty());
        assert_eq!(
            h.runner.journal.entries().len(),
            spawned,
            "nothing respawned"
        );
        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop");
        h.close().await;
    })
    .await
    .expect("timeout");
}
