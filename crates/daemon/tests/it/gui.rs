//! GUI services (`gui: true`) with the fake runner's GUI host, which serves
//! the test pattern in-process: the host's place around its unit, what its
//! ready line does to the unit, its failures, a host that dies, orphans,
//! freezing, and machines without a GUI backend.

use std::path::{Path, PathBuf};
use std::time::Duration;

use devlanes_daemon::gui::GuiOptions;
use devlanes_gui_client::{ClientOptions, connect};
use devlanes_gui_host::Command;
use devlanes_gui_proto::{
    Buttons, Point, Role,
    ready::{Ready, UnavailableReason, Unready},
};
use devlanes_proto::{GuiStatus, LaneSummary, Method, Page, UnitSummary};
use serde_json::{Value, json};

use crate::fakes::{GuiHost, SpawnedHost};
use crate::{FakeProcesses, Harness, LIMIT, Setup, cx, eventually, harness, repo, temp};

/// A GUI service whose command prints the variable its host announces.
/// Its own `WAYLAND_DISPLAY` loses to the host's.
const CONFIG: &str = "project: shop\nservices:\n  desktop: { run: printenv WAYLAND_DISPLAY; sleep, gui: true, env: { WAYLAND_DISPLAY: mine } }\n  api: { run: sleep }\n";

/// What the fake runner journals for a GUI host spawn.
const HOST: &str = " host --backend test-pattern ";

/// An engine with GUI services on, `backend` as this machine's, and a
/// `devlanes-gui` file at `program` when `installed`, over port pool `range`.
async fn gui_harness(
    tmp: &crate::Temp,
    range: &str,
    backend: Option<&str>,
    installed: bool,
) -> Harness {
    let program = tmp.path.join("bin").join("devlanes-gui");
    if installed {
        std::fs::create_dir_all(program.parent().expect("dir")).expect("bin");
        std::fs::write(&program, "").expect("devlanes-gui");
    }
    gui_harness_at(
        &tmp.path.join("home"),
        range,
        backend,
        program,
        Setup::default(),
    )
    .await
}

/// [`gui_harness`] over `home`, with the rest of `setup`.
async fn gui_harness_at(
    home: &Path,
    range: &str,
    backend: Option<&str>,
    program: PathBuf,
    setup: Setup,
) -> Harness {
    harness(
        home,
        Setup {
            user: crate::pool(range),
            gui: Some(GuiOptions {
                enabled: true,
                backend: backend.map(str::to_owned),
                program,
            }),
            ..setup
        },
    )
    .await
}

/// A context whose shell also has `DISPLAY`, which the host removes.
fn with_display(cwd: &Path) -> devlanes_proto::Context {
    let mut context = cx(cwd);
    if let Some(env) = context.env.as_mut() {
        env.insert("DISPLAY".into(), ":0".into());
    }
    context
}

/// The lane at `cwd`.
async fn lane(h: &Harness, cwd: &Path) -> LaneSummary {
    let value = h
        .call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status");
    let page: Page<LaneSummary> = serde_json::from_value(value).expect("page");
    page.items.into_iter().next().expect("lane")
}

