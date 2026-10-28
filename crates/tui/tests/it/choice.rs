//! Choosing which lane serves hostnames that several lanes run: the lanes
//! row with this machine's lane and the boxes', grey lanes and `m`.

use std::sync::Arc;

use crossterm::event::KeyCode;
use devlanes_proto::{Event, Method, RpcError, remote::WARNING_EVENT};
use devlanes_tui::{
    ApiError,
    app::{Dialog, Effect, Msg, Selected, Target},
    theme::{Glyphs, Theme, Tier},
};
use ratatui::style::Modifier;
use serde_json::{Value, json};

use super::{TestResult, draw, footer, key, loaded, select};
use crate::daemon::{presses, run_keys};
use crate::fake::{FakeApi, mixed_lanes};

/// `host.list` with `fork-a` and `fork-b`, both running `acme@0` and
/// `acme@1`. This machine runs `acme@0`, so it keeps those hostnames.
/// `acme@1` was chosen for `fork-a`, so fork-b's and this machine's stopped
/// one are grey.
fn forks() -> Value {
    let lane =
        |index: u32| json!({"key": {"project": "acme", "lane": index.to_string()}, "index": index});
    let skipped = |lane: &str, cause: &str, winner: Value| {
        json!({"lane": lane, "hosts": [format!("web.{lane}.localhost")], "box": winner,
               "reason": "clashes", "cause": cause, "winner": winner, "winner_lane": lane,
               "can_use": cause != "local_running"})
    };
    let fork = |host: &str, chosen: Value, skipped: Value| {
        json!({"host": host, "state": "connected", "transport": "ssh",
               "chosen_lanes": chosen, "skipped_lanes": skipped,
               "registry": {"lanes": [lane(0), lane(1)], "units": []}})
    };
    json!({
        "items": [
            fork("fork-b", json!([]), json!([
                skipped("acme@0", "local_running", Value::Null),
                skipped("acme@1", "box_chosen", json!("fork-a")),
            ])),
            fork("fork-a", json!(["acme@1"]), json!([
                skipped("acme@0", "local_running", Value::Null),
            ])),
        ],
        "yielded_local_lanes": [{"lane": "acme@1", "winner": "fork-a"}],
    })
}

/// An app with `mixed_lanes` and [`forks`] loaded.
async fn with_forks(width: u16, height: u16) -> devlanes_tui::app::App {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, width, height).await;
    app.update(Msg::Boxes(Ok(forks())));
    app
}

/// The header's notice.
fn notice(app: &devlanes_tui::app::App) -> String {
    app.notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default()
}

/// The `remote.use` effects among `effects`, as `(lane, host)`.
fn uses(effects: &[Effect]) -> Vec<(String, Option<String>)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::UseLane { lane, host } => Some((lane.clone(), host.clone())),
            _ => None,
        })
        .collect()
}

/// The pane's text as one line, its wrapping undone.
fn pane(app: &devlanes_tui::app::App) -> Result<String, Box<dyn std::error::Error>> {
    let terminal = draw(app)?;
    let buffer = terminal.backend().buffer();
    let area = app.areas().ok_or("no layout")?.pane_inner;
    let text: Vec<String> = (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    Ok(text
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" "))
}

/// The open confirm dialog's prompt, detail and key hint, as one line.
fn pane_and_dialog(app: &devlanes_tui::app::App) -> Result<String, Box<dyn std::error::Error>> {
    match &app.dialog {
        Some(Dialog::Confirm {
            prompt,
            detail,
            action,
            ..
        }) => Ok(format!("{prompt} {} y {action}", detail.join(" "))),
        other => Err(format!("no confirm dialog: {other:?}").into()),
    }
}

