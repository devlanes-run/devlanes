//! `s` shows its units `queued` at once, and queued rows say what they wait for.

use crossterm::event::KeyCode;
use devlanes_proto::{Event, Method, RpcError, UnitSummary, events};
use devlanes_tui::{
    ApiError,
    app::{App, Effect, Msg, RowId, Section, Selected, Target},
    ui,
};
use serde_json::{Value, json};

use crate::errors::{render, row};
use crate::fake::{FakeApi, lane, mixed_lanes, unit};
use crate::{key, loaded, select};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A subscription event.
fn event(name: &str, data: Value) -> Msg {
    Msg::Event(Event {
        seq: 2,
        event: name.into(),
        data,
    })
}

/// The state `acme@0:<name>` shows now.
fn state(app: &App, name: &str) -> Option<String> {
    app.lane("acme@0")?
        .units
        .iter()
        .find(|u| u.name == name)
        .map(|u| u.state.clone())
}

/// The list's row for unit `name`: the left box's part of the screen row.
fn drawn_row(app: &App, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let buffer = render(app)?;
    (0..buffer.area.height)
        .filter_map(|y| row(&buffer, y).split('│').nth(1).map(str::to_owned))
        .find(|line| line.contains(&format!(" {name} ")))
        .ok_or_else(|| format!("no row for {name}").into())
}

/// `acme@0:worker` (failed in the fake) selected, after `s` on it.
async fn started_worker() -> Result<(App, Vec<Effect>), Box<dyn std::error::Error>> {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 100, 24).await;
    select(&mut app, Selected::unit("acme@0", "worker"));
    let effects = app.update(key(KeyCode::Char('s')));
    Ok((app, effects))
}

/// The id of the only control effect in `effects`.
fn action(effects: &[Effect]) -> Result<u64, String> {
    match effects {
        [Effect::Control { action, .. }] => Ok(*action),
        _ => Err(format!("not one control effect: {effects:?}")),
    }
}

/// The answer to control effect `action` on `acme@0:worker`.
fn worker_done(action: u64, label: &str, result: Result<Value, ApiError>) -> Msg {
    Msg::ActionDone {
        action,
        label: label.into(),
        target: Target::Unit {
            lane: "acme@0".into(),
            unit: "worker".into(),
        },
        units: vec!["acme@0:worker".into()],
        result,
    }
}

#[tokio::test]
async fn s_shows_the_unit_queued_before_the_daemon_says_anything() -> TestResult {
    let (app, effects) = started_worker().await?;
    assert!(
        matches!(effects.as_slice(), [Effect::Control { method: Method::Start, selection, .. }]
            if selection.units == ["acme@0:worker"]),
        "{effects:?}"
    );
    assert_eq!(state(&app, "worker").as_deref(), Some("queued"));
    let line = drawn_row(&app, "worker")?;
    assert!(line.contains("queued"), "{line}");
    assert!(!line.contains("failed"), "{line}");
    Ok(())
}

#[tokio::test]
async fn s_on_a_section_shows_every_unit_it_sends_queued() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 100, 24).await;
    select(
        &mut app,
        Selected {
            lane: "acme@0".into(),
            row: RowId::Section(Section::Services),
        },
    );
    let effects = app.update(key(KeyCode::Char('s')));
    assert!(
        matches!(effects.as_slice(), [Effect::Control { selection, .. }]
            if selection.units == ["acme@0:worker"]),
        "{effects:?}"
    );
    assert_eq!(state(&app, "worker").as_deref(), Some("queued"));
    // What already ran keeps its state.
    assert_eq!(state(&app, "api").as_deref(), Some("ready"));
    // Everything is running or queued now: a second `s` sends nothing.
    assert!(app.update(key(KeyCode::Char('s'))).is_empty());
    assert!(
        app.notice
            .as_ref()
            .is_some_and(|n| n.text == "everything here is already running or queued"),
        "{:?}",
        app.notice
    );
    Ok(())
}

