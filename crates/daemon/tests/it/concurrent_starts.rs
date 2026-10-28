//! Several starts on one lane: a start that adds units never waits for another
//! such start, shows its units `queued` from the moment it arrives, and joins a
//! bring-up in progress instead of running it again.

use std::{sync::Arc, time::Duration};

use devlanes_core::{
    clock::TokioClock,
    gate::{StartGate, StartRequest},
};
use devlanes_daemon::Session;
use devlanes_ipc::Frame;
use devlanes_proto::{Context, LaneSummary, Method, Page, Request, RpcError, StartResult};
use devlanes_start_gate::{FixedWindow, GateOptions};
use serde_json::{Value, json};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{Harness, LIMIT, Setup, cx, eventually, harness, repo, temp};

/// A request on its own session in the background; keep the frames alive.
fn spawn_call(
    h: &Harness,
    method: Method,
    params: Value,
    context: Context,
) -> (
    Session,
    mpsc::Receiver<Frame>,
    JoinHandle<Result<Value, RpcError>>,
) {
    let (session, frames) = h.session();
    let engine = h.engine.clone();
    let running = session.clone();
    let task = tokio::spawn(async move {
        let request = Request {
            id: 1,
            method,
            params,
            context,
        };
        devlanes_daemon::Handler::handle(&engine, request, running).await
    });
    (session, frames, task)
}

/// The result of a background start; the limit only bounds a failing test.
async fn start_result(task: JoinHandle<Result<Value, RpcError>>) -> StartResult {
    let value = tokio::time::timeout(Duration::from_secs(30), task)
        .await
        .expect("the start answers")
        .expect("join")
        .expect("start");
    serde_json::from_value(value).expect("start result")
}

/// The lane visible at `cwd`.
async fn lane(h: &Harness, cwd: &std::path::Path) -> LaneSummary {
    let value = h
        .call(Method::Status, json!({}), cx(cwd))
        .await
        .expect("status");
    let page: Page<LaneSummary> = serde_json::from_value(value).expect("page");
    page.items.into_iter().next().expect("lane")
}

/// The state of `unit` and its `queued` reason data, if any.
async fn unit_state(h: &Harness, cwd: &std::path::Path, unit: &str) -> (String, Value) {
    let summary = lane(h, cwd).await;
    let unit = summary
        .units
        .iter()
        .find(|u| u.name == unit)
        .expect("unit listed");
    let data = unit
        .reason
        .as_ref()
        .filter(|reason| reason.code == "queued")
        .map(|reason| reason.data.clone())
        .unwrap_or(Value::Null);
    (unit.state.clone(), data)
}

/// How many spawned scripts start with `prefix`.
fn spawned(h: &Harness, prefix: &str) -> usize {
    h.runner
        .journal
        .entries()
        .iter()
        .filter(|script| script.starts_with(prefix))
        .count()
}

const FOUR: &str = r#"project: four
profiles:
  default: [a]
services:
  a: { run: "echo a; sleep", ready: started }
  b: { run: "echo b; sleep", ready: started }
  c: { run: "echo c; sleep", ready: started }
  d: { run: "echo d; sleep", ready: started }
"#;

