//! Daemon shutdown: an explicit one (`daemon stop`, `daemon restart`) runs the
//! `lane.stop` tasks of lanes with live units before their units stop; a signal
//! stops the units without them.

use std::time::Duration;

use devlanes_daemon::{Handler, Shutdown, ShutdownReason};
use devlanes_proto::{DaemonStopping, Event, Method, StoppingState, StoppingStep};
use serde_json::{Value, json};

use crate::{LIMIT, Setup, cx, eventually, harness, repo, temp};

/// One service and two stop tasks, the first of which fails.
const CONFIG: &str = "project: bye\nservices:\n  api: { run: sleep }\ntasks:\n  first: { run: fail, when: lane.stop }\n  second: { run: echo stop-hook, when: lane.stop }\n";

/// How often `script` was spawned.
fn spawned(journal: &[String], script: &str) -> usize {
    journal
        .iter()
        .filter(|entry| entry.as_str() == script)
        .count()
}

#[tokio::test(flavor = "multi_thread")]
/// With `idle_exit`, the engine goes idle only after a subscriber came and
/// left, and only while nothing runs for the whole grace period.
async fn the_engine_goes_idle_after_the_last_subscriber_with_nothing_running() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(
            &tmp.path,
            "r",
            "project: quiet\nservices:\n  api: { run: sleep }\n",
        );
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                idle_exit: Some(Duration::from_millis(200)),
                ..Setup::default()
            },
        )
        .await;
        let wait = Duration::from_millis(800);
        // Never watched: a daemon only commands used waits ten grace periods.
        assert!(tokio::time::timeout(wait, h.engine.idle()).await.is_err());
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        // A TUI watches while a unit runs, then leaves: still not idle.
        let tui = h.engine.subscribe();
        assert!(tokio::time::timeout(wait, h.engine.idle()).await.is_err());
        drop(tui);
        assert!(tokio::time::timeout(wait, h.engine.idle()).await.is_err());
        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop");
        tokio::time::timeout(Duration::from_secs(5), h.engine.idle())
            .await
            .expect("idle once the lane stopped");
        // A subscriber keeps it up again.
        let _watching = h.engine.subscribe();
        assert!(tokio::time::timeout(wait, h.engine.idle()).await.is_err());
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A daemon no subscriber ever watched goes idle after
/// `UNWATCHED_IDLE_FACTOR` grace periods of quiet, never while a unit runs.
async fn an_unwatched_engine_goes_idle_after_ten_grace_periods() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(
            &tmp.path,
            "r",
            "project: quiet\nservices:\n  api: { run: sleep }\n",
        );
        let grace = Duration::from_millis(100);
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                idle_exit: Some(grace),
                ..Setup::default()
            },
        )
        .await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let unwatched = grace * devlanes_daemon::UNWATCHED_IDLE_FACTOR;
        assert!(
            tokio::time::timeout(unwatched * 2, h.engine.idle())
                .await
                .is_err(),
            "a running unit keeps it up"
        );
        h.call(Method::Stop, json!({}), cx(&main))
            .await
            .expect("stop");
        let begun = std::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(10), h.engine.idle())
            .await
            .expect("idle once quiet long enough");
        assert!(begun.elapsed() >= unwatched, "{:?}", begun.elapsed());
        h.close().await;
    })
    .await
    .expect("timeout");
}

/// One `daemon.stopping` event as (step, state, total, left, lanes).
type StepRow = (
    StoppingStep,
    StoppingState,
    Option<u64>,
    Option<u64>,
    Option<u64>,
);

/// The `daemon.stopping` events in `events`, as [`StepRow`]s.
fn steps(events: &[Event]) -> Vec<StepRow> {
    stopping(events)
        .iter()
        .map(|step| (step.step, step.state, step.total, step.left, step.lanes))
        .collect()
}