#[tokio::test]
async fn the_daemon_replaces_the_local_queued_once_it_reports_the_unit() -> TestResult {
    let (mut app, _) = started_worker().await?;
    // A lane snapshot taken before the request arrived still says `failed`.
    let before = mixed_lanes().remove(0);
    app.update(event(events::LANE_CHANGED, json!({ "lane": before })));
    assert_eq!(state(&app, "worker").as_deref(), Some("queued"));
    // So does a status reply sent before it.
    app.update(Msg::Status(Ok(mixed_lanes())));
    assert_eq!(state(&app, "worker").as_deref(), Some("queued"));
    // The daemon's own `queued`, with what the unit waits for.
    let mut waiting = unit("acme@0", "worker", "queued");
    waiting.reason = Some(RpcError::new(
        "queued",
        json!({"waiting_for": "containers"}),
        0,
    ));
    let mut lane0 = mixed_lanes().remove(0);
    for unit in &mut lane0.units {
        if unit.name == "worker" {
            *unit = waiting.clone();
        }
    }
    app.update(event(events::LANE_CHANGED, json!({ "lane": lane0 })));
    // Wide enough for the list to show the detail; narrower lists drop it first.
    app.update(Msg::Resize(160, 30));
    let line = drawn_row(&app, "worker")?;
    assert!(line.contains("queued · waiting for containers"), "{line}");
    // From then on the daemon's word counts, even when it says `failed` again.
    let mut failed = unit("acme@0", "worker", "failed");
    failed.exit_code = Some(1);
    app.update(event(events::UNIT_CHANGED, json!({ "unit": failed })));
    app.update(Msg::Status(Ok(mixed_lanes())));
    assert_eq!(state(&app, "worker").as_deref(), Some("failed"));
    Ok(())
}

#[tokio::test]
async fn a_unit_event_ends_the_local_queued() -> TestResult {
    let (mut app, _) = started_worker().await?;
    app.update(event(
        events::UNIT_CHANGED,
        json!({ "unit": unit("acme@0", "worker", "starting") }),
    ));
    assert_eq!(state(&app, "worker").as_deref(), Some("starting"));
    Ok(())
}

#[tokio::test]
async fn s_on_a_queued_unit_sends_nothing() -> TestResult {
    let (mut app, _) = started_worker().await?;
    assert!(app.update(key(KeyCode::Char('s'))).is_empty());
    assert_eq!(
        app.notice.as_ref().map(|n| n.text.as_str()),
        Some("acme@0:worker is already queued")
    );
    // Queued by the daemon, the same.
    app.notice = None;
    app.update(event(
        events::UNIT_CHANGED,
        json!({ "unit": unit("acme@0", "worker", "queued") }),
    ));
    assert!(app.update(key(KeyCode::Char('s'))).is_empty());
    assert_eq!(
        app.notice.as_ref().map(|n| n.text.as_str()),
        Some("acme@0:worker is already queued")
    );
    Ok(())
}

#[tokio::test]
async fn a_refused_start_puts_back_the_last_state_and_reports_it() -> TestResult {
    let (mut app, effects) = started_worker().await?;
    let target = Target::Unit {
        lane: "acme@0".into(),
        unit: "worker".into(),
    };
    app.update(worker_done(
        action(&effects)?,
        "start acme@0:worker",
        Err(ApiError::Rpc(RpcError::new(
            "no_config",
            json!({"lane": "acme@0"}),
            3,
        ))),
    ));
    assert_eq!(state(&app, "worker").as_deref(), Some("failed"));
    assert!(app.target_error(&target).is_some());
    // A status reply that still says `failed` no longer turns it `queued`.
    app.update(Msg::Status(Ok(mixed_lanes())));
    assert_eq!(state(&app, "worker").as_deref(), Some("failed"));
    // The next `s` sends a start again.
    assert_eq!(app.update(key(KeyCode::Char('s'))).len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_answered_start_asks_for_status_when_no_event_came() -> TestResult {
    let (mut app, effects) = started_worker().await?;
    let effects = app.update(worker_done(
        action(&effects)?,
        "start acme@0:worker",
        Ok(json!({})),
    ));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Refresh)),
        "{effects:?}"
    );
    Ok(())
}

/// [`mixed_lanes`] with a running exec unit `acme@0:shell`, which the
/// services section lists in its `ad-hoc` group.
fn lanes_with_shell() -> Vec<devlanes_proto::LaneSummary> {
    let mut lanes = mixed_lanes();
    let mut shell = unit("acme@0", "shell", "running");
    shell.kind = "exec".into();
    lanes[0].units.push(shell);
    lanes
}