/// `name` of the lane at `cwd`.
async fn unit(h: &Harness, cwd: &Path, name: &str) -> UnitSummary {
    lane(h, cwd)
        .await
        .units
        .into_iter()
        .find(|unit| unit.name == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

/// `unit`'s environment with each variable's layer, and its launch notes.
async fn explain(h: &Harness, cwd: &Path, unit: &str) -> (Vec<(String, String)>, Vec<String>) {
    let value = h
        .call(Method::Env, json!({"unit": unit, "explain": true}), cx(cwd))
        .await
        .expect("env");
    let items = value["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            (
                item["key"].as_str().unwrap_or_default().to_owned(),
                item["layer"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let notes = value["notes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|note| note.as_str().map(str::to_owned))
        .collect();
    (items, notes)
}

/// The reason codes `debug why` gives for `unit`.
async fn why(h: &Harness, cwd: &Path, unit: &str) -> Vec<Value> {
    let value = h
        .call(Method::Why, json!({"unit": unit}), cx(cwd))
        .await
        .expect("why");
    value["reasons"].as_array().cloned().unwrap_or_default()
}

/// The `n`th GUI host the runner spawned, once it registered.
async fn host(h: &Harness, n: usize) -> SpawnedHost {
    eventually(Duration::from_secs(5), || async {
        h.runner.hosts().get(n).cloned()
    })
    .await
    .unwrap_or_else(|| panic!("no GUI host {n}"))
}

/// Waits until the pattern of `host` received a command `pick` takes.
async fn received(host: &SpawnedHost, what: &str, pick: impl Fn(&Command) -> bool) {
    let pattern = host.pattern.clone().expect("a serving host");
    let seen = eventually(Duration::from_secs(5), || async {
        pattern.commands().iter().any(&pick).then_some(())
    })
    .await;
    assert!(seen.is_some(), "the host never got {what}");
}

/// Whether a GUI client is welcomed on `port`.
async fn answers(port: u16) -> bool {
    let options = ClientOptions::new(Role::Tool, "test", "test");
    matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            connect(("127.0.0.1", port), options)
        )
        .await,
        Ok(Ok(_))
    )
}

/// A viewer on `port` and, once its connection ended, whether it saw a
/// window Closed before that.
async fn viewer(port: u16) -> tokio::task::JoinHandle<bool> {
    let options = ClientOptions::new(Role::Viewer, "viewer", "viewer");
    let (client, mut events) = connect(("127.0.0.1", port), options)
        .await
        .expect("a viewer is welcomed");
    tokio::spawn(async move {
        let _client = client;
        let mut closed = false;
        while let Some(event) = events.next().await {
            match event {
                devlanes_gui_client::Event::Lock {
                    state: devlanes_gui_proto::LockState::Closed,
                    ..
                } => closed = true,
                devlanes_gui_client::Event::Closed(_) => break,
                _ => {}
            }
        }
        closed
    })
}

/// The unit command's position in the runner's journal, after its host's.
fn spawn_order(h: &Harness) -> (Option<usize>, Option<usize>) {
    let journal = h.runner.journal.entries();
    (
        journal.iter().position(|entry| entry.contains(HOST)),
        journal
            .iter()
            .position(|entry| entry == "printenv WAYLAND_DISPLAY; sleep"),
    )
}

#[tokio::test(flavor = "multi_thread")]
/// The host starts before the unit, announces the variables the unit gets and
/// is told the unit's identity; a restart replaces both, and a stop leaves
/// neither and frees the port. `exec` commands never get a host.
async fn a_gui_host_starts_before_its_unit_and_stops_after_it() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = gui_harness(&tmp, "17000-17049", Some("test-pattern"), true).await;
        h.call(Method::Start, json!({}), with_display(&main))
            .await
            .expect("start");
        let (host_at, unit_at) = spawn_order(&h);
        assert!(
            host_at < unit_at && host_at.is_some(),
            "{:?}",
            h.runner.journal.entries()
        );

        let desktop = unit(&h, &main, "desktop").await;
        assert_eq!(
            desktop.gui,
            Some(GuiStatus {
                host: "running".into(),
                real_desktop: Some(false),
            })
        );
        assert!(unit(&h, &main, "api").await.gui.is_none());
        let port = desktop.ports[devlanes_proto::GUI_PORT];
        assert!(answers(port).await);
        let first = host(&h, 0).await;
        let pid = desktop.pid.expect("pid");
        received(
            &first,
            "the unit's identity",
            |c| matches!(c, Command::UnitIdentity { pid: p, .. } if *p == pid),
        )
        .await;

        // The ready line's variables are the unit's, from the gui layer, and
        // the one it removes is gone.
        let (env, notes) = explain(&h, &main, "desktop").await;
        assert!(
            env.contains(&("WAYLAND_DISPLAY".into(), "gui".into())),
            "{env:?}"
        );
        assert!(!env.iter().any(|(key, _)| key == "DISPLAY"), "{env:?}");
        assert!(
            notes.iter().any(|note| note.contains("removed DISPLAY")),
            "{notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|note| note.contains("WAYLAND_DISPLAY replaces")),
            "{notes:?}"
        );
        let printed = eventually(Duration::from_secs(5), || async {
            let page = h
                .call(
                    Method::Logs,
                    json!({"units": ["desktop"], "tail": 10}),
                    cx(&main),
                )
                .await
                .ok()?;
            page.to_string().contains("devlanes-test").then_some(())
        })
        .await;
        assert!(printed.is_some(), "the unit never saw WAYLAND_DISPLAY");

        h.call(
            Method::Exec,
            json!({"name": "tool", "command": ["sleep"]}),
            cx(&main),
        )
        .await
        .expect("exec");
        assert_eq!(h.runner.hosts().len(), 1, "exec got a GUI host");

        // A restart: the viewer loses its connection without a Closed window,
        // so it waits for the next host.
        let watching = viewer(port).await;
        h.call(Method::Restart, json!({"units": ["desktop"]}), cx(&main))
            .await
            .expect("restart");
        assert!(first.unit.ended(), "the old host outlived its unit");
        assert!(
            !watching.await.expect("viewer"),
            "a restart showed the window Closed"
        );
        let second = host(&h, 1).await;
        let pid = unit(&h, &main, "desktop").await.pid.expect("pid");
        received(
            &second,
            "the new unit's identity",
            |c| matches!(c, Command::UnitIdentity { pid: p, .. } if *p == pid),
        )
        .await;
        assert!(answers(port).await);

        // A stop: the unit's exit reaches the host first, so the viewer sees
        // the window Closed before the connection ends.
        let watching = viewer(port).await;
        h.call(Method::Stop, json!({"units": ["desktop"]}), cx(&main))
            .await
            .expect("stop");
        assert!(second.unit.ended(), "the host outlived its stopped unit");
        assert!(
            watching.await.expect("viewer"),
            "a stop did not show the window Closed"
        );
        received(&second, "the unit's exit", |c| {
            matches!(c, Command::UnitExited { .. })
        })
        .await;
        assert!(!answers(port).await, "the GUI port still answers");
        assert_eq!(
            unit(&h, &main, "desktop").await.gui.map(|gui| gui.host),
            Some("stopped".into())
        );
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A host that does not come up fails its unit before the unit's command
/// runs, with the code and reason clients word, and is not left running.
async fn a_host_that_does_not_come_up_fails_the_unit_before_it_runs() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = format!("{CONFIG}  broken: {{ run: nospawn, gui: true }}\n");
        let main = repo(&tmp.path, "r", &config);
        let h = gui_harness(&tmp, "17050-17099", Some("test-pattern"), true).await;
        let address = "127.0.0.1:17050".parse().expect("address");
        let mut other_protocol = Ready::new(address, "0.0.1", &[], false);
        other_protocol.protocol += 1;
        let unready = Unready::new(
            devlanes_version::VERSION,
            UnavailableReason::NoDesktopSession,
            "no desktop session",
        );
        let cases = [
            (
                GuiHost::Exit(1, "no pixman here".into()),
                "gui_host_failed",
                "exited",
            ),
            (
                GuiHost::Line("not json".into()),
                "gui_host_failed",
                "bad_ready_line",
            ),
            (
                GuiHost::Line(other_protocol.to_line()),
                "gui_host_failed",
                "version_mismatch",
            ),
            (
                GuiHost::Line(unready.to_line()),
                "gui_unavailable",
                "no_desktop_session",
            ),
            // The control role refused: a host that got no token.
            (GuiHost::Tokenless, "gui_host_failed", "control"),
        ];
        for (n, (mode, code, reason)) in cases.into_iter().enumerate() {
            *h.runner.gui.lock().expect("lock") = mode;
            let _ = h
                .call(Method::Start, json!({"units": ["desktop"]}), cx(&main))
                .await;
            let desktop = unit(&h, &main, "desktop").await;
            assert_eq!(desktop.state, "failed", "{code} {reason}");
            let failure = desktop.reason.expect("reason");
            assert_eq!(failure.code, code, "{failure:?}");
            assert_eq!(failure.data["reason"], reason, "{failure:?}");
            if reason == "exited" {
                assert!(
                    failure.data["stderr"]
                        .as_str()
                        .is_some_and(|s| s.contains("no pixman")),
                    "{failure:?}"
                );
            }
            assert_eq!(spawn_order(&h).1, None, "the unit ran without its host");
            assert!(
                host(&h, n).await.unit.ended(),
                "a failed host was left running"
            );
        }
        // The host came up and the unit's own spawn failed: the unit fails
        // as without a host, and the host goes.
        *h.runner.gui.lock().expect("lock") = GuiHost::Serve;
        let _ = h
            .call(Method::Start, json!({"units": ["broken"]}), cx(&main))
            .await;
        let failure = unit(&h, &main, "broken").await.reason.expect("reason");
        assert_eq!(failure.code, "spawn_failed", "{failure:?}");
        let abandoned = h.runner.hosts().last().cloned().expect("a host");
        assert!(
            abandoned.unit.ended(),
            "the host outlived its unit's failed spawn"
        );
        h.close().await;

        // No devlanes-gui next to devlanes.
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = gui_harness(&tmp, "17100-17149", Some("test-pattern"), false).await;
        let _ = h.call(Method::Start, json!({}), cx(&main)).await;
        let failure = unit(&h, &main, "desktop").await.reason.expect("reason");
        assert_eq!(failure.code, "gui_host_missing", "{failure:?}");
        assert!(
            failure.data["path"]
                .as_str()
                .is_some_and(|p| p.contains("devlanes-gui"))
        );
        assert!(h.runner.hosts().is_empty());
        assert_eq!(unit(&h, &main, "api").await.state, "ready");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A host that dies while its unit runs fails nothing: the unit keeps
/// running, and status, `debug why` and the unit's notes say so.
async fn a_host_that_dies_leaves_its_unit_running() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = gui_harness(&tmp, "17150-17199", Some("test-pattern"), true).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        host(&h, 0).await.unit.crash(7);
        let exited = eventually(Duration::from_secs(5), || async {
            let desktop = unit(&h, &main, "desktop").await;
            (desktop.gui.as_ref()?.host == "exited").then_some(desktop)
        })
        .await
        .expect("status shows the exited host");
        assert_eq!(exited.state, "ready");
        let reasons = why(&h, &main, "desktop").await;
        let died = reasons
            .iter()
            .find(|reason| reason["code"] == "gui_host_exited")
            .unwrap_or_else(|| panic!("{reasons:?}"));
        assert_eq!(died["data"]["exit_code"], 7);
        let noted = eventually(Duration::from_secs(5), || async {
            let (_, notes) = explain(&h, &main, "desktop").await;
            notes
                .iter()
                .any(|note| note.contains("GUI host exited"))
                .then_some(())
        })
        .await;
        assert!(
            noted.is_some(),
            "the unit's notes do not say the host exited"
        );
        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// The host is recorded with its unit's run, so a daemon started after a
/// crash stops it with the orphaned unit.
async fn an_orphaned_gui_service_is_stopped_with_its_host() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let home = tmp.path.join("home");
        let first = gui_harness(&tmp, "17200-17249", Some("test-pattern"), true).await;
        first
            .call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let pid = unit(&first, &main, "desktop").await.pid.expect("pid");
        let gui = host(&first, 0).await;
        let program = tmp.path.join("bin").join("devlanes-gui");
        // The crash: nothing is stopped. The fake host's listener goes with
        // the old engine's tasks, as the port would stay the process's.
        drop(first);
        gui.unit.crash(0);
        let processes = FakeProcesses::default();
        processes
            .alive
            .lock()
            .expect("lock")
            .extend([pid, gui.unit.pid()]);
        // The old engine still holds its log writers, which a real crash releases.
        let second = gui_harness_at(
            &home,
            "17200-17249",
            Some("test-pattern"),
            program,
            Setup {
                processes: Some(processes.clone()),
                logs: Some(tmp.path.join("logs-after-crash")),
                ..Setup::default()
            },
        )
        .await;
        assert_eq!(unit(&second, &main, "desktop").await.state, "orphaned");
        second
            .call(Method::Stop, json!({"units": ["desktop"]}), cx(&main))
            .await
            .expect("stop");
        let killed = processes.killed.lock().expect("lock").clone();
        assert!(killed.contains(&pid), "{killed:?}");
        assert!(
            killed.contains(&gui.unit.pid()),
            "the host was not stopped: {killed:?}"
        );
        second.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A freeze suspends the unit but not its host, which is told; input in the
/// frozen lane's window reaches the host, counts as lane activity and thaws
/// the lane.
async fn input_in_a_frozen_lanes_window_thaws_it() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = gui_harness(&tmp, "17250-17299", Some("test-pattern"), true).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let gui = host(&h, 0).await;
        h.call(Method::DebugFreeze, json!({}), cx(&main))
            .await
            .expect("freeze");
        let unit_pid = unit(&h, &main, "desktop").await.pid.expect("pid");
        received(&gui, "the freeze", |c| matches!(c, Command::Frozen)).await;
        // The host hears of the freeze only once the unit is suspended.
        assert!(
            h.runner
                .powers
                .entries()
                .contains(&format!("{unit_pid} Frozen")),
            "{:?}",
            h.runner.powers.entries()
        );
        let host_pid = gui.unit.pid().to_string();
        assert!(
            !h.runner
                .powers
                .entries()
                .iter()
                .any(|entry| entry.starts_with(&format!("{host_pid} "))),
            "the host was frozen with the lane"
        );
        let port = unit(&h, &main, "desktop").await.ports[devlanes_proto::GUI_PORT];
        let options = ClientOptions::new(Role::Tool, "test", "test");
        let (client, _events) = connect(("127.0.0.1", port), options)
            .await
            .expect("the frozen lane's host answers");
        let window = client.welcome().windows.first().expect("a window").id;
        client
            .click(window, Point::new(5, 5), Buttons::LEFT)
            .expect("click");
        let thawed = eventually(Duration::from_secs(10), || async {
            lane(&h, &main).await.power.is_none().then_some(())
        })
        .await;
        assert!(thawed.is_some(), "the click did not thaw the lane");
        received(&gui, "the thaw", |c| matches!(c, Command::Thawed)).await;
        client.close();
        h.close().await;
    })
    .await
    .expect("timeout");
}

