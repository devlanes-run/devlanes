//! Choosing which lane serves hostnames that several lanes run: the lanes
//! row with this machine's lane and the boxes', grey lanes and `m`.

use std::sync::Arc;

use crossterm::event::KeyCode;
use devlanes_proto::{Event, RpcError, remote::WARNING_EVENT};
use devlanes_tui::{
    ApiError,
    app::{Effect, Msg, Selected},
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
               "reason": "clashes", "cause": cause, "winner": winner,
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
            json!({"lane": "acme@1", "host": "fork-b", "previous": "fork-a", "mirrored": true}),
        ),
    });
    assert_eq!(notice(&app), "acme@1 now comes from fork-b");
    assert!(
        effects.iter().any(|e| matches!(e, Effect::FetchBoxes)),
        "{effects:?}"
    );
    Ok(())
}

#[tokio::test]
async fn m_on_a_yielded_local_lane_gives_the_hostnames_back_to_this_machine() -> TestResult {
    let mut app = with_forks(120, 30).await;
    select(&mut app, Selected::lane("acme@1"));
    let text = pane(&app)?;
    assert!(
        text.contains("Not mirrored. fork-a's acme@1 is in use. Press m to use this one."),
        "{text}"
    );
    assert!(footer(&app)?.starts_with(" m use this one   "));
    let effects = app.update(key(KeyCode::Char('m')));
    assert_eq!(uses(&effects), [("acme@1".into(), None)]);
    app.update(Msg::LaneUsed {
        lane: "acme@1".into(),
        host: None,
        result: Ok(json!({"lane": "acme@1", "host": null, "previous": "fork-a", "mirrored": true})),
    });
    assert_eq!(notice(&app), "acme@1 now comes from this machine");
    Ok(())
}

#[tokio::test]
async fn m_is_refused_while_this_machine_runs_the_lane() -> TestResult {
    let mut app = with_forks(120, 30).await;
    select(&mut app, Selected::lane("acme@0 @fork-b"));
    let text = pane(&app)?;
    assert!(
        text.contains("Not mirrored. acme@0 runs on this machine. Stop it there to use this one."),
        "{text}"
    );
    assert!(
        !footer(&app)?.contains("m use this one"),
        "{}",
        footer(&app)?
    );
    let effects = app.update(key(KeyCode::Char('m')));
    assert!(uses(&effects).is_empty(), "{effects:?}");
    assert_eq!(
        notice(&app),
        "acme@0 runs on this machine. Stop it there to use this one"
    );
    Ok(())
}

#[tokio::test]
async fn m_on_a_lane_in_use_only_says_so() -> TestResult {
    let mut app = with_forks(120, 30).await;
    let cases = [
        ("acme@1 @fork-a", "acme@1 from fork-a is already in use"),
        ("acme@0", "acme@0 on this machine is already in use"),
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
        "box_not_connected",
        json!({"message": "fork-b is not connected. Connect it with devlanes remote connect fork-b"}),
        4,
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
        "use acme@1 from fork-b failed: fork-b is not connected. Connect it with devlanes remote connect fork-b"
    );
    Ok(())
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
        "acme@1 started here, so fork-a's acme@1 is no longer mirrored"
    );
    assert!(effects.iter().any(|e| matches!(e, Effect::FetchBoxes)));
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