/// Row `y` of the drawn screen.
fn row(app: &devlanes_tui::app::App, y: u16) -> Result<String, Box<dyn std::error::Error>> {
    let terminal = draw(app)?;
    let buffer = terminal.backend().buffer();
    Ok((0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect())
}

#[tokio::test]
async fn the_lanes_row_goes_by_number_with_this_machine_first_then_boxes_by_name() -> TestResult {
    let app = with_forks(120, 30).await;
    let lanes: Vec<&str> = app
        .project_lanes()
        .iter()
        .map(|lane| lane.reference.as_str())
        .collect();
    assert_eq!(
        lanes,
        [
            "acme@0",
            "acme@0 @fork-a",
            "acme@0 @fork-b",
            "acme@1",
            "acme@1 @fork-a",
            "acme@1 @fork-b",
            "acme@2",
        ]
    );
    let bar = row(&app, 1)?;
    assert!(
        bar.contains(
            " lanes  1 @0  2 @0 fork-a ○  3 @0 fork-b ○  4 @1 ○  5 @1 fork-a  6 @1 fork-b ○  7 @2 "
        ),
        "{bar}"
    );
    Ok(())
}

#[tokio::test]
async fn a_long_box_name_is_cut_in_the_chip_and_whole_in_the_title() -> TestResult {
    let mut listing = forks();
    listing["items"][1]["host"] = json!("clickmax-fork-3f2a");
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 80, 24).await;
    app.update(Msg::Boxes(Ok(listing)));
    select(&mut app, Selected::lane("acme@1 @clickmax-fork-3f2a"));
    let bar = row(&app, 1)?;
    assert!(bar.contains("@1 clickma…"), "{bar}");
    let title = row(&app, 2)?;
    assert!(title.contains("@1 clickmax-fork-3f2a"), "{title}");
    Ok(())
}

/// Grey chips are dim with a hollow dot; with ascii glyphs and no color the
/// brackets say it alone.
#[tokio::test]
async fn grey_chips_are_dim_and_marked_without_color() -> TestResult {
    let mut app = with_forks(120, 30).await;
    let terminal = draw(&app)?;
    let buffer = terminal.backend().buffer();
    let bar: String = (0..buffer.area.width)
        .map(|x| buffer[(x, 1)].symbol())
        .collect();
    // A cell per character: `○` takes one cell but three bytes.
    let at = |text: &str| {
        bar.find(text)
            .map(|byte| u16::try_from(bar[..byte].chars().count()).unwrap_or(0))
    };
    let grey = at("@0 fork-b ○").ok_or("no grey chip")?;
    for x in grey..grey + 11 {
        assert!(
            buffer[(x, 1)].modifier.contains(Modifier::DIM),
            "cell {x} of {bar:?} is not dim"
        );
    }
    // Idle lanes are dim too; only the dot tells the grey ones apart.
    assert!(bar.contains("5 @1 fork-a  6"), "{bar}");

    app.theme = Theme {
        tier: Tier::Plain,
        glyphs: Glyphs::Ascii,
    };
    let bar = row(&app, 1)?;
    assert!(bar.contains("3 (@0 fork-b)"), "{bar}");
    assert!(bar.contains("4 (@1)"), "{bar}");
    assert!(!bar.contains('○'), "{bar}");
    // A mirrored lane keeps its state glyph without color.
    assert!(bar.contains("5 @1 fork-a "), "{bar}");
    Ok(())
}

#[tokio::test]
async fn m_on_a_grey_box_lane_asks_the_daemon_to_use_it() -> TestResult {
    let mut app = with_forks(120, 30).await;
    select(&mut app, Selected::lane("acme@1 @fork-b"));
    let text = pane(&app)?;
    assert!(
        text.contains("Not mirrored. fork-a's acme@1 is in use. Press m to use this one."),
        "{text}"
    );
    assert!(
        footer(&app)?.starts_with(" m use this one   "),
        "{}",
        footer(&app)?
    );
    let effects = app.update(key(KeyCode::Char('m')));
    assert_eq!(uses(&effects), [("acme@1".into(), Some("fork-b".into()))]);
    assert_eq!(notice(&app), "switching acme@1 to fork-b…");
    let effects = app.update(Msg::LaneUsed {
        lane: "acme@1".into(),
        host: Some("fork-b".into()),
        result: Ok(
            json!({"lane": "acme@1", "host": "fork-b", "previous": "fork-a",
                          "mirrored": true, "pending": false}),
        ),
    });
    // The daemon's notice says it switched, not the reply.
    assert_eq!(notice(&app), "switching acme@1 to fork-b…");
    assert!(
        effects.iter().any(|e| matches!(e, Effect::FetchBoxes)),
        "{effects:?}"
    );
    app.update(switched(
        "acme@1",
        Some("fork-b"),
        Some("acme@1 now comes from fork-b"),
    ));
    assert_eq!(notice(&app), "acme@1 now comes from fork-b");
    Ok(())
}

