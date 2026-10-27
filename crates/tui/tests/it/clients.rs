//! The clients box on the daemon page: the machines paired with this one as
//! a box and the ones connected now, and revoking one.

use std::sync::Arc;

use crossterm::event::KeyCode;
use devlanes_proto::{Event, RpcError, events, remote::CLIENTS_EVENT};
use devlanes_tui::{
    ApiError,
    app::{App, DaemonBox, Dialog, Effect, Msg, View},
};
use serde_json::{Value, json};

use crate::daemon::{daemon_app, presses, run_keys, shifted};
use crate::errors::{assert_clean, key, render, screen};
use crate::fake::{FakeApi, mixed_lanes};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The daemon's clock in [`clients`].
const NOW_MS: u64 = 1_792_000_000_000;
const LAPTOP: &str = "dl_cid_3f2a9c1e5b7d40a8c6e2f1b9d3a7c5e0b8f4d2a6c1e9b7d5f3a0c8e6b4d2f1a9";

/// `remote.clients` with a laptop connected under two box names, a desktop
/// away for three days and a tablet connected without pairing, sorted by
/// name as the daemon sorts them.
fn clients(require_pairing: bool) -> Value {
    json!({
        "now_ms": NOW_MS,
        "require_pairing": require_pairing,
        "items": [
            {"id": "dl_cid_7c41e0a9b2d85f3c6e1a4d7b0c9f2e5a8d3b6c1f4e7a0d9c2b5e8f1a4d7c0b3e",
             "name": "desktop", "paired": true, "pairings": 1,
             "paired_at_ms": NOW_MS - 1_209_600_000, "last_seen_ms": NOW_MS - 266_400_000,
             "connections": []},
            {"id": LAPTOP, "name": "laptop", "paired": true, "pairings": 2,
             "paired_at_ms": NOW_MS - 604_800_000, "last_seen_ms": NOW_MS - 8_040_000,
             "connections": [{"address": "203.0.113.24", "since_ms": NOW_MS - 8_040_000,
                              "mirror": {"ports": 6, "bound": 5, "skipped_lanes": 1}}]},
            {"id": "dl_cid_e5d8a1c4b7f02e6d9a3c8b1f5e4d7a0c2b9f6e3d8a5c1b4f7e0d3a6c9b2f5e8d1",
             "name": "tablet", "paired": false, "pairings": 0,
             "paired_at_ms": null, "last_seen_ms": null,
             "connections": [{"address": "198.51.100.7", "since_ms": NOW_MS - 300_000,
                              "mirror": null}]},
        ],
    })
}

/// The daemon page of [`daemon_app`] with `value` as the clients.
async fn with_clients(
    width: u16,
    height: u16,
    value: Value,
) -> Result<App, Box<dyn std::error::Error>> {
    let mut app = daemon_app(width, height).await?;
    app.update(Msg::Clients(Ok(value)));
    Ok(app)
}

/// Opening the daemon page asks for the clients, once at a time.
#[tokio::test]
async fn the_daemon_page_asks_for_the_clients_once_at_a_time() -> TestResult {
    let mut app = App::new(120, 40);
    app.update(Msg::Subscribed(Ok(())));
    let effects = app.update(shifted('D'));
    let fetches = |effects: &[Effect]| {
        effects
            .iter()
            .filter(|e| matches!(e, Effect::FetchClients))
            .count()
    };
    assert_eq!(fetches(&effects), 1, "{effects:?}");
    let event = |name: &str| {
        Msg::Event(Event {
            seq: 1,
            event: name.into(),
            data: json!({}),
        })
    };
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
    // A slow poll while the page is open, for the times.
    let ticks: usize = (0..15).map(|_| fetches(&app.update(Msg::Tick))).sum();
    assert_eq!(ticks, 1);
    // Elsewhere, changes are left until the page opens.
    app.update(Msg::Clients(Ok(clients(true))));
    app.update(key(KeyCode::Esc));
    assert_eq!(app.view, View::Project);
    assert_eq!(fetches(&app.update(event(CLIENTS_EVENT))), 0);
    assert_eq!(fetches(&app.update(event(events::RESYNC))), 0);
    Ok(())
}

/// Each client in one row: name, short id, address, how long it is
/// connected or when it was last seen, its pairing and what it mirrors.
#[tokio::test]
async fn the_clients_box_lists_paired_and_connected_clients() -> TestResult {
    let app = with_clients(200, 60, clients(true)).await?;
    let buffer = render(&app)?;
    assert_clean(&buffer, "clients box");
    let text = screen(&buffer);
    for expected in [
        "remote clients",
        "2 connected · 2 paired",
        "desktop  dl_cid_7c41e0a9",
        "last seen 3d ago",
        "laptop   dl_cid_3f2a9c1e  203.0.113.24  connected for 2h 14m  paired under 2 names  mirrors 5 of 6 ports, 1 lane skipped",
        "connected for 5m 0s",
        "not paired",
    ] {
        assert!(text.contains(expected), "missing {expected:?}:\n{text}");
    }
    assert!(!text.contains("require_pairing"), "{text}");
    // The times count on between answers.
    let mut app = app;
    app.update(Msg::Tick);
    assert!(screen(&render(&app)?).contains("connected for 5m 1s"));
    Ok(())
}

