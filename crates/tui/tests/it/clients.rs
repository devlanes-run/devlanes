//! This machine as a box in the boxes view: the machines paired with it
//! and the ones connected now, revoking one, and the daemon page without
//! them.

use std::sync::Arc;

use crossterm::event::KeyCode;
use devlanes_proto::{Event, RpcError, events, remote::CLIENTS_EVENT};
use devlanes_tui::{
    ApiError, DaemonApi,
    app::{App, BoxPart, Dialog, Effect, Msg, View},
};
use serde_json::{Value, json};

use crate::daemon::{Step, daemon_app, presses, run_keys, shifted};
use crate::errors::{assert_clean, key, render, screen};
use crate::fake::{FakeApi, mixed_lanes};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The daemon's clock in [`clients`].
const NOW_MS: u64 = 1_792_000_000_000;
const LAPTOP: &str = "dl_cid_3f2a9c1e5b7d40a8c6e2f1b9d3a7c5e0b8f4d2a6c1e9b7d5f3a0c8e6b4d2f1a9";
const TABLET: &str = "dl_cid_e5d8a1c4b7f02e6d9a3c8b1f5e4d7a0c2b9f6e3d8a5c1b4f7e0d3a6c9b2f5e8d1";
const DESKTOP: &str = "dl_cid_7c41e0a9b2d85f3c6e1a4d7b0c9f2e5a8d3b6c1f4e7a0d9c2b5e8f1a4d7c0b3e";

/// The laptop connected with two pairings (two box names).
fn laptop() -> Value {
    json!({"id": LAPTOP, "name": "laptop", "paired": true, "pairings": 2,
           "paired_at_ms": NOW_MS - 604_800_000, "last_seen_ms": NOW_MS - 8_040_000,
           "connections": [{"address": "203.0.113.24", "since_ms": NOW_MS - 8_040_000,
                            "mirror": {"ports": 6, "bound": 5, "skipped_lanes": 1}}]})
}

/// A tablet connected without pairing.
fn tablet() -> Value {
    json!({"id": TABLET, "name": "tablet", "paired": false, "pairings": 0,
           "paired_at_ms": null, "last_seen_ms": null,
           "connections": [{"address": "198.51.100.7", "since_ms": NOW_MS - 300_000,
                            "mirror": null}]})
}

/// A desktop paired and away for three days.
fn desktop() -> Value {
    json!({"id": DESKTOP, "name": "desktop", "paired": true, "pairings": 1,
           "paired_at_ms": NOW_MS - 1_209_600_000, "last_seen_ms": NOW_MS - 266_400_000,
           "connections": []})
}

/// `remote.clients` with `items` in that order, connections allowed.
fn list(require_pairing: bool, items: Vec<Value>) -> Value {
    json!({"now_ms": NOW_MS, "require_pairing": require_pairing, "allow_connections": true,
           "items": items})
}

/// `remote.clients` with the laptop, the tablet and the desktop, connected
/// ones first, then by name, as the daemon sorts them.
fn clients(require_pairing: bool) -> Value {
    list(require_pairing, vec![laptop(), tablet(), desktop()])
}

/// The boxes view, with no box and `value` as this machine's clients.
async fn with_clients(
    width: u16,
    height: u16,
    value: Value,
) -> Result<App, Box<dyn std::error::Error>> {
    let api = FakeApi::with_lanes(mixed_lanes());
    let mut app = App::new(width, height);
    app.update(Msg::Status(api.status().await));
    app.update(Msg::Subscribed(Ok(())));
    app.update(shifted('B'));
    assert_eq!(app.view, View::Boxes);
    app.update(Msg::Boxes(Ok(json!({"items": []}))));
    app.update(Msg::Clients(Ok(value)));
    Ok(app)
}

/// How many `remote.clients` fetches `effects` hold.
fn fetches(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|e| matches!(e, Effect::FetchClients))
        .count()
}

/// A subscription event named `name`.
fn event(name: &str) -> Msg {
    Msg::Event(Event {
        seq: 1,
        event: name.into(),
        data: json!({}),
    })
}

/// The notice's text, empty without one.
fn notice(app: &App) -> String {
    app.notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default()
}

/// The selected client's name.
fn selected(app: &App) -> Option<String> {
    app.boxes.selected_client().map(|c| c.name.clone())
}

