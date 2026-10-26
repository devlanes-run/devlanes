//! Boxes in the TUI: box projects after the local ones, read-only lanes
//! whose keys say where to act instead, the details pane and the boxes view.

use crossterm::event::KeyCode;
use devlanes_proto::remote::MIRROR_EVENT;
use devlanes_tui::app::{App, Effect, Msg, RowId, Selected, View};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::{Value, json};

use super::{TestResult, draw, key, loaded, select, shifted};
use crate::fake::{FakeApi, mixed_lanes};

/// `host.list` with `hetzner` connected over ssh, running `acme@1` with a
/// mirrored `web`, a `worker` whose port is taken here and a task, and
/// `devbox` reached by a command, waiting for a sign-in.
fn hosts() -> Value {
    let lane = json!({"project": "acme", "lane": "1"});
    let unit = |kind: &str, name: &str, state: &str, ports: Value, hosts: Value| {
        json!({"key": {"lane": lane, "kind": kind, "name": name}, "hidden": false,
               "state": state, "ports": ports, "hosts": hosts})
    };
    json!({"items": [
        {
            "host": "hetzner", "state": "connected", "transport": "ssh",
            "connected_since_ms": 1_792_000_000_000u64,
            "ports": [
                {"port": 24100, "lane": "acme@1", "unit": "web", "kind": "service",
                 "state": "bound", "addresses": ["127.0.0.1", "::1"], "conflicts": [],
                 "relay": null, "last_error": null},
                {"port": 24101, "lane": "acme@1", "unit": "worker", "kind": "service",
                 "state": "in_use", "addresses": [], "relay": null, "last_error": null,
                 "conflicts": [{"holder": "postgres (pid 812)",
                                "message": "port 24101 is in use on this machine by postgres (pid 812); not mirrored"}]},
            ],
            "routes": [{"host": "web.acme-1.localhost", "port": 24100, "lane": "acme@1",
                        "unit": "web", "url": "https://web.acme-1.localhost", "state": "ready"}],
            "skipped_lanes": [], "skipped_hosts": [], "syncs": [],
            "warnings": ["the box runs devlanes 0.0.1 and this machine X; install the same release on both"],
            "registry": {
                "lanes": [{"key": lane, "index": 1, "domain": "acme-1.localhost"}],
                "units": [
                    unit("service", "web", "ready", json!([{"name": "default", "port": 24100}]),
                         json!([{"host": "web.acme-1.localhost", "port": 24100}])),
                    unit("service", "worker", "running", json!([{"name": "default", "port": 24101}]), json!([])),
                    unit("task", "migrate", "running", json!([]), json!([])),
                ],
            },
            "box": {"version": "0.0.1", "https_port": 443, "ports_range": "24000-24999"},
            "suffixes": ["localhost"],
            "proxy": {"https_port": 443, "box_port": null, "open": false},
        },
        {
            "host": "devbox", "state": "needs_auth", "transport": "command",
            "error": "Permission denied (publickey).", "fix": "devlanes remote connect devbox",
            "ports": [], "routes": [], "skipped_lanes": [], "skipped_hosts": [], "syncs": [],
            "warnings": [], "registry": {"lanes": [], "units": []},
        },
    ]})
}

/// The screen's rows, one line each.
fn screen_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An app with `mixed_lanes` and the boxes of [`hosts`] loaded.
async fn with_boxes(width: u16, height: u16) -> App {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, width, height).await;
    app.update(Msg::Boxes(Ok(hosts())));
    app
}

/// Effects that would act on a unit or lane, or open its stream.
fn acts(effects: &[Effect]) -> bool {
    effects.iter().any(|effect| {
        matches!(
            effect,
            Effect::Control { .. }
                | Effect::Run { .. }
                | Effect::Attach { .. }
                | Effect::FollowContainer { .. }
                | Effect::FetchLogs { .. }
                | Effect::FetchProfiles { .. }
                | Effect::FetchTasks { .. }
                | Effect::CleanLane { .. }
                | Effect::Input(_)
        )
    })
}

