//! Boxes in the TUI: box projects after the local ones, read-only lanes
//! whose keys say where to act instead, the details pane and the boxes view.

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use devlanes_proto::remote::MIRROR_EVENT;
use devlanes_tui::app::{App, Effect, Focus, Msg, RowId, Section, Selected, View};
use ratatui::{Terminal, backend::TestBackend};
use serde_json::{Value, json};

use super::{TestResult, draw, key, loaded, select, shifted};
use crate::fake::{FakeApi, mixed_lanes};

/// `host.list` with `hetzner` connected over ssh, running `acme@1` with a
/// mirrored `web`, a `worker` whose port is taken here, a task and a `db`
/// container whose port cannot be bound here, and `devbox` reached by a
/// command, waiting for a sign-in.
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
                                "message": "port 24101 is not mirrored because postgres (pid 812) uses it here"}]},
                {"port": 24110, "lane": "acme@1", "unit": "db", "kind": "container",
                 "state": "failed", "addresses": [], "relay": null, "last_error": null,
                 "conflicts": [{"holder": null,
                                "message": "port 24110 is not mirrored because binding it on 127.0.0.1 failed: access denied"}]},
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
                    unit("container", "db", "running", json!([{"name": "5432", "port": 24110}]), json!([])),
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

/// The header's notice.
fn notice(app: &App) -> String {
    app.notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default()
}

/// The refusal line the pane shows first.
fn refusal(app: &App) -> String {
    app.refusal
        .as_ref()
        .map(|(_, text)| text.clone())
        .unwrap_or_default()
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
    // The footer's note in the form that fits.
    let footer = text.lines().last().unwrap_or_default();
    assert!(footer.contains("   read-only"), "{footer}");
    assert!(!footer.contains('…'), "the note is never cut: {footer}");
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
        (
            key(KeyCode::Char('s')),
            "s does not start it here.",
            "devlanes start acme@1:web",
        ),
        (
            key(KeyCode::Char('x')),
            "x does not stop it here.",
            "devlanes stop acme@1:web",
        ),
        (
            key(KeyCode::Char('r')),
            "r does not restart it here.",
            "devlanes restart acme@1:web",
        ),
        (
            shifted('S'),
            "S does not start the lane here.",
            "devlanes start --lane acme@1",
        ),
        (
            key(KeyCode::Char('p')),
            "p does not start the lane with profiles here.",
            "devlanes start --lane acme@1 --profile <name>",
        ),
        (
            shifted('C'),
            "C does not clean the lane here.",
            "devlanes lane clean acme@1",
        ),
        (
            key(KeyCode::Enter),
            "enter has nothing to type into here.",
            "devlanes attach acme@1:web",
        ),
        (
            key(KeyCode::Char('c')),
            "c has no output to select here.",
            "devlanes logs acme@1:web",
        ),
        (
            key(KeyCode::PageUp),
            "pgup has no output to scroll here.",
            "devlanes logs acme@1:web",
        ),
    ];
    for (msg, refused, command) in cases {
        let shown = format!("{msg:?}");
        let effects = app.update(msg);
        assert!(!acts(&effects), "{shown}: {effects:?}");
        assert!(app.dialog.is_none(), "{shown} opened a dialog");
        assert_eq!(
            notice(&app),
            format!("on hetzner: {command}"),
            "{shown}: the header's short form"
        );
        assert_eq!(
            refusal(&app),
            format!("{refused} On hetzner: ssh hetzner, then {command}"),
            "{shown}: the pane's first line"
        );
    }
    let text = screen_text(&draw(&app)?);
    assert!(
        text.contains("! pgup has no output to scroll here."),
        "{text}"
    );
    // A box task runs again on the box; the lane stops there.
    select(&mut app, Selected::unit("acme@1 @hetzner", "migrate"));
    app.update(key(KeyCode::Char('r')));
    assert_eq!(
        notice(&app),
        "on hetzner: devlanes run migrate --lane acme@1"
    );
    assert!(refusal(&app).starts_with("r does not run it again here."));
    select(&mut app, Selected::lane("acme@1 @hetzner"));
    let effects = app.update(key(KeyCode::Char('x')));
    assert!(!acts(&effects), "{effects:?}");
    assert_eq!(notice(&app), "on hetzner: devlanes stop --lane acme@1");
    assert!(refusal(&app).starts_with("x does not stop the lane here."));
    // The line goes with the notice.
    for _ in 0..10 {
        app.update(Msg::Tick);
    }
    assert!(app.refusal.is_none() && app.notice.is_none());
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
        "Boxes · 2 issues",
        "connected since 2026-10-14 ",
        "proxy on 443 · ports.range 24000-24999",
        "needs a sign-in · command",
        "fix       devlanes remote connect devbox",
        "the box runs devlanes 0.0.1",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(!text.contains('`'), "backticks stripped: {text}");
    assert_screen!("boxes_view", draw(&app)?.backend());
    // B again, or esc, goes back.
    app.update(shifted('B'));
    assert_eq!(app.view, View::Project);
    app.update(shifted('B'));
    assert_eq!(app.view, View::Boxes);
    app.update(key(KeyCode::Esc));
    assert_eq!(app.view, View::Project);
    Ok(())
}

#[tokio::test]
async fn the_boxes_view_says_what_connecting_does_without_boxes() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 100, 24).await;
    app.update(shifted('B'));
    let text = screen_text(&draw(&app)?);
    assert!(text.contains("no boxes connected"), "{text}");
    assert!(
        text.contains("devlanes remote connect <box>   mirrors a box's lanes here"),
        "{text}"
    );
    Ok(())
}