/// Opening the boxes view asks for the clients, once at a time; the daemon
/// page no longer does.
#[tokio::test]
async fn the_boxes_view_asks_for_the_clients_once_at_a_time() -> TestResult {
    let mut app = App::new(120, 40);
    app.update(Msg::Subscribed(Ok(())));
    assert_eq!(fetches(&app.update(shifted('D'))), 0, "the daemon page");
    app.update(key(KeyCode::Esc));
    let effects = app.update(shifted('B'));
    assert_eq!(fetches(&effects), 1, "{effects:?}");
    // A change while the fetch is out asks again once it returns.
    let effects = app.update(event(CLIENTS_EVENT));
    assert_eq!(fetches(&effects), 0, "{effects:?}");
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::FetchBoxes)),
        "the clients are no news for host.list: {effects:?}"
    );
    let effects = app.update(Msg::Clients(Ok(clients(true))));
    assert_eq!(fetches(&effects), 1, "{effects:?}");
    let effects = app.update(Msg::Clients(Ok(clients(true))));
    assert_eq!(fetches(&effects), 0, "{effects:?}");
    // A slow poll while the view is open, for the times.
    let ticks: usize = (0..15).map(|_| fetches(&app.update(Msg::Tick))).sum();
    assert_eq!(ticks, 1);
    // A resync may have missed a change.
    app.update(Msg::Clients(Ok(clients(true))));
    assert_eq!(fetches(&app.update(event(events::RESYNC))), 1);
    // A change during that fetch waits for the view if it closed meanwhile.
    app.update(event(CLIENTS_EVENT));
    app.update(key(KeyCode::Esc));
    assert_eq!(app.view, View::Project);
    assert_eq!(fetches(&app.update(Msg::Clients(Ok(clients(true))))), 0);
    // Elsewhere, changes are left until the view opens.
    assert_eq!(fetches(&app.update(event(CLIENTS_EVENT))), 0);
    assert_eq!(fetches(&app.update(event(events::RESYNC))), 0);
    assert_eq!(fetches(&app.update(shifted('B'))), 1);
    Ok(())
}

/// Each client in one row, connected ones first: name, short id, address,
/// how long it is connected or when it was last seen, its pairings and what
/// it mirrors.
#[tokio::test]
async fn this_machine_lists_paired_and_connected_clients() -> TestResult {
    let app = with_clients(200, 60, clients(true)).await?;
    let buffer = render(&app)?;
    assert_clean(&buffer, "clients box");
    let text = screen(&buffer);
    for expected in [
        "this machine",
        "2 connected · 2 paired",
        "desktop  dl_cid_7c41e0a9",
        "last seen 3d ago",
        "laptop   dl_cid_3f2a9c1e  203.0.113.24  connected for 2h 14m  2 pairings  mirrors 5 of 6 ports, 1 lane skipped",
        "tablet   dl_cid_e5d8a1c4  198.51.100.7  connected for 5m      not paired",
        "no boxes yet",
    ] {
        assert!(text.contains(expected), "missing {expected:?}:\n{text}");
    }
    assert!(!text.contains("require_pairing"), "{text}");
    assert!(!text.contains("No other machine"), "{text}");
    let at: Vec<Option<usize>> = ["laptop   dl_cid", "tablet   dl_cid", "desktop  dl_cid"]
        .into_iter()
        .map(|row| text.find(row))
        .collect();
    assert!(
        at.windows(2).all(|pair| pair[0] < pair[1]) && at[0].is_some(),
        "the daemon's order:\n{text}"
    );
    // The times count on between answers, in whole minutes.
    let mut app = app;
    for _ in 0..59 {
        app.update(Msg::Tick);
    }
    assert!(screen(&render(&app)?).contains("connected for 5m "));
    app.update(Msg::Tick);
    assert!(screen(&render(&app)?).contains("connected for 6m "));
    Ok(())
}

/// At 80 columns the address and the mirror go first, and the note says
/// clients connect without a code.
#[tokio::test]
async fn this_machine_at_80x24() -> TestResult {
    let app = with_clients(80, 24, clients(false)).await?;
    let buffer = render(&app)?;
    assert_clean(&buffer, "clients at 80x24");
    let text = screen(&buffer);
    assert!(
        text.contains("remote.require_pairing is false, so clients connect without a code."),
        "{text}"
    );
    assert!(!text.contains("203.0.113.24"), "{text}");
    assert_screen!("boxes_clients_80x24", text);
    Ok(())
}