/// Nothing of a grey lane is routed here, so it offers nothing to open.
#[tokio::test]
async fn a_grey_lane_has_nothing_to_open() -> TestResult {
    let mut app = with_forks(120, 30).await;
    select(&mut app, Selected::lane("acme@1 @fork-b"));
    assert!(
        !footer(&app)?.contains("open in browser"),
        "{}",
        footer(&app)?
    );
    let effects = app.update(key(KeyCode::Char('o')));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::OpenUrl(_))),
        "{effects:?}"
    );
    assert_eq!(
        notice(&app),
        "this lane is not in use here, so it has nothing to open. m uses it"
    );
    // The lane in use still opens.
    select(&mut app, Selected::lane("acme@1 @fork-a"));
    assert!(
        footer(&app)?.contains("o open in browser"),
        "{}",
        footer(&app)?
    );
    Ok(())
}

/// A box lane that this machine's stopped lane keeps out names it the way
/// every message does.
#[tokio::test]
async fn a_lane_this_machine_keeps_reads_this_machines() -> TestResult {
    let lane = json!({"key": {"project": "acme", "lane": "1"}, "index": 1});
    let listing = json!({"items": [{"host": "fork-a", "state": "connected", "transport": "ssh",
        "skipped_lanes": [{"lane": "acme@1", "hosts": [], "box": null,
                           "reason": "this machine's acme@1 is in use", "cause": "local_stopped",
                           "winner": null, "winner_lane": "acme@1", "can_use": true}],
        "registry": {"lanes": [lane], "units": []}}]});
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 120, 40).await;
    app.update(Msg::Boxes(Ok(listing)));
    select(&mut app, Selected::lane("acme@1 @fork-a"));
    let text = pane(&app)?;
    assert!(
        text.contains("Not mirrored. This machine's acme@1 is in use. Press m to use this one."),
        "{text}"
    );
    app.update(crate::shifted('B'));
    let terminal = draw(&app)?;
    let buffer = terminal.backend().buffer();
    let screen: String = (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .map(|(x, y)| buffer[(x, y)].symbol().to_owned())
        .collect();
    assert!(
        screen.contains("acme@1: this machine's acme@1 is in use. Press m to use this one."),
        "{screen}"
    );
    Ok(())
}

/// A `lane_switched` notice from the daemon, for a switch any client made.
fn switched(lane: &str, to: Option<&str>, message: Option<&str>) -> Msg {
    Msg::Event(Event {
        seq: 11,
        event: WARNING_EVENT.into(),
        data: json!({"code": "lane_switched", "lane": lane, "to": to, "from": "fork-a",
                     "host": to.unwrap_or("fork-a"), "message": message}),
    })
}

/// A switch made from the CLI or another TUI shows here too, once.
#[tokio::test]
async fn a_switch_from_another_client_shows_the_daemons_notice() {
    let mut app = with_forks(120, 30).await;
    let effects = app.update(switched(
        "acme@1",
        Some("fork-b"),
        Some("acme@1 now comes from fork-b"),
    ));
    assert_eq!(notice(&app), "acme@1 now comes from fork-b");
    assert!(app.notice.as_ref().is_some_and(|n| !n.error));
    assert!(effects.iter().any(|e| matches!(e, Effect::FetchBoxes)));
    // Without a message, the same words from its fields.
    app.update(switched("acme@1", None, None));
    assert_eq!(notice(&app), "acme@1 now comes from this machine");
}

#[tokio::test]
async fn a_reply_that_switched_nothing_says_why() {
    let mut app = with_forks(120, 30).await;
    let reply = |mirrored: bool, previous: &str| Msg::LaneUsed {
        lane: "acme@1".into(),
        host: Some("fork-b".into()),
        result: Ok(
            json!({"lane": "acme@1", "host": "fork-b", "previous": previous,
                          "mirrored": mirrored, "pending": false}),
        ),
    };
    app.update(reply(false, "fork-a"));
    assert_eq!(
        notice(&app),
        "fork-b's acme@1 is chosen but not mirrored yet"
    );
    app.update(reply(true, "fork-b"));
    assert_eq!(notice(&app), "fork-b's acme@1 is already in use");
}