#[tokio::test(flavor = "multi_thread")]
/// A start of one unit waiting in the start gate does not hold up another
/// start of the same lane: the second spawns while the first still waits.
async fn a_start_waiting_for_a_slot_does_not_hold_up_the_lane() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", FOUR);
        let gate = Arc::new(FixedWindow::new(
            1,
            GateOptions::default(),
            Arc::new(TokioClock::new()),
        ));
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                gate: Some(gate.clone()),
                ..Setup::default()
            },
        )
        .await;
        let first: StartResult = serde_json::from_value(
            h.call(Method::Start, json!({}), cx(&main))
                .await
                .expect("start a"),
        )
        .expect("result");
        assert_eq!(first.started, ["four@0:a"]);

        // Every slot taken: a start of `b` waits in the gate.
        let holder = gate
            .acquire(StartRequest {
                lane: "elsewhere@0".into(),
                unit: "holder".into(),
                order: 0,
                memory_need: 0,
            })
            .await;
        let (_session, _frames, waiting) = spawn_call(
            &h,
            Method::Start,
            json!({"units": ["b"], "timeout_ms": 20_000}),
            cx(&main),
        );
        let queued = eventually(Duration::from_secs(5), || async {
            (gate.snapshot().queued.len() == 1).then_some(())
        })
        .await;
        assert!(queued.is_some(), "b waits for a slot");

        // A start that skips the gate goes ahead at once.
        let other = tokio::time::timeout(
            Duration::from_secs(5),
            h.call(
                Method::Start,
                json!({"units": ["c"], "all_at_once": true}),
                cx(&main),
            ),
        )
        .await
        .expect("the start of c does not wait for the start of b")
        .expect("start c");
        let other: StartResult = serde_json::from_value(other).expect("result");
        assert_eq!(other.started, ["four@0:c"], "{other:?}");
        assert_eq!(spawned(&h, "echo b"), 0, "b still waits");

        drop(holder);
        let result = start_result(waiting).await;
        assert_eq!(result.started, ["four@0:b"], "{result:?}");
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A unit reads `queued` as soon as its start arrives, even while the start
/// still waits for its turn.
async fn a_requested_unit_reads_queued_at_once() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", FOUR);
        let gate = Arc::new(FixedWindow::new(
            1,
            GateOptions::default(),
            Arc::new(TokioClock::new()),
        ));
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                gate: Some(gate.clone()),
                ..Setup::default()
            },
        )
        .await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start a");
        let holder = gate
            .acquire(StartRequest {
                lane: "elsewhere@0".into(),
                unit: "holder".into(),
                order: 0,
                memory_need: 0,
            })
            .await;
        let (_session, _frames, waiting) = spawn_call(
            &h,
            Method::Start,
            json!({"units": ["d"], "timeout_ms": 20_000}),
            cx(&main),
        );
        let queued = eventually(Duration::from_secs(5), || async {
            let (state, _) = unit_state(&h, &main, "d").await;
            (state == "queued").then_some(())
        })
        .await;
        assert!(queued.is_some(), "d reads queued while it waits");

        // A second start of the queued unit joins the first instead of
        // replacing it in the gate's line.
        let (_again_session, _again_frames, again) = spawn_call(
            &h,
            Method::Start,
            json!({"units": ["d"], "timeout_ms": 20_000}),
            cx(&main),
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(gate.snapshot().queued.len(), 1, "d is in line once");

        drop(holder);
        let first = start_result(waiting).await;
        let second = start_result(again).await;
        assert_eq!(first.started, ["four@0:d"], "{first:?}");
        assert!(second.started.is_empty(), "{second:?}");
        assert_eq!(second.already_running, ["four@0:d"], "{second:?}");
        assert!(first.ready && second.ready);
        assert_eq!(spawned(&h, "echo d"), 1);
        h.close().await;
    })
    .await
    .expect("timeout");
}

#[tokio::test(flavor = "multi_thread")]
/// A start that joined another start's unit and runs out of time before that
/// unit spawned reports a timeout, never a ready lane without the unit.
async fn a_joined_unit_past_the_deadline_times_out() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", FOUR);
        let gate = Arc::new(FixedWindow::new(
            1,
            GateOptions::default(),
            Arc::new(TokioClock::new()),
        ));
        let h = harness(
            &tmp.path.join("home"),
            Setup {
                gate: Some(gate.clone()),
                ..Setup::default()
            },
        )
        .await;
        h.call(Method::Start, json!({}), cx(&main))
            .await
            .expect("start a");
        let holder = gate
            .acquire(StartRequest {
                lane: "elsewhere@0".into(),
                unit: "holder".into(),
                order: 0,
                memory_need: 0,
            })
            .await;
        let (_session, _frames, owner) = spawn_call(
            &h,
            Method::Start,
            json!({"units": ["d"], "timeout_ms": 20_000}),
            cx(&main),
        );
        let queued = eventually(Duration::from_secs(5), || async {
            (gate.snapshot().queued.len() == 1).then_some(())
        })
        .await;
        assert!(queued.is_some(), "d waits for a slot");

        let joined = tokio::time::timeout(
            Duration::from_secs(10),
            h.call(
                Method::Start,
                json!({"units": ["d"], "timeout_ms": 500}),
                cx(&main),
            ),
        )
        .await
        .expect("the joined start answers at its deadline")
        .expect("start");
        let joined: StartResult = serde_json::from_value(joined).expect("result");
        assert!(joined.timed_out && !joined.ready, "{joined:?}");

        drop(holder);
        let owned = start_result(owner).await;
        assert_eq!(owned.started, ["four@0:d"], "{owned:?}");
        h.close().await;
    })
    .await
    .expect("timeout");
}