/// At 58 columns the pairing and then the id go too, so a client with two
/// connections still shows its name and how long it has been connected.
#[tokio::test]
async fn this_machine_at_58_columns_drops_the_pairing_and_the_id() -> TestResult {
    let mut twice = laptop();
    if let Some(connections) = twice["connections"].as_array_mut() {
        connections.push(json!({"address": null, "since_ms": NOW_MS - 600_000, "mirror": null}));
    }
    let app = with_clients(58, 24, list(true, vec![twice, tablet(), desktop()])).await?;
    let buffer = render(&app)?;
    assert_clean(&buffer, "clients at 58 columns");
    let text = screen(&buffer);
    assert!(
        text.contains("laptop   connected for 2h 14m (2 connections)"),
        "{text}"
    );
    assert!(text.contains("desktop  last seen 3d ago"), "{text}");
    for gone in ["dl_cid_", "pairings", "not paired", "mirrors"] {
        assert!(!text.contains(gone), "{gone:?} should be dropped:\n{text}");
    }
    Ok(())
}

/// Nobody paired or connected: one line says how to pair one, with the
/// note when pairing is off; with connections off it says so instead, and
/// why when invalid remote settings turned them off.
#[tokio::test]
async fn no_clients_says_how_to_pair_one() -> TestResult {
    let empty = "No other machine connects here yet. Press p for a pairing code.";
    let off = "Connections to this machine are off";
    for (value, expected, absent) in [
        (list(true, Vec::new()), empty, off),
        (
            list(false, Vec::new()),
            "remote.require_pairing is false, so clients connect without a code.",
            off,
        ),
        (
            json!({"now_ms": NOW_MS, "require_pairing": true, "allow_connections": false,
                   "items": []}),
            "Connections to this machine are off. Press o to turn them on.",
            empty,
        ),
        (
            json!({"now_ms": NOW_MS, "require_pairing": true, "allow_connections": false,
                   "remote_invalid": true, "items": []}),
            "Connections to this machine are off because config.yaml has invalid remote",
            "Press o to turn them on.",
        ),
    ] {
        let app = with_clients(120, 40, value).await?;
        let buffer = render(&app)?;
        assert_clean(&buffer, "no clients");
        let text = screen(&buffer);
        assert!(text.contains(expected), "{expected:?} missing:\n{text}");
        assert!(!text.contains(absent), "{absent:?} shown:\n{text}");
        assert!(text.contains("this machine"), "{text}");
    }
    Ok(())
}

/// A daemon without `remote.clients` lists no client and reports nothing.
#[tokio::test]
async fn a_daemon_without_remote_clients_lists_none() -> TestResult {
    let mut app = with_clients(120, 40, clients(true)).await?;
    app.update(Msg::Clients(Err(ApiError::Rpc(RpcError::new(
        "unknown_method",
        json!({"method": "remote.clients"}),
        2,
    )))));
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    assert_eq!(
        app.boxes.part,
        BoxPart::Hosts,
        "the selection left the list"
    );
    let text = screen(&render(&app)?);
    assert!(!text.contains("laptop"), "{text}");
    assert!(text.contains("Press p for a pairing code."), "{text}");
    Ok(())
}

/// A failed fetch is reported once while it repeats, and a lost connection
/// not at all: the subscription reports that.
#[tokio::test]
async fn a_failed_fetch_is_reported_once() -> TestResult {
    let mut app = with_clients(120, 40, clients(true)).await?;
    let failure = || {
        Msg::Clients(Err(ApiError::Rpc(RpcError::new(
            "io",
            json!({"message": "database is locked"}),
            1,
        ))))
    };
    app.update(failure());
    app.update(failure());
    let listed: Vec<&str> = app.errors.iter().map(|e| e.what.as_str()).collect();
    assert_eq!(listed, ["list clients"], "{:?}", app.errors);
    app.update(Msg::Clients(Err(ApiError::Disconnected("gone".into()))));
    assert_eq!(app.errors.len(), 1, "{:?}", app.errors);
    assert!(
        screen(&render(&app)?).contains("laptop"),
        "the last list stays"
    );
    Ok(())
}