#[tokio::test]
async fn m_on_a_yielded_local_lane_gives_the_hostnames_back_to_this_machine() -> TestResult {
    let mut app = with_forks(120, 30).await;
    select(&mut app, Selected::lane("acme@1"));
    let text = pane(&app)?;
    assert!(
        text.contains("! fork-a's acme@1 is in use instead. Press m to use this one."),
        "{text}"
    );
    // The lanes table says which acme@1 is live, and which are not.
    for expected in [
        "feat/login @1 not in use",
        "@1 on fork-a in use",
        "@1 on fork-b not mirrored",
        "main @0 in use",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    // This machine's own lane is not mirrored from anywhere: it is not in use.
    let title = row(&app, 2)?;
    assert!(title.contains("@1 · not in use "), "{title}");
    assert!(footer(&app)?.starts_with(" m use this one   "));
    let effects = app.update(key(KeyCode::Char('m')));
    assert_eq!(uses(&effects), [("acme@1".into(), None)]);
    app.update(Msg::LaneUsed {
        lane: "acme@1".into(),
        host: None,
        result: Ok(json!({"lane": "acme@1", "host": null, "previous": "fork-a",
                          "mirrored": true, "pending": false})),
    });
    assert_eq!(notice(&app), "switching acme@1 to this machine…");
    app.update(switched(
        "acme@1",
        None,
        Some("acme@1 now comes from this machine"),
    ));
    assert_eq!(notice(&app), "acme@1 now comes from this machine");
    Ok(())
}

/// The stop that `m` sends for this machine's lane, as `(action, lane)`.
fn stops(effects: &[Effect]) -> Vec<(u64, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Control {
                action,
                method: Method::Stop,
                lane,
                selection,
                ..
            } if selection.units.is_empty() => Some((*action, lane.clone())),
            _ => None,
        })
        .collect()
}

/// `mixed_lanes` with `acme@0` stopped, as status reports it after a stop.
fn acme_stopped() -> Vec<devlanes_proto::LaneSummary> {
    let mut lanes = mixed_lanes();
    lanes[0].state = "idle".into();
    for unit in &mut lanes[0].units {
        unit.state = "stopped".into();
    }
    lanes
}

/// `local_lane_running` for `acme@0`, as the daemon answers while it still
/// counts the stopped lane as running.
fn still_running() -> Result<Value, ApiError> {
    Err(ApiError::Rpc(RpcError::new(
        "local_lane_running",
        json!({"host": "fork-b", "lane": "acme@0", "local_lane": "acme@0",
               "message": "acme@0 runs on this machine and keeps the hostnames"}),
        1,
    )))
}

/// The stop answered for `action`.
fn stopped(action: u64) -> Msg {
    Msg::ActionDone {
        action,
        label: "stop acme@0".into(),
        target: Target::Lane("acme@0".into()),
        units: Vec::new(),
        result: Ok(json!({})),
    }
}

/// While this machine runs the lane, `m` asks to stop it and stops it. The
/// stop's answer sends `remote.use` at once, and a `local_lane_running`
/// answer sends it again on the next tick.
#[tokio::test]
async fn m_stops_this_machines_lane_after_a_confirm_then_switches() -> TestResult {
    let mut app = with_forks(120, 30).await;
    select(&mut app, Selected::lane("acme@0 @fork-b"));
    let text = pane(&app)?;
    assert!(
        text.contains(
            "Not mirrored. acme@0 runs on this machine. Press m to stop it and use this one."
        ),
        "{text}"
    );
    assert!(
        footer(&app)?.starts_with(" m use this one   "),
        "{}",
        footer(&app)?
    );
    let effects = app.update(key(KeyCode::Char('m')));
    assert!(
        uses(&effects).is_empty() && stops(&effects).is_empty(),
        "{effects:?}"
    );
    let dialog = pane_and_dialog(&app)?;
    assert!(
        dialog.contains("acme@0 runs on this machine. Stop it and use fork-b's acme@0?"),
        "{dialog}"
    );
    assert!(dialog.contains("y stop and switch"), "{dialog}");
    // Enter does not confirm a stop.
    app.update(key(KeyCode::Enter));
    assert!(app.dialog.is_some());
    let effects = app.update(key(KeyCode::Char('y')));
    let [(action, lane)] = stops(&effects).try_into().map_err(|_| "one stop")?;
    assert_eq!(lane, "acme@0");
    assert!(uses(&effects).is_empty(), "{effects:?}");
    assert_eq!(notice(&app), "stopping acme@0 to use fork-b's acme@0…");
    // The stop answered while status still says the lane failed: its worker
    // failed before, which the daemon does not count as running.
    let mut failed = acme_stopped();
    failed[0].state = "failed".into();
    app.update(Msg::Status(Ok(failed)));
    let use_fork_b = [("acme@0".to_owned(), Some("fork-b".to_owned()))];
    let effects = app.update(stopped(action));
    assert_eq!(uses(&effects), use_fork_b);
    assert_eq!(notice(&app), "switching acme@0 to fork-b…");
    // Nothing more while it is on its way.
    let effects = app.update(Msg::Tick);
    assert!(uses(&effects).is_empty(), "{effects:?}");
    // Still counted as running: no dialog, no error, and the next tick asks again.
    app.update(Msg::LaneUsed {
        lane: "acme@0".into(),
        host: Some("fork-b".into()),
        result: still_running(),
    });
    assert!(app.dialog.is_none());
    assert!(app.notice.as_ref().is_some_and(|n| !n.error));
    let effects = app.update(Msg::Tick);
    assert_eq!(uses(&effects), use_fork_b);
    app.update(Msg::LaneUsed {
        lane: "acme@0".into(),
        host: Some("fork-b".into()),
        result: Ok(json!({"lane": "acme@0", "host": "fork-b", "previous": null,
                          "mirrored": true, "pending": false})),
    });
    // Done: no more asking.
    let effects = app.update(Msg::Tick);
    assert!(uses(&effects).is_empty(), "{effects:?}");
    Ok(())
}