#[tokio::test]
async fn other_actions_leave_a_pending_start_queued() -> TestResult {
    let api = FakeApi::with_lanes(lanes_with_shell());
    let (mut app, _) = loaded(&api, 100, 24).await;
    select(&mut app, Selected::unit("acme@0", "worker"));
    let start = action(&app.update(key(KeyCode::Char('s'))))?;
    // `x` on a group without the worker stops only what that group runs.
    select(
        &mut app,
        Selected {
            lane: "acme@0".into(),
            row: RowId::Group(Section::Services, "ad-hoc".into()),
        },
    );
    let effects = app.update(key(KeyCode::Char('x')));
    assert!(
        matches!(effects.as_slice(), [Effect::Control { method: Method::Stop, selection, .. }]
            if selection.units == ["acme@0:shell"]),
        "{effects:?}"
    );
    let stop = action(&effects)?;
    assert_eq!(state(&app, "worker").as_deref(), Some("queued"));
    // Neither its answer nor that of another lane-wide start ends the worker's `queued`.
    for (label, id, units) in [
        ("stop acme@0:shell", stop, vec!["acme@0:shell".to_owned()]),
        (
            "start 2 units in acme@0",
            stop + 1,
            vec!["acme@0:web".to_owned()],
        ),
        ("start acme@0", stop + 2, Vec::new()),
    ] {
        app.update(Msg::ActionDone {
            action: id,
            label: label.into(),
            target: Target::Lane("acme@0".into()),
            units,
            result: Ok(json!({})),
        });
        assert_eq!(state(&app, "worker").as_deref(), Some("queued"), "{label}");
    }
    // The worker's own answer does.
    app.update(worker_done(start, "start acme@0:worker", Ok(json!({}))));
    assert_eq!(state(&app, "worker").as_deref(), Some("failed"));
    Ok(())
}

#[tokio::test]
async fn x_on_the_lane_ends_every_local_queued() -> TestResult {
    let (mut app, _) = started_worker().await?;
    select(
        &mut app,
        Selected {
            lane: "acme@0".into(),
            row: RowId::WholeLane,
        },
    );
    app.update(key(KeyCode::Char('x')));
    assert_eq!(state(&app, "worker").as_deref(), Some("failed"));
    Ok(())
}

/// The lane pane's row for `name` under `services  N of M up`.
fn pane_row(app: &App, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let buffer = render(app)?;
    (0..buffer.area.height)
        .filter_map(|y| row(&buffer, y).split('│').nth(3).map(str::to_owned))
        .find(|line| line.contains(&format!(" {name} ")))
        .ok_or_else(|| format!("no pane row for {name}").into())
}

#[tokio::test]
async fn the_lane_pane_drops_a_queued_detail_that_does_not_fit() -> TestResult {
    let mut api = unit("acme@0", "api", "queued");
    api.reason = Some(RpcError::new(
        "queued",
        json!({"position": 1, "queued": 2, "in_flight": 4, "window": 4, "hold": "window_full"}),
        0,
    ));
    let starting = lane("acme", Some(0), "main", "starting", vec![api]);
    let fake = FakeApi::with_lanes(vec![starting]);
    let (mut app, _) = loaded(&fake, 80, 24).await;
    select(
        &mut app,
        Selected {
            lane: "acme@0".into(),
            row: RowId::WholeLane,
        },
    );
    let line = pane_row(&app, "api")?;
    assert!(line.trim_end().ends_with("queued"), "{line}");
    assert!(!line.contains("next"), "{line}");
    app.update(Msg::Resize(140, 30));
    let line = pane_row(&app, "api")?;
    assert!(line.contains("queued, next in line"), "{line}");
    Ok(())
}

#[tokio::test]
async fn stop_and_restart_still_reach_a_unit_started_from_here() -> TestResult {
    let (mut app, _) = started_worker().await?;
    // Stopped before the daemon reported anything: the stop goes out, and the
    // row goes back to what the daemon said last until it reports again.
    let effects = app.update(key(KeyCode::Char('x')));
    assert!(
        matches!(effects.as_slice(), [Effect::Control { method: Method::Stop, selection, .. }]
            if selection.units == ["acme@0:worker"]),
        "{effects:?}"
    );
    assert_eq!(state(&app, "worker").as_deref(), Some("failed"));
    // Started again, then running: restart and stop work as on any unit.
    app.update(key(KeyCode::Char('s')));
    app.update(event(
        events::UNIT_CHANGED,
        json!({ "unit": unit("acme@0", "worker", "ready") }),
    ));
    let effects = app.update(key(KeyCode::Char('r')));
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Control {
                method: Method::Restart,
                ..
            }]
        ),
        "{effects:?}"
    );
    let effects = app.update(key(KeyCode::Char('x')));
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Control {
                method: Method::Stop,
                ..
            }]
        ),
        "{effects:?}"
    );
    assert_eq!(state(&app, "worker").as_deref(), Some("ready"));
    Ok(())
}