/// At 80 columns the address and the mirror go first; the box keeps half
/// of the room below the checks and says which rows show.
#[tokio::test]
async fn the_clients_box_at_80x24() -> TestResult {
    let app = with_clients(80, 24, clients(false)).await?;
    let buffer = render(&app)?;
    assert_clean(&buffer, "clients box at 80x24");
    let text = screen(&buffer);
    assert!(
        text.contains("remote.require_pairing is false: clients connect without pairing."),
        "{text}"
    );
    assert!(!text.contains("203.0.113.24"), "{text}");
    assert_screen!("clients_box_80x24", text);
    Ok(())
}

/// Nobody paired or connected: one line saying how to pair one.
#[tokio::test]
async fn the_clients_box_says_how_to_pair_when_empty() -> TestResult {
    let app = with_clients(
        80,
        24,
        json!({"items": [], "now_ms": NOW_MS, "require_pairing": true}),
    )
    .await?;
    let text = screen(&render(&app)?);
    assert!(
        text.contains(
            "No client is paired or connected. devlanes remote pair prints a code for one."
        ),
        "{text}"
    );
    Ok(())
}

/// A daemon without `remote.clients` shows no box, and Tab skips it.
#[tokio::test]
async fn a_daemon_without_remote_clients_shows_no_box() -> TestResult {
    let mut app = daemon_app(120, 40).await?;
    app.update(Msg::Clients(Err(ApiError::Rpc(RpcError::new(
        "unknown_method",
        json!({"method": "remote.clients"}),
        2,
    )))));
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    assert!(!screen(&render(&app)?).contains("remote clients"));
    app.update(key(KeyCode::Tab));
    assert_eq!(app.daemon.focus, DaemonBox::Checks);
    app.update(key(KeyCode::Tab));
    assert_eq!(app.daemon.focus, DaemonBox::Log);
    Ok(())
}

/// Tab reaches the clients between the checks and the log; there `j k` pick
/// a client and `x` asks before revoking it by id. Enter does not confirm,
/// and a client connected without pairing has nothing to revoke.
#[tokio::test]
async fn x_asks_before_revoking_the_selected_client() -> TestResult {
    let mut app = with_clients(120, 40, clients(false)).await?;
    app.update(key(KeyCode::Char('x')));
    assert!(app.dialog.is_none(), "x on the log revokes nothing");
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    assert_eq!(app.daemon.focus, DaemonBox::Clients);
    app.update(key(KeyCode::Char('G')));
    app.update(key(KeyCode::Char('x')));
    assert!(app.dialog.is_none());
    let notice = app
        .notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default();
    assert!(
        notice.contains("tablet is connected without pairing"),
        "{notice}"
    );
    app.update(key(KeyCode::Char('k')));
    app.update(key(KeyCode::Char('x')));
    match &app.dialog {
        Some(Dialog::Confirm { title, detail, .. }) => {
            assert_eq!(title, "revoke laptop");
            assert!(
                detail[0].contains("pairings under 2 names and ends its connection now"),
                "{detail:?}"
            );
        }
        other => return Err(format!("no confirm dialog: {other:?}").into()),
    }
    let text = screen(&render(&app)?);
    assert!(text.contains("Revoke laptop (dl_cid_3f2a9c1e)?"), "{text}");
    assert!(app.update(key(KeyCode::Enter)).is_empty());
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        effects.iter().any(
            |e| matches!(e, Effect::RevokeClient { id, name } if id == LAPTOP && name == "laptop")
        ),
        "{effects:?}"
    );
    Ok(())
}

/// The whole loop: `x` and `y` on laptop send `remote.revoke` with its id,
/// and the list fetched after it no longer has it.
#[tokio::test]
async fn drive_revokes_the_selected_client_and_the_list_updates() -> TestResult {
    let fake = FakeApi::with_lanes(mixed_lanes());
    *fake.clients.lock().map_err(|e| e.to_string())? = Some(clients(true));
    let api = Arc::new(fake);
    let keys = vec![
        shifted('D'),
        key(KeyCode::Tab),
        key(KeyCode::Tab),
        key(KeyCode::Char('j')),
        key(KeyCode::Char('x')),
        key(KeyCode::Char('y')),
    ];
    let revoke = format!("remote.revoke {LAPTOP}");
    let terminal = run_keys(Arc::clone(&api), presses(keys), &["1 paired"], |calls| {
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
    assert!(text.contains("dl_cid_7c41e0a9"), "{text}");
    Ok(())
}