/// `j k` pick a client and `x` asks before revoking it by id. Enter does
/// not confirm, a client connected without pairing has nothing to revoke,
/// and the dialog says whether the client needs a new code to come back.
#[tokio::test]
async fn x_asks_before_revoking_the_selected_client() -> TestResult {
    let mut app = with_clients(120, 40, clients(false)).await?;
    // Without boxes the selection starts on the clients.
    assert_eq!(app.boxes.part, BoxPart::Clients);
    assert_eq!(selected(&app).as_deref(), Some("laptop"));
    app.update(key(KeyCode::Char('j')));
    app.update(key(KeyCode::Char('x')));
    assert!(app.dialog.is_none());
    assert!(
        notice(&app).contains("tablet connected without pairing"),
        "{}",
        notice(&app)
    );
    app.update(key(KeyCode::Char('k')));
    app.update(key(KeyCode::Char('x')));
    match &app.dialog {
        Some(Dialog::Confirm { prompt, detail, .. }) => {
            assert_eq!(prompt, "Revoke laptop (dl_cid_3f2a9c1e)?");
            assert_eq!(
                detail,
                &[
                    "It deletes the client's 2 pairings and ends its connection now.".to_owned(),
                    "remote.require_pairing is false here, so it can reconnect without a code."
                        .to_owned()
                ]
            );
        }
        other => return Err(format!("no confirm dialog: {other:?}").into()),
    }
    let text = screen(&render(&app)?);
    assert_eq!(
        text.matches("laptop (dl_cid_3f2a9c1e)").count(),
        1,
        "{text}"
    );
    assert!(app.update(key(KeyCode::Enter)).is_empty());
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        effects.iter().any(
            |e| matches!(e, Effect::RevokeClient { id, name } if id == LAPTOP && name == "laptop")
        ),
        "{effects:?}"
    );

    // With pairing required it needs a new code.
    let mut app = with_clients(80, 24, clients(true)).await?;
    app.update(key(KeyCode::Char('x')));
    match &app.dialog {
        Some(Dialog::Confirm { detail, .. }) => {
            assert_eq!(detail[1], "It needs a new pairing code to connect again.");
        }
        other => return Err(format!("no confirm dialog: {other:?}").into()),
    }
    let buffer = render(&app)?;
    assert_clean(&buffer, "revoke dialog at 80x24");
    let text = screen(&buffer);
    assert!(
        text.contains("It needs a new pairing code to connect again."),
        "{text}"
    );
    Ok(())
}

/// One selection moves through the boxes, then the clients: `j` past the
/// last box reaches the first client and `k` goes back; `x` on a box says
/// where clients are.
#[tokio::test]
async fn one_selection_runs_through_boxes_and_clients() -> TestResult {
    let mut app = with_clients(120, 40, clients(true)).await?;
    app.update(Msg::Boxes(Ok(json!({"items": [
        {"host": "hetzner", "state": "connected", "transport": "ssh"},
        {"host": "gpu", "state": "needs_auth", "transport": "ssh"},
    ]}))));
    app.update(key(KeyCode::Char('g')));
    assert_eq!(
        app.boxes.selected_host().map(|h| h.host.as_str()),
        Some("hetzner")
    );
    app.update(key(KeyCode::Char('x')));
    assert!(app.dialog.is_none());
    assert_eq!(notice(&app), "select a client below to revoke it");
    app.update(key(KeyCode::Char('j')));
    app.update(key(KeyCode::Char('j')));
    assert_eq!(selected(&app).as_deref(), Some("laptop"));
    app.update(key(KeyCode::Char('k')));
    assert_eq!(
        app.boxes.selected_host().map(|h| h.host.as_str()),
        Some("gpu")
    );
    app.update(key(KeyCode::Char('G')));
    assert_eq!(selected(&app).as_deref(), Some("desktop"));
    Ok(())
}

/// PgUp and PgDn move the selection by a page of client rows.
#[tokio::test]
async fn pages_move_through_the_clients() -> TestResult {
    let many: Vec<Value> = (0..30)
        .map(|n| {
            let mut client = desktop();
            client["id"] = json!(format!("dl_cid_{n:064x}"));
            client["name"] = json!(format!("desk-{n:02}"));
            client
        })
        .collect();
    let mut app = with_clients(80, 24, list(true, many)).await?;
    render(&app)?;
    let page = usize::from(app.boxes.clients_rows.get().0.height);
    assert!(page > 1, "a page of rows: {page}");
    app.update(key(KeyCode::PageDown));
    assert_eq!(app.boxes.clients.selected, page);
    app.update(key(KeyCode::PageUp));
    assert_eq!(app.boxes.clients.selected, 0);
    let text = screen(&render(&app)?);
    assert!(text.contains("rows 1-"), "{text}");
    Ok(())
}