#[tokio::test]
async fn a_failed_stop_ends_the_switch_with_the_daemons_words() -> TestResult {
    let mut app = with_forks(120, 30).await;
    select(&mut app, Selected::lane("acme@0 @fork-b"));
    app.update(key(KeyCode::Char('m')));
    let effects = app.update(key(KeyCode::Char('y')));
    let [(action, _)] = stops(&effects).try_into().map_err(|_| "one stop")?;
    app.update(Msg::ActionDone {
        action,
        label: "stop acme@0".into(),
        target: Target::Lane("acme@0".into()),
        units: Vec::new(),
        result: Err(ApiError::Rpc(RpcError::new(
            "stop_failed",
            json!({"message": "worker did not exit"}),
            1,
        ))),
    });
    let shown = app.notice.as_ref().ok_or("no notice")?;
    assert!(shown.error);
    assert!(
        shown.text.starts_with("stop acme@0 failed: ")
            && shown.text.contains("worker did not exit"),
        "{}",
        shown.text
    );
    let effects = app.update(Msg::Tick);
    assert!(
        uses(&effects).is_empty(),
        "no switch after a failed stop: {effects:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_lane_the_daemon_keeps_counting_as_running_ends_the_switch() -> TestResult {
    let mut app = with_forks(120, 30).await;
    select(&mut app, Selected::lane("acme@0 @fork-b"));
    app.update(key(KeyCode::Char('m')));
    let effects = app.update(key(KeyCode::Char('y')));
    let [(action, _)] = stops(&effects).try_into().map_err(|_| "one stop")?;
    app.update(stopped(action));
    for _ in 0..30 {
        app.update(Msg::LaneUsed {
            lane: "acme@0".into(),
            host: Some("fork-b".into()),
            result: still_running(),
        });
        app.update(Msg::Tick);
    }
    let shown = app.notice.as_ref().ok_or("no notice")?;
    assert!(shown.error);
    assert_eq!(
        shown.text,
        "use acme@0 from fork-b failed: acme@0 is still running. Stop it, then press m again"
    );
    let effects = app.update(Msg::Tick);
    assert!(uses(&effects).is_empty(), "{effects:?}");
    Ok(())
}

#[tokio::test]
async fn n_keeps_this_machines_lane_running() -> TestResult {
    let mut app = with_forks(120, 30).await;
    select(&mut app, Selected::lane("acme@0 @fork-b"));
    app.update(key(KeyCode::Char('m')));
    let effects = app.update(key(KeyCode::Char('n')));
    assert!(app.dialog.is_none());
    assert!(
        stops(&effects).is_empty() && uses(&effects).is_empty(),
        "{effects:?}"
    );
    let effects = app.update(Msg::Status(Ok(acme_stopped())));
    assert!(uses(&effects).is_empty(), "{effects:?}");
    Ok(())
}

/// Another project's lane may clash on a hostname: the pane names the lane
/// in use by the daemon's `winner_lane`.
#[tokio::test]
async fn the_pane_names_the_lane_in_use_when_another_project_clashes() -> TestResult {
    let lane = json!({"key": {"project": "billing", "lane": "0"}, "index": 0});
    let fork = |host: &str, skipped: Value| {
        json!({"host": host, "state": "connected", "transport": "ssh",
               "skipped_lanes": skipped, "registry": {"lanes": [lane], "units": []}})
    };
    let listing = json!({"items": [
        fork("fork-a", json!([])),
        fork("fork-b", json!([{"lane": "billing@0", "hosts": ["pay.billing.localhost"],
                               "box": "fork-a", "reason": "fork-a's shop@3 is in use",
                               "cause": "box_earlier", "winner": "fork-a",
                               "winner_lane": "shop@3", "can_use": true}])),
        fork("fork-c", json!([{"lane": "billing@0", "hosts": ["api.acme.localhost"],
                               "box": null, "reason": "acme@0 runs on this machine",
                               "cause": "local_running", "winner": null,
                               "winner_lane": "acme@0", "can_use": false}])),
    ]});
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 120, 30).await;
    app.update(Msg::Boxes(Ok(listing)));
    select(&mut app, Selected::lane("billing@0 @fork-b"));
    let text = pane(&app)?;
    assert!(
        text.contains("Not mirrored. fork-a's shop@3 is in use."),
        "{text}"
    );
    select(&mut app, Selected::lane("billing@0 @fork-c"));
    let text = pane(&app)?;
    assert!(
        text.contains("Not mirrored. acme@0 runs on this machine."),
        "{text}"
    );
    app.update(key(KeyCode::Char('m')));
    let dialog = pane_and_dialog(&app)?;
    assert!(
        dialog.contains("acme@0 runs on this machine. Stop it and use fork-c's billing@0?"),
        "{dialog}"
    );
    Ok(())
}