/// A process that runs for a while, standing in for a viewer.
fn stand_in() -> tokio::process::Child {
    #[cfg(windows)]
    let mut command = {
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let mut command = tokio::process::Command::new(format!(
            r"{system}\System32\WindowsPowerShell\v1.0\powershell.exe"
        ));
        command.args(["-NoProfile", "-Command", "Start-Sleep 60"]);
        command
    };
    #[cfg(unix)]
    let mut command = {
        let mut command = tokio::process::Command::new("sleep");
        command.arg("60");
        command
    };
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("stand-in")
}

/// A viewer an earlier daemon started stays open across a daemon restart
/// (the next daemon takes it on) and ends when the daemon stops for good.
/// The viewer is a stand-in process holding the viewer lock; no window opens.
#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_restart_keeps_auto_opened_viewers_and_a_stop_ends_them() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let home = tmp.path.join("home");
        let mut viewer = stand_in();
        let pid = viewer.id().expect("pid");
        let identity = devlanes_sys::identity(pid)
            .ok()
            .flatten()
            .expect("identity");
        let dir = devlanes_daemon::gui::viewer::instance_dir(&home);
        let key = devlanes_gui_client::instance::key("acme@1:desktop", 26001);
        let info = devlanes_gui_client::instance::Info {
            pid,
            start_time: identity.start_time,
            started_by: devlanes_gui_client::instance::StartedBy::Daemon,
            service: "acme@1:desktop".into(),
            host_box: None,
        };
        // This test holds the lock in the viewer's place.
        let _lock = devlanes_gui_client::instance::claim(&dir, &key, &info).expect("claim");
        let program = tmp.path.join("bin").join("devlanes-gui");
        let setup = || Setup {
            auto_open: true,
            ..Setup::default()
        };
        for restart in [true, false] {
            let h = gui_harness_at(&home, "17350-17399", None, program.clone(), setup()).await;
            let mut shutdown = crate::signal_shutdown();
            shutdown.restart = restart;
            let _ = devlanes_daemon::Handler::shutdown(&h.engine, shutdown).await;
            if restart {
                assert!(
                    devlanes_sys::matches(&identity).unwrap_or(false),
                    "a restart leaves the viewer open"
                );
            } else {
                let ended = eventually(Duration::from_secs(10), || async {
                    (!devlanes_sys::matches(&identity).unwrap_or(true)).then_some(())
                })
                .await;
                assert!(ended.is_some(), "a stop ends the viewer");
            }
        }
        let _ = viewer.wait().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// On a machine without a GUI backend a GUI service runs as before, without
/// a host, and says so; turning `gui` off later is drift.
async fn without_a_backend_a_gui_service_runs_without_a_host() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = gui_harness(&tmp, "17300-17349", None, true).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        assert!(h.runner.hosts().is_empty());
        let desktop = unit(&h, &main, "desktop").await;
        assert_eq!(desktop.state, "ready");
        assert_eq!(desktop.gui.map(|gui| gui.host), Some("none".into()));
        let (_, notes) = explain(&h, &main, "desktop").await;
        assert!(
            notes.iter().any(|note| note.contains("no GUI host")),
            "{notes:?}"
        );
        let reasons = why(&h, &main, "desktop").await;
        assert!(
            reasons
                .iter()
                .any(|reason| reason["code"] == "gui_no_backend"),
            "{reasons:?}"
        );
        std::fs::write(
            main.join("devlanes.yaml"),
            CONFIG.replace(", gui: true", ""),
        )
        .expect("edit");
        let drift = unit(&h, &main, "desktop").await.drift.expect("drift");
        assert!(drift.fields.contains(&"gui".to_owned()), "{drift:?}");
        h.close().await;
    })
    .await
    .expect("timeout");
}