#[tokio::test]
async fn a_late_answer_to_an_earlier_start_leaves_the_next_start_queued() -> TestResult {
    let (mut app, effects) = started_worker().await?;
    let first = action(&effects)?;
    // `x` before the daemon answers puts back `failed`; `s` starts it again.
    let stop = action(&app.update(key(KeyCode::Char('x'))))?;
    assert_eq!(state(&app, "worker").as_deref(), Some("failed"));
    let second = action(&app.update(key(KeyCode::Char('s'))))?;
    assert!(first < stop && stop < second, "{first} {stop} {second}");
    assert_eq!(state(&app, "worker").as_deref(), Some("queued"));
    // The runtime runs each control call in its own task, so the first start
    // and the stop can answer while the second start is still in flight.
    for (id, label) in [(first, "start acme@0:worker"), (stop, "stop acme@0:worker")] {
        app.update(worker_done(id, label, Ok(json!({}))));
        assert_eq!(state(&app, "worker").as_deref(), Some("queued"), "{label}");
    }
    assert!(app.update(key(KeyCode::Char('s'))).is_empty());
    // The second start's own answer ends it.
    app.update(worker_done(second, "start acme@0:worker", Ok(json!({}))));
    assert_eq!(state(&app, "worker").as_deref(), Some("failed"));
    Ok(())
}

/// A queued unit whose reason carries `data`.
fn queued(data: Value) -> UnitSummary {
    let mut unit = unit("acme@0", "api", "queued");
    unit.reason = Some(RpcError::new("queued", data, 0));
    unit
}

#[test]
fn queued_rows_say_what_they_wait_for() {
    let cases = [
        (
            json!({"waiting_for": "migrate-db"}),
            "queued  waiting for migrate-db",
        ),
        (
            json!({"waiting_for": "containers"}),
            "queued  waiting for containers",
        ),
        (
            json!({"waiting_for": "lane"}),
            "queued  waiting for the lane",
        ),
        (
            json!({"position": 3, "queued": 12, "in_flight": 4, "window": 4, "hold": "window_full"}),
            "queued  3rd in line",
        ),
        (
            json!({"position": 1, "queued": 2, "in_flight": 0, "window": 4, "hold": "memory"}),
            "next in line, held: memory",
        ),
        (
            json!({"position": 12, "queued": 12, "hold": "memory_critical"}),
            "queued  12th in line, held: memory",
        ),
        // Without a reason (this TUI's own guess) the word stands alone.
        (Value::Null, "queued"),
    ];
    for (data, expected) in cases {
        let unit = if data.is_null() {
            unit("acme@0", "api", "queued")
        } else {
            queued(data)
        };
        let shown = ui::unit_state(&unit);
        assert!(shown.ends_with(expected), "{shown:?} for {expected:?}");
    }
}

/// The header's first row.
fn header(app: &App) -> Result<String, Box<dyn std::error::Error>> {
    Ok(row(&render(app)?, 0))
}

/// A `lane.phase` event for `acme@1`.
fn phase(phase: &str, state: &str, extra: Value) -> Msg {
    let mut data = json!({"lane": "acme@1", "phase": phase, "state": state, "start_id": 5});
    if let (Some(data), Value::Object(extra)) = (data.as_object_mut(), extra) {
        data.extend(extra);
    }
    event(events::PHASE, data)
}

#[tokio::test]
async fn the_header_names_steps_the_way_people_read_them() -> TestResult {
    let starting = lane(
        "acme",
        Some(1),
        "feat/login",
        "starting",
        vec![unit("acme@1", "api", "queued")],
    );
    let api = FakeApi::with_lanes(vec![starting]);
    let (mut app, _) = loaded(&api, 100, 24).await;
    app.update(phase("infra", "started", json!({})));
    let line = header(&app)?;
    assert!(line.contains("starting feat/login: containers"), "{line}");
    assert!(!line.contains("infra"), "{line}");
    // A skipped phase is never named; with nothing else running, the services count.
    app.update(phase("infra", "done", json!({})));
    app.update(phase("setup", "started", json!({})));
    assert!(header(&app)?.contains("starting feat/login: setup"));
    app.update(phase("setup", "skipped", json!({})));
    let line = header(&app)?;
    assert!(line.contains("starting feat/login: 0 of 1 ready"), "{line}");
    assert!(!line.contains("setup"), "{line}");
    // A task phase names the task it runs.
    app.update(phase("tasks", "started", json!({})));
    app.update(phase(
        "tasks",
        "progress",
        json!({"step": {"name": "migrate-db", "index": 1, "count": 1}}),
    ));
    let line = header(&app)?;
    assert!(line.contains("starting feat/login: migrate-db"), "{line}");
    assert!(!line.contains("tasks"), "{line}");
    Ok(())
}