#[tokio::test]
async fn box_projects_follow_the_local_ones_labelled_with_the_box() -> TestResult {
    let mut app = with_boxes(100, 24).await;
    assert_eq!(app.projects(), ["acme", "shop", "acme @hetzner"]);
    // Shift+3: the box project, on its only lane.
    let effects = app.update(key(KeyCode::Char('#')));
    assert!(!acts(&effects), "{effects:?}");
    assert_eq!(app.project(), Some("acme @hetzner"));
    assert_eq!(
        app.selected.as_ref().map(|s| s.lane.as_str()),
        Some("acme@1 @hetzner")
    );
    // g: the lane itself, with the box's facts.
    app.update(key(KeyCode::Char('g')));
    assert_eq!(app.selected_id(), Some(&RowId::WholeLane));
    assert_screen!("box_project", draw(&app)?.backend());
    Ok(())
}

#[tokio::test]
async fn a_box_unit_shows_its_details_instead_of_a_terminal() -> TestResult {
    let mut app = with_boxes(100, 24).await;
    let effects = select(&mut app, Selected::unit("acme@1 @hetzner", "worker"));
    assert!(!acts(&effects), "no stream for a box unit: {effects:?}");
    assert!(app.attached.is_none());
    let text = screen_text(&draw(&app)?);
    assert!(text.contains("in use here by postgres"), "{text}");
    assert!(text.contains("read-only: manage it on"), "{text}");
    assert_screen!("box_unit", draw(&app)?.backend());
    select(&mut app, Selected::unit("acme@1 @hetzner", "web"));
    // The URL that opens it here is a link like every unit URL.
    assert!(
        app.link_targets()
            .contains(&"https://web.acme-1.localhost".to_owned())
    );
    let effects = app.update(key(KeyCode::Char('o')));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::OpenUrl(url) if url == "https://web.acme-1.localhost")),
        "{effects:?}"
    );
    Ok(())
}

#[tokio::test]
async fn actions_on_box_units_do_nothing_and_name_the_command_for_the_box() -> TestResult {
    let mut app = with_boxes(100, 24).await;
    select(&mut app, Selected::unit("acme@1 @hetzner", "web"));
    let cases = [
        (key(KeyCode::Char('s')), "devlanes start acme@1:web"),
        (key(KeyCode::Char('x')), "devlanes stop acme@1:web"),
        (key(KeyCode::Char('r')), "devlanes restart acme@1:web"),
        (shifted('S'), "devlanes start --lane acme@1"),
        (
            key(KeyCode::Char('p')),
            "devlanes start --lane acme@1 --profile <name>",
        ),
        (shifted('C'), "devlanes lane clean acme@1"),
        (key(KeyCode::Enter), "devlanes attach acme@1:web"),
        (key(KeyCode::Char('c')), "devlanes logs acme@1:web"),
        (key(KeyCode::PageUp), "devlanes logs acme@1:web"),
    ];
    for (msg, command) in cases {
        let shown = format!("{msg:?}");
        let effects = app.update(msg);
        assert!(!acts(&effects), "{shown}: {effects:?}");
        assert!(app.dialog.is_none(), "{shown} opened a dialog");
        let notice = app
            .notice
            .as_ref()
            .map(|n| n.text.clone())
            .unwrap_or_default();
        assert!(
            notice.contains("runs on hetzner: ssh hetzner, then ") && notice.contains(command),
            "{shown}: {notice}"
        );
    }
    // A box task runs again on the box; the lane stops there.
    select(&mut app, Selected::unit("acme@1 @hetzner", "migrate"));
    app.update(key(KeyCode::Char('r')));
    let notice = app
        .notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default();
    assert!(
        notice.contains("devlanes run migrate --lane acme@1"),
        "{notice}"
    );
    select(&mut app, Selected::lane("acme@1 @hetzner"));
    let effects = app.update(key(KeyCode::Char('x')));
    assert!(!acts(&effects), "{effects:?}");
    let notice = app
        .notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default();
    assert!(notice.contains("devlanes stop --lane acme@1"), "{notice}");
    // Moving, folding and the filter still work.
    app.update(key(KeyCode::Char('j')));
    assert_ne!(app.selected_id(), Some(&RowId::WholeLane));
    Ok(())
}