#[tokio::test]
async fn m_on_a_lane_in_use_only_says_so() -> TestResult {
    let mut app = with_forks(120, 30).await;
    let cases = [
        ("acme@1 @fork-a", "fork-a's acme@1 is already in use"),
        ("acme@0", "this machine's acme@0 is already in use"),
        (
            "acme@2",
            "no box runs acme@2, so there is nothing to switch to",
        ),
    ];
    for (lane, said) in cases {
        select(&mut app, Selected::lane(lane));
        assert!(!footer(&app)?.contains("m use this one"), "{lane}");
        let effects = app.update(key(KeyCode::Char('m')));
        assert!(uses(&effects).is_empty(), "{lane}: {effects:?}");
        assert_eq!(notice(&app), said);
    }
    Ok(())
}

#[tokio::test]
async fn a_refused_switch_shows_the_daemons_words() -> TestResult {
    let mut app = with_forks(120, 30).await;
    let error = RpcError::new(
        "lane_not_on_box",
        json!({"message": "box fork-b has no lane acme@1. Run devlanes remote ls to see its lanes"}),
        1,
    );
    app.update(Msg::LaneUsed {
        lane: "acme@1".into(),
        host: Some("fork-b".into()),
        result: Err(ApiError::Rpc(error)),
    });
    let shown = app.notice.as_ref().ok_or("no notice")?;
    assert!(shown.error);
    assert_eq!(
        shown.text,
        "use acme@1 from fork-b failed: box fork-b has no lane acme@1. Run devlanes remote ls to see its lanes"
    );
    Ok(())
}

/// A local lane that started before `remote.use` arrived: the switch asks
/// to stop the lane the daemon names in `local_lane`.
#[tokio::test]
async fn a_switch_refused_for_a_running_lane_offers_to_stop_it() -> TestResult {
    let mut app = with_forks(120, 30).await;
    let error = RpcError::new(
        "local_lane_running",
        json!({"host": "fork-b", "lane": "acme@1", "local_lane": "shop@0",
               "message": "shop@0 runs on this machine and keeps the hostnames. Stop it first with devlanes stop --lane shop@0"}),
        1,
    );
    app.update(Msg::LaneUsed {
        lane: "acme@1".into(),
        host: Some("fork-b".into()),
        result: Err(ApiError::Rpc(error)),
    });
    let dialog = pane_and_dialog(&app)?;
    assert!(
        dialog.contains("shop@0 runs on this machine. Stop it and use fork-b's acme@1?"),
        "{dialog}"
    );
    assert!(dialog.contains("Then fork-b's acme@1 serves"), "{dialog}");
    let effects = app.update(key(KeyCode::Char('y')));
    let stopped: Vec<String> = stops(&effects).into_iter().map(|(_, lane)| lane).collect();
    assert_eq!(stopped, ["shop@0"]);
    Ok(())
}