const TASKED: &str = r#"project: tasked
profiles:
  default: [web]
tasks:
  pause: { run: "nap 1500", when: lane.start }
services:
  web: { run: "echo web; sleep", ready: started }
  extra: { run: "echo extra; sleep", ready: started }
"#;

#[tokio::test(flavor = "multi_thread")]
/// A start that arrives while the lane is being brought up says what it waits
/// for, joins that bring-up instead of running its tasks again, and spawns
/// once the bring-up reached its services.
async fn a_start_during_the_bring_up_joins_it() {
    tokio::time::timeout(LIMIT, async {
        let tmp = temp();
        let main = repo(&tmp.path, "r", TASKED);
        let h = harness(&tmp.path.join("home"), Setup::default()).await;
        let (_session, _frames, bringing) =
            spawn_call(&h, Method::Start, json!({"timeout_ms": 20_000}), cx(&main));
        let running = eventually(Duration::from_secs(10), || async {
            (spawned(&h, "nap") == 1).then_some(())
        })
        .await;
        assert!(running.is_some(), "the lane.start task runs");

        let (_extra_session, _extra_frames, joining) = spawn_call(
            &h,
            Method::Start,
            json!({"units": ["extra"], "timeout_ms": 20_000}),
            cx(&main),
        );
        let waiting = eventually(Duration::from_secs(5), || async {
            let (state, data) = unit_state(&h, &main, "extra").await;
            (state == "queued").then_some(data)
        })
        .await
        .expect("extra reads queued while the bring-up runs");
        assert_eq!(waiting["waiting_for"], "pause", "{waiting}");
        assert_eq!(spawned(&h, "echo extra"), 0, "extra waits for the task");

        let brought = start_result(bringing).await;
        let joined = start_result(joining).await;
        assert!(brought.whole_lane && brought.ready, "{brought:?}");
        assert!(!joined.whole_lane && joined.ready, "{joined:?}");
        assert_eq!(joined.started, ["tasked@0:extra"], "{joined:?}");
        assert_eq!(spawned(&h, "nap"), 1, "the task ran once");
        h.close().await;
    })
    .await
    .expect("timeout");
}

const INFRA: &str = r#"project: infra
infra:
  containers:
    db: { image: "postgres:16", ports: { pg: 5432 }, per_lane: true }
services:
  api: { run: "echo api; sleep", ready: started }
  extra: { run: "echo extra; sleep", ready: started }
profiles:
  default: [api]
"#;

#[tokio::test(flavor = "multi_thread")]
/// Adding a unit to a lane that is up asks the infra provider nothing: the
/// bring-up's namespace and setup check still hold.
async fn adding_a_unit_to_a_running_lane_asks_infra_nothing() {
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
        let first: StartResult = serde_json::from_value(
            h.call(Method::Start, json!({}), cx(&main))
                .await
                .expect("start"),
        )
        .expect("result");
        assert!(first.ready, "{first:?}");
        let calls = h.infra.calls.entries();
        let queries = h.infra.queries.entries();

        let added: StartResult = serde_json::from_value(
            h.call(Method::Start, json!({"units": ["extra"]}), cx(&main))
                .await
                .expect("start extra"),
        )
        .expect("result");
        assert_eq!(added.started, ["infra@0:extra"], "{added:?}");
        assert_eq!(h.infra.calls.entries(), calls);
        assert_eq!(h.infra.queries.entries(), queries);
        h.close().await;
    })
    .await
    .expect("timeout");
}