#[tokio::test]
async fn quitting_never_offers_to_stop_a_boxs_lanes() {
    let mut app = with_boxes(100, 24).await;
    select(&mut app, Selected::unit("acme@1 @hetzner", "web"));
    let effects = app.update(key(KeyCode::Char('q')));
    assert!(app.dialog.is_none(), "no quit dialog for a box lane");
    assert!(
        effects.iter().any(|e| matches!(e, Effect::Quit)),
        "{effects:?}"
    );
}

#[tokio::test]
async fn the_monitor_leaves_box_units_out() {
    let app = with_boxes(100, 24).await;
    let rows = format!("{:?}", app.monitor_rows());
    assert!(!rows.contains("@hetzner"), "{rows}");
}

#[tokio::test]
async fn the_boxes_view_shows_links_errors_fixes_and_warnings() -> TestResult {
    let mut app = with_boxes(100, 30).await;
    let effects = app.update(shifted('B'));
    assert_eq!(app.view, View::Boxes);
    assert!(
        effects.iter().any(|e| matches!(e, Effect::FetchBoxes)),
        "opening the view fetches host.list: {effects:?}"
    );
    app.update(Msg::Boxes(Ok(hosts())));
    let text = screen_text(&draw(&app)?);
    for expected in [
        "B boxes · 2 issues",
        "connected since 2026-10-14 ",
        "proxy on 443 · ports.range 24000-24999",
        "needs a sign-in · command",
        "fix       devlanes remote connect devbox",
        "the box runs devlanes 0.0.1",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert_screen!("boxes_view", draw(&app)?.backend());
    // B again, or esc, goes back.
    app.update(key(KeyCode::Esc));
    assert_eq!(app.view, View::Project);
    Ok(())
}

#[tokio::test]
async fn remote_events_refetch_the_boxes_once_at_a_time() {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 100, 24).await;
    let changed = || {
        Msg::Event(devlanes_proto::Event {
            seq: 1,
            event: MIRROR_EVENT.into(),
            data: json!({"host": "hetzner"}),
        })
    };
    let fetches = |effects: &[Effect]| {
        effects
            .iter()
            .filter(|e| matches!(e, Effect::FetchBoxes))
            .count()
    };
    assert_eq!(fetches(&app.update(changed())), 1);
    // In flight: further events only ask for one more fetch, after this one.
    assert_eq!(fetches(&app.update(changed())), 0);
    let state = Msg::Event(devlanes_proto::Event {
        seq: 2,
        event: "remote.state".into(),
        data: json!({"host": "hetzner", "state": "reconnecting"}),
    });
    assert_eq!(fetches(&app.update(state)), 0);
    assert_eq!(fetches(&app.update(Msg::Boxes(Ok(hosts())))), 1);
    assert_eq!(fetches(&app.update(Msg::Boxes(Ok(hosts())))), 0);
    assert_eq!(
        app.projects().last().map(String::as_str),
        Some("acme @hetzner")
    );
    // A box that goes away takes its project, and the selection moves on.
    select(&mut app, Selected::unit("acme@1 @hetzner", "web"));
    app.update(Msg::Boxes(Ok(json!({"items": []}))));
    assert_eq!(app.projects(), ["acme", "shop"]);
    assert!(
        app.selected
            .as_ref()
            .is_some_and(|s| !s.lane.ends_with("@hetzner")),
        "{:?}",
        app.selected
    );
}