#[tokio::test]
async fn a_box_the_restore_could_not_reopen_shows_as_failed_with_its_error() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 100, 30).await;
    app.update(shifted('B'));
    // What `host.list` lists for a remembered box whose command left the
    // user config before the daemon restarted.
    app.update(Msg::Boxes(Ok(json!({"items": [{
        "host": "devbox", "transport": "command", "address": "devbox",
        "daemon_version": "devlanes 0.0.1", "last_connected_ms": 1_792_000_000_000u64,
        "state": "failed",
        "error": "the user config no longer has remote.boxes.devbox.command for box devbox. \
                  Add it back and run `devlanes remote connect devbox`, or run \
                  `devlanes remote disconnect devbox`",
    }]}))));
    let text = screen_text(&draw(&app)?);
    for expected in [
        "Boxes · 1 issue",
        "failed · command",
        "failed, so nothing is mirrored",
        "remote.boxes.devbox.command",
        "fix       devlanes remote connect devbox",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert_eq!(
        app.projects(),
        ["acme", "shop"],
        "no box project without a link"
    );
    Ok(())
}

#[tokio::test]
async fn sections_and_containers_of_a_box_lane_show_their_details() -> TestResult {
    let mut app = with_boxes(100, 24).await;
    let effects = select(
        &mut app,
        Selected {
            lane: "acme@1 @hetzner".into(),
            row: RowId::Section(Section::Services),
        },
    );
    assert!(!acts(&effects), "{effects:?}");
    let text = screen_text(&draw(&app)?);
    assert!(text.contains("name    state     url"), "{text}");
    assert!(text.contains("https://web.acme-1.localhost"), "{text}");
    assert!(!text.contains("memory"), "no metrics for box units: {text}");
    // Enter folds a section of a box lane as of any lane.
    app.update(key(KeyCode::Enter));
    assert!(
        app.rows()
            .iter()
            .any(|row| row.id == RowId::Section(Section::Services) && row.folded == Some(true))
    );
    let effects = select(
        &mut app,
        Selected {
            lane: "acme@1 @hetzner".into(),
            row: RowId::Container("db".into()),
        },
    );
    assert!(
        !acts(&effects),
        "no docker logs for a box container: {effects:?}"
    );
    let text = screen_text(&draw(&app)?);
    assert!(
        text.contains("binding") && text.contains("access denied"),
        "a failed bind shows the daemon's words: {text}"
    );
    assert!(!text.contains("in use here"), "{text}");
    assert!(text.contains("Read-only here."), "{text}");
    app.update(key(KeyCode::Char('s')));
    assert_eq!(notice(&app), "on hetzner: devlanes start --lane acme@1");
    Ok(())
}

#[tokio::test]
async fn a_port_bound_on_ipv6_alone_opens_there() -> TestResult {
    let mut boxes = hosts();
    boxes["items"][0]["ports"][0]["addresses"] = json!(["::1"]);
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 100, 24).await;
    app.update(Msg::Boxes(Ok(boxes)));
    select(&mut app, Selected::unit("acme@1 @hetzner", "web"));
    let text = screen_text(&draw(&app)?);
    assert!(text.contains("mirrored here (IPv6 only)"), "{text}");
    assert!(
        app.link_targets()
            .contains(&"http://[::1]:24100".to_owned()),
        "{:?}",
        app.link_targets()
    );
    Ok(())
}

#[tokio::test]
async fn a_click_in_a_box_units_pane_does_nothing() -> TestResult {
    let mut app = with_boxes(100, 24).await;
    select(&mut app, Selected::unit("acme@1 @hetzner", "web"));
    draw(&app)?;
    let pane = app.areas().ok_or("no layout")?.pane_inner;
    let at = |kind| {
        Msg::Mouse(MouseEvent {
            kind,
            column: pane.x + 2,
            row: pane.y + 2,
            modifiers: KeyModifiers::NONE,
        })
    };
    let mut effects = app.update(at(MouseEventKind::Down(MouseButton::Left)));
    effects.extend(app.update(at(MouseEventKind::Up(MouseButton::Left))));
    assert!(!acts(&effects), "{effects:?}");
    assert!(app.notice.is_none() && app.refusal.is_none());
    assert_eq!(app.focus, Focus::List);
    Ok(())
}

#[tokio::test]
async fn a_box_lane_that_goes_away_leaves_the_nearest_one_of_its_box() {
    // `user@host` box names hold an `@` of their own.
    let lane =
        |index: u32| json!({"key": {"project": "acme", "lane": index.to_string()}, "index": index});
    let listing = |indexes: &[u32]| {
        json!({"items": [{"host": "me@gpu", "state": "connected", "transport": "ssh",
            "registry": {"lanes": indexes.iter().map(|i| lane(*i)).collect::<Vec<_>>(), "units": []}}]})
    };
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 100, 24).await;
    app.update(Msg::Boxes(Ok(listing(&[0, 3, 5]))));
    select(&mut app, Selected::lane("acme@3 @me@gpu"));
    app.update(Msg::Boxes(Ok(listing(&[0, 5]))));
    assert_eq!(
        app.selected.as_ref().map(|s| s.lane.as_str()),
        Some("acme@5 @me@gpu")
    );
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