/// A box that is not connected keeps the choice for when it connects.
#[tokio::test]
async fn a_choice_for_a_box_that_is_not_connected_waits_for_it() {
    let mut app = with_forks(120, 30).await;
    let effects = app.update(Msg::LaneUsed {
        lane: "acme@1".into(),
        host: Some("fork-b".into()),
        result: Ok(
            json!({"lane": "acme@1", "host": "fork-b", "previous": "fork-a",
                          "mirrored": false, "pending": true}),
        ),
    });
    let shown = app.notice.as_ref().map(|n| (n.text.as_str(), n.error));
    assert_eq!(
        shown,
        Some(("acme@1 will come from fork-b once it connects", false))
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::FetchBoxes)));
}

#[tokio::test]
async fn a_local_start_that_clears_a_choice_says_so() {
    let mut app = with_forks(120, 30).await;
    let effects = app.update(Msg::Event(Event {
        seq: 9,
        event: WARNING_EVENT.into(),
        data: json!({"code": "choice_cleared", "lane": "acme@1", "host": "fork-a"}),
    }));
    assert_eq!(
        notice(&app),
        "acme@1 started on this machine, so fork-a's acme@1 is no longer mirrored or chosen"
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::FetchBoxes)));
    // Another project's lane that clashes on a hostname.
    app.update(Msg::Event(Event {
        seq: 10,
        event: WARNING_EVENT.into(),
        data: json!({"code": "choice_cleared", "lane": "acme@1", "host": "fork-a",
                     "local_lane": "shop@0"}),
    }));
    assert_eq!(
        notice(&app),
        "shop@0 started on this machine, so fork-a's acme@1 is no longer mirrored or chosen"
    );
}

#[tokio::test]
async fn the_boxes_view_names_what_serves_a_skipped_lane_and_the_chosen_ones() -> TestResult {
    let mut app = with_forks(120, 40).await;
    app.update(crate::shifted('B'));
    app.update(Msg::Boxes(Ok(forks())));
    let terminal = draw(&app)?;
    let buffer = terminal.backend().buffer();
    let rows: Vec<String> = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .replace('│', " ")
        })
        .collect();
    let text = rows
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for expected in [
        "chosen acme@1",
        "acme@1: fork-a's acme@1 is in use. Press m to use this one.",
        "skipped acme@0 also runs on this machine. Press m to stop it and use this one.",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    Ok(())
}