/// On the boxes, a page is the boxes the view shows, not the client rows.
#[tokio::test]
async fn pages_on_the_boxes_move_by_the_boxes_shown() -> TestResult {
    let mut app = with_clients(120, 40, clients(true)).await?;
    let hosts: Vec<Value> = (0..12)
        .map(|n| json!({"host": format!("box-{n:02}"), "state": "connected", "transport": "ssh"}))
        .collect();
    app.update(Msg::Boxes(Ok(json!({"items": hosts}))));
    app.update(key(KeyCode::Char('g')));
    render(&app)?;
    let shown = app.boxes.host_blocks.borrow().len();
    assert!((2..12).contains(&shown), "some boxes in view: {shown}");
    app.update(key(KeyCode::PageDown));
    assert_eq!(app.boxes.host, shown, "{:?}", app.boxes.part);
    app.update(key(KeyCode::PageUp));
    assert_eq!(app.boxes.host, 0);
    Ok(())
}

/// The selection follows its client by id when the list reorders; when the
/// client is gone the selection stays at its place, clamped to the list,
/// even when another client of the same name comes in.
#[tokio::test]
async fn the_selection_follows_its_client_across_refreshes() -> TestResult {
    let mut app = with_clients(120, 40, clients(true)).await?;
    app.update(key(KeyCode::Char('j')));
    assert_eq!(selected(&app).as_deref(), Some("tablet"));
    app.update(Msg::Clients(Ok(list(
        true,
        vec![tablet(), laptop(), desktop()],
    ))));
    assert_eq!(app.boxes.clients.selected, 0);
    assert_eq!(selected(&app).as_deref(), Some("tablet"));
    // Another tablet comes in while this one leaves: the place is kept.
    let mut other = tablet();
    other["id"] = json!(format!("dl_cid_{}", "0".repeat(64)));
    app.update(Msg::Clients(Ok(list(
        true,
        vec![laptop(), other, desktop()],
    ))));
    assert_eq!(app.boxes.clients.selected, 0);
    assert_eq!(selected(&app).as_deref(), Some("laptop"));
    app.update(key(KeyCode::Char('G')));
    assert_eq!(selected(&app).as_deref(), Some("desktop"));
    app.update(Msg::Clients(Ok(list(true, vec![laptop()]))));
    assert_eq!(app.boxes.clients.selected, 0);
    assert_eq!(selected(&app).as_deref(), Some("laptop"));
    // An emptied list sends the selection back to the boxes.
    app.update(Msg::Clients(Ok(list(true, Vec::new()))));
    assert_eq!(app.boxes.part, BoxPart::Hosts);
    Ok(())
}

/// A revoke says what it did and fetches the list again; a refused one is
/// reported with the daemon's words and fetches it too.
#[tokio::test]
async fn a_revoke_reports_what_happened() -> TestResult {
    let mut app = with_clients(120, 40, clients(true)).await?;
    let effects = app.update(Msg::Revoked {
        name: "laptop".into(),
        result: Ok(json!({"revoked": [{"id": LAPTOP, "name": "laptop"}], "ended": 1})),
    });
    assert_eq!(notice(&app), "revoked laptop and ended its connection");
    assert_eq!(fetches(&effects), 1, "{effects:?}");
    app.update(Msg::Clients(Ok(clients(true))));
    let effects = app.update(Msg::Revoked {
        name: "laptop".into(),
        result: Err(ApiError::Rpc(RpcError::new(
            "client_not_found",
            json!({"client": LAPTOP, "unpaired": false, "candidates": []}),
            3,
        ))),
    });
    assert_eq!(fetches(&effects), 1, "{effects:?}");
    let error = app.errors.front().ok_or("no error")?;
    assert_eq!(error.what, "revoke laptop");
    assert_eq!(
        error.worded.summary,
        "the client has no pairing here any more"
    );
    assert_eq!(
        error.worded.hint.as_deref(),
        Some("devlanes daemon clients")
    );
    Ok(())
}