/// The `daemon.stopping` payloads in `events`.
fn stopping(events: &[Event]) -> Vec<DaemonStopping> {
    events
        .iter()
        .filter(|event| event.event == devlanes_proto::events::DAEMON_STOPPING)
        .map(|event| serde_json::from_value(event.data.clone()).expect("payload"))
        .collect()
}

/// Everything `bus` received so far.
fn received(bus: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<Event> {
    std::iter::from_fn(|| bus.try_recv().ok()).collect()
}

#[tokio::test(flavor = "multi_thread")]
/// An explicit restart reports each step in order with its counts: steps with
/// nothing to do are skipped, the units count down, and every event carries
/// the reason and the restart.
async fn shutdown_reports_its_steps() {
    tokio::time::timeout(LIMIT, async {
        use StoppingState::{Done, Progress, Skipped, Started};
        use StoppingStep::{Remote, State, Tasks, Thaw, Units};
        let tmp = temp();
        let config = "project: bye\nservices:\n  api: { run: sleep }\n  web: { run: sleep }\ntasks:\n  note: { run: echo stop-hook, when: lane.stop }\n";
        let main = repo(&tmp.path, "r", config);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let mut bus = h.engine.subscribe();
        let mut request = Shutdown::new(ShutdownReason::Explicit);
        request.restart = true;
        h.engine.shutdown(request).await.expect("shutdown");
        let events = received(&mut bus);
        assert_eq!(
            steps(&events),
            [
                (Remote, Skipped, None, None, None),
                (Thaw, Skipped, None, None, None),
                (Tasks, Started, Some(1), None, None),
                (Tasks, Done, Some(1), None, None),
                (Units, Started, Some(2), None, Some(1)),
                (Units, Progress, Some(2), Some(1), Some(1)),
                (Units, Progress, Some(2), Some(0), Some(1)),
                (Units, Done, Some(2), None, Some(1)),
                (State, Started, None, None, None),
                (State, Done, None, None, None),
            ]
        );
        let payloads = stopping(&events);
        assert!(
            payloads
                .iter()
                .all(|step| step.reason == "explicit" && step.restart)
        );
        assert!(
            payloads
                .iter()
                .all(|step| (step.state == Done) == step.elapsed_ms.is_some())
        );
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A signal shutdown with nothing running skips every step but `state`, and
/// says it is no restart.
async fn an_empty_signal_shutdown_skips_its_steps() {
    tokio::time::timeout(LIMIT, async {
        use StoppingState::{Done, Skipped, Started};
        use StoppingStep::{Remote, State, Tasks, Thaw, Units};
        let tmp = temp();
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let mut bus = h.engine.subscribe();
        h.engine
            .shutdown(crate::signal_shutdown())
            .await
            .expect("shutdown");
        let events = received(&mut bus);
        assert_eq!(
            steps(&events),
            [
                (Remote, Skipped, None, None, None),
                (Thaw, Skipped, None, None, None),
                (Tasks, Skipped, None, None, None),
                (Units, Skipped, None, None, None),
                (State, Started, None, None, None),
                (State, Done, None, None, None),
            ]
        );
        assert!(
            stopping(&events)
                .iter()
                .all(|step| step.reason == "signal" && !step.restart)
        );
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// Every stop task runs, a failing one included, and the report names the
/// stopped units and each task's outcome.
async fn explicit_shutdown_runs_lane_stop_tasks() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let report = h
            .engine
            .shutdown(Shutdown::new(ShutdownReason::Explicit))
            .await
            .expect("shutdown");
        assert_eq!(report["reason"], "explicit", "{report}");
        assert_eq!(
            report["stopped"],
            json!([{"lane": "bye@0", "unit": "api"}]),
            "{report}"
        );
        assert_eq!(
            report["tasks"],
            json!([
                {"lane": "bye@0", "task": "first", "exit_code": 1},
                {"lane": "bye@0", "task": "second", "exit_code": 0},
            ]),
            "{report}"
        );
        assert_eq!(spawned(&h.runner.journal.entries(), "echo stop-hook"), 1);
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A signal shutdown stops the units and runs no stop task.
async fn signal_shutdown_skips_lane_stop_tasks() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let report = h
            .engine
            .shutdown(crate::signal_shutdown())
            .await
            .expect("shutdown");
        assert_eq!(report["reason"], "signal", "{report}");
        assert_eq!(
            report["stopped"],
            json!([{"lane": "bye@0", "unit": "api"}]),
            "{report}"
        );
        assert_eq!(report["tasks"], json!([]), "{report}");
        let journal = h.runner.journal.entries();
        assert_eq!(spawned(&journal, "fail"), 0, "{journal:?}");
        assert_eq!(spawned(&journal, "echo stop-hook"), 0, "{journal:?}");
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A lane with nothing running gets no stop tasks at an explicit shutdown.
async fn explicit_shutdown_skips_idle_lanes() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", CONFIG);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        // A named stop runs no stop task and leaves the lane idle.
        h.call(Method::Stop, json!({"units": ["api"]}), cx(&main))
            .await
            .expect("stop api");
        let report = h
            .engine
            .shutdown(Shutdown::new(ShutdownReason::Explicit))
            .await
            .expect("shutdown");
        assert_eq!(report["stopped"], json!([]), "{report}");
        assert_eq!(report["tasks"], json!([]), "{report}");
        assert_eq!(spawned(&h.runner.journal.entries(), "echo stop-hook"), 0);
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// `doctor` lists each service's stop timeout, for the CLI to compare with the
/// service manager's stop window: written, defaulted, and an expression once a
/// run resolved it.
async fn doctor_lists_stop_timeouts() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = "project: slow\nservices:\n  api: { run: sleep, stop: { timeout: 90s } }\n  web: { run: sleep }\n  worker: { run: sleep, autostart: false, stop: { timeout: \"${{ lane.index }}s\" } }\n";
        let main = repo(&tmp.path, "r", config);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let doctor = h
            .call(Method::Doctor, json!({}), cx(&main))
            .await
            .expect("doctor");
        assert_eq!(
            doctor["stop_timeouts"],
            json!([
                {"project": "slow", "unit": "api", "timeout_ms": 90_000},
                {"project": "slow", "unit": "web", "timeout_ms": 10_000},
            ]),
            "{doctor}"
        );
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A signal during an explicit shutdown interrupts the stop task that runs,
/// skips the rest, and the units still stop.
async fn interrupt_ends_lane_stop_tasks() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let config = "project: bye\nservices:\n  api: { run: sleep }\ntasks:\n  stuck: { run: hold, when: lane.stop }\n  after: { run: echo after, when: lane.stop }\n";
        let main = repo(&tmp.path, "r", config);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start");
        let request = Shutdown::new(ShutdownReason::Explicit);
        let interrupt = request.interrupt.clone();
        let engine = h.engine.clone();
        let shutdown = tokio::spawn(async move { engine.shutdown(request).await });
        let journal = h.runner.journal.clone();
        eventually(Duration::from_secs(10), || {
            let held = spawned(&journal.entries(), "hold") == 1;
            async move { held.then_some(()) }
        })
        .await
        .expect("the stop task runs");
        interrupt.cancel();
        let report: Value = tokio::time::timeout(Duration::from_secs(20), shutdown)
            .await
            .expect("the shutdown ends")
            .expect("join")
            .expect("shutdown");
        assert_eq!(
            report["stopped"],
            json!([{"lane": "bye@0", "unit": "api"}]),
            "{report}"
        );
        assert_eq!(report["tasks"][0]["task"], "stuck", "{report}");
        assert_eq!(report["tasks"][0]["exit_code"], Value::Null, "{report}");
        assert_eq!(report["tasks"][1]["task"], "after", "{report}");
        assert_eq!(report["tasks"][1]["error"]["code"], "cancelled", "{report}");
        assert_eq!(spawned(&h.runner.journal.entries(), "echo after"), 0);
    })
    .await
    .expect("timeout");
}