/// A skipped lane row that wraps is selected on every line.
#[tokio::test]
async fn a_wrapped_skipped_lane_row_is_selected_whole() -> TestResult {
    let mut app = with_forks(80, 24).await;
    app.update(crate::shifted('B'));
    app.update(Msg::Boxes(Ok(forks())));
    app.update(key(KeyCode::Char('j')));
    let terminal = draw(&app)?;
    let buffer = terminal.backend().buffer();
    let selected: Vec<String> = (0..buffer.area.height)
        .filter(|y| buffer[(4, *y)].modifier.contains(Modifier::REVERSED))
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect();
    let joined = selected
        .join(" ")
        .replace('│', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(selected.len(), 2, "{selected:?}");
    assert!(
        joined.contains("acme@0 also runs on this machine. Press m to stop it and use this one."),
        "{joined}"
    );
    Ok(())
}

/// `j` moves through each box's skipped lanes, and `m` on one does what
/// `m` does on that lane in its tab.
#[tokio::test]
async fn m_on_a_skipped_lane_row_of_the_boxes_view_uses_it() -> TestResult {
    let mut app = with_forks(120, 40).await;
    app.update(crate::shifted('B'));
    app.update(Msg::Boxes(Ok(forks())));
    // On fork-b itself, m has no lane to use.
    let effects = app.update(key(KeyCode::Char('m')));
    assert!(uses(&effects).is_empty() && app.dialog.is_none());
    assert_eq!(notice(&app), "select a skipped lane of a box to use it");
    // fork-b's acme@0, which this machine runs.
    app.update(key(KeyCode::Char('j')));
    assert!(
        footer(&app)?.starts_with(" m use this one   "),
        "{}",
        footer(&app)?
    );
    app.update(key(KeyCode::Char('m')));
    let dialog = pane_and_dialog(&app)?;
    assert!(
        dialog.contains("acme@0 runs on this machine. Stop it and use fork-b's acme@0?"),
        "{dialog}"
    );
    app.update(key(KeyCode::Char('n')));
    // fork-b's acme@1, chosen for fork-a; Enter does the same as m.
    app.update(key(KeyCode::Char('j')));
    let terminal = draw(&app)?;
    let buffer = terminal.backend().buffer();
    let selected: Vec<String> = (0..buffer.area.height)
        .filter(|y| buffer[(4, *y)].modifier.contains(Modifier::REVERSED))
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    assert!(
        matches!(selected.as_slice(), [row] if row.contains("acme@1: fork-a's acme@1 is in use.")),
        "{selected:?}"
    );
    let effects = app.update(key(KeyCode::Enter));
    assert_eq!(uses(&effects), [("acme@1".into(), Some("fork-b".into()))]);
    // Then fork-a and its own skipped lane, past which come the clients.
    app.update(key(KeyCode::Char('j')));
    assert!(app.boxes.lane.is_none());
    assert_eq!(app.boxes.host, 1);
    app.update(key(KeyCode::Char('j')));
    assert_eq!(app.boxes.lane, Some(0));
    Ok(())
}

/// The whole loop: `3`, `m` and `y` on fork-b's `acme@0` stop this
/// machine's `acme@0`, then send `remote.use`, and the daemon's
/// `lane_switched` notice shows.
#[tokio::test]
async fn drive_stop_and_switch_stops_the_local_lane_then_uses_the_box() -> TestResult {
    let mut fake = FakeApi::with_lanes(mixed_lanes());
    fake.stops = true;
    *fake.boxes.lock().map_err(|e| e.to_string())? = Some(forks());
    let api = Arc::new(fake);
    let mut steps = vec![crate::daemon::Step::Screen("fork-b")];
    steps.extend(presses(vec![
        key(KeyCode::Char('3')),
        key(KeyCode::Char('m')),
    ]));
    steps.push(crate::daemon::Step::Screen("stop and switch"));
    steps.extend(presses(vec![key(KeyCode::Char('y'))]));
    run_keys(
        Arc::clone(&api),
        steps,
        &["acme@0 now comes from fork-b"],
        |calls| calls.iter().any(|c| c == "remote.use acme@0 fork-b"),
    )
    .await?;
    let calls = api.calls();
    let stop = calls.iter().position(|c| c == "Stop acme@0 []");
    let switch = calls.iter().position(|c| c == "remote.use acme@0 fork-b");
    assert!(
        matches!((stop, switch), (Some(stop), Some(switch)) if stop < switch),
        "{calls:?}"
    );
    Ok(())
}

/// A stop the daemon refuses ends the switch: no `remote.use`.
#[tokio::test]
async fn drive_a_failed_stop_never_switches() -> TestResult {
    let mut fake = FakeApi::with_lanes(mixed_lanes());
    fake.control_error = Some(RpcError::new(
        "stop_failed",
        json!({"message": "worker did not exit"}),
        1,
    ));
    *fake.boxes.lock().map_err(|e| e.to_string())? = Some(forks());
    let api = Arc::new(fake);
    let mut steps = vec![crate::daemon::Step::Screen("fork-b")];
    steps.extend(presses(vec![
        key(KeyCode::Char('3')),
        key(KeyCode::Char('m')),
    ]));
    steps.push(crate::daemon::Step::Screen("stop and switch"));
    steps.extend(presses(vec![key(KeyCode::Char('y'))]));
    run_keys(Arc::clone(&api), steps, &["stop acme@0 failed"], |calls| {
        calls.iter().any(|c| c == "Stop acme@0 []")
    })
    .await?;
    let calls = api.calls();
    assert!(
        !calls.iter().any(|c| c.starts_with("remote.use")),
        "{calls:?}"
    );
    Ok(())
}

/// The whole loop: `4` and `m` on this machine's grey `acme@1` send
/// `remote.use` with `host: null`, then fetch `host.list` again.
#[tokio::test]
async fn drive_m_sends_remote_use_and_fetches_the_boxes() -> TestResult {
    let fake = FakeApi::with_lanes(mixed_lanes());
    *fake.boxes.lock().map_err(|e| e.to_string())? = Some(forks());
    let api = Arc::new(fake);
    let mut steps = vec![crate::daemon::Step::Screen("fork-a")];
    steps.extend(presses(vec![
        key(KeyCode::Char('4')),
        key(KeyCode::Char('m')),
    ]));
    run_keys(Arc::clone(&api), steps, &["now comes from"], |calls| {
        calls
            .iter()
            .skip_while(|c| *c != "remote.use acme@1 null")
            .any(|c| c == "host.list")
    })
    .await?;
    let calls = api.calls();
    assert_eq!(
        calls.iter().filter(|c| c.starts_with("remote.use")).count(),
        1,
        "{calls:?}"
    );
    Ok(())
}