/// A revoke that loses the daemon says the outcome is unknown, then the
/// connection loss is reported as any other.
#[tokio::test]
async fn a_revoke_that_loses_the_daemon_has_an_unknown_outcome() -> TestResult {
    let mut app = with_clients(120, 40, clients(true)).await?;
    let effects = app.update(Msg::Revoked {
        name: "laptop".into(),
        result: Err(ApiError::Disconnected("connection reset".into())),
    });
    // Newest first: the revoke was reported, then the lost connection.
    let listed: Vec<&str> = app.errors.iter().map(|e| e.what.as_str()).collect();
    assert_eq!(
        listed,
        ["daemon connection", "revoke laptop"],
        "{:?}",
        app.errors
    );
    assert!(
        app.errors[1]
            .worded
            .summary
            .contains("Check the clients in the boxes view after the TUI reconnects"),
        "{:?}",
        app.errors[1]
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::ScheduleReconnect { .. })),
        "{effects:?}"
    );
    Ok(())
}

/// Stopping the daemon empties the clients and keeps `x` from asking; the
/// next daemon lists them again.
#[tokio::test]
async fn a_stopped_daemon_has_no_clients() -> TestResult {
    let mut app = with_clients(120, 40, clients(true)).await?;
    app.update(Msg::DaemonStopped(Ok(json!({}))));
    assert!(app.boxes.clients.list.is_none());
    assert_eq!(app.boxes.part, BoxPart::Hosts);
    assert!(!screen(&render(&app)?).contains("laptop"));
    app.update(key(KeyCode::Char('x')));
    assert!(app.dialog.is_none());
    // Nothing asks a stopped daemon, not even a change that came in before.
    assert_eq!(fetches(&app.update(event(CLIENTS_EVENT))), 0);
    let ticks: usize = (0..15).map(|_| fetches(&app.update(Msg::Tick))).sum();
    assert_eq!(ticks, 0);
    assert_eq!(fetches(&app.update(Msg::Subscribed(Ok(())))), 1);
    Ok(())
}

/// A click on a client selects it; the wheel moves the selection.
#[tokio::test]
async fn the_mouse_selects_a_client() -> TestResult {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = with_clients(120, 40, clients(true)).await?;
    render(&app)?;
    let (rows, first) = app.boxes.clients_rows.get();
    assert_eq!(first, 0);
    let mouse = |kind, row| {
        Msg::Mouse(MouseEvent {
            kind,
            column: rows.x + 4,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        })
    };
    app.update(mouse(MouseEventKind::ScrollDown, rows.y));
    assert_eq!(selected(&app).as_deref(), Some("tablet"));
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), rows.y + 2));
    assert_eq!(selected(&app).as_deref(), Some("desktop"));
    Ok(())
}

/// The daemon page is back to its facts, checks and log: no clients box
/// and no remote row, whatever clients this machine has.
#[tokio::test]
async fn the_daemon_page_has_no_clients() -> TestResult {
    let mut app = daemon_app(120, 40).await?;
    app.update(Msg::Clients(Ok(clients(true))));
    let text = screen(&render(&app)?);
    for gone in ["remote clients", "laptop", "no clients", "daemon pair"] {
        assert!(!text.contains(gone), "{gone:?} on the daemon page:\n{text}");
    }
    assert!(text.contains("daemon.log"), "{text}");
    Ok(())
}

/// The whole loop: `x` and `y` on laptop send `remote.revoke` with its id,
/// and the list fetched after it no longer has it.
#[tokio::test]
async fn drive_revokes_the_selected_client_and_the_list_updates() -> TestResult {
    let fake = FakeApi::with_lanes(mixed_lanes());
    *fake.clients.lock().map_err(|e| e.to_string())? = Some(clients(true));
    let api = Arc::new(fake);
    let mut steps = presses(vec![shifted('B')]);
    steps.push(Step::Screen("laptop"));
    steps.extend(presses(vec![
        key(KeyCode::Char('x')),
        key(KeyCode::Char('y')),
    ]));
    let revoke = format!("remote.revoke {LAPTOP}");
    let terminal = run_keys(Arc::clone(&api), steps, &["1 paired"], |calls| {
        calls
            .iter()
            .skip_while(|c| **c != revoke)
            .any(|c| c == "remote.clients")
    })
    .await?;
    let calls = api.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("remote.revoke"))
            .count(),
        1,
        "{calls:?}"
    );
    let text = screen(terminal.buffer());
    assert!(!text.contains("dl_cid_3f2a9c1e"), "{text}");
    assert!(text.contains("desktop"), "{text}");
    Ok(())
}
