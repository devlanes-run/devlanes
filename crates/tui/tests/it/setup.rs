//! The setup prompt: once per session, after the welcome and never over
//! another dialog, when the daemon reports a setup step missing or this side
//! finds the CA untrusted, and a lane on screen needs it. Confirming closes
//! the TUI to run `devlanes setup`; turning it down waits a day.

use std::{sync::Arc, time::Duration};

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use devlanes_tui::welcome::KEY as WELCOME;
use devlanes_tui::{
    ExitReason,
    app::{App, Dialog, Effect, Msg},
};
use devlanes_update::InstallMethod;
use futures_util::stream;
use ratatui::Terminal;
use serde_json::json;
use tokio::sync::mpsc;

use super::{SharedBackend, TestResult, draw, hermetic_options, key, loaded};
use crate::fake::{FakeApi, daemon_status, lane, mixed_lanes, unit};

/// Sends `daemon.status` with `missing` as its `setup.missing`.
fn daemon_reports(app: &mut App, missing: &[&str]) {
    let mut status = daemon_status();
    status["setup"] = json!({"missing": missing});
    app.update(Msg::DaemonInfo {
        status: Ok(status),
        doctor: Ok(json!({})),
    });
}

/// The open confirm's prompt and detail, if one is open.
fn confirm(app: &App) -> Option<(&str, &[String])> {
    match &app.dialog {
        Some(Dialog::Confirm { prompt, detail, .. }) => Some((prompt, detail)),
        _ => None,
    }
}

#[tokio::test]
async fn the_prompt_follows_the_welcome_and_shows_once() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 40).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: Vec::new(),
    });
    app.update(Msg::SetupMissing(vec!["ca".into()]));
    daemon_reports(&mut app, &["hosts"]);
    assert!(matches!(app.dialog, Some(Dialog::Welcome(_))));
    app.update(key(KeyCode::Enter));
    let (prompt, detail) = confirm(&app).ok_or("no setup prompt")?;
    assert_eq!(prompt, "Set up HTTPS and hostnames?");
    assert_eq!(
        detail,
        [
            "HTTPS: the devlanes CA is not trusted, so browsers warn on https:// addresses.",
            "Hostnames: a lane needs the /etc/hosts helper, which is not installed.",
            "devlanes setup runs in this terminal, then the TUI opens again on the same lane.",
            "It may ask for your password once.",
        ]
    );
    let screen = draw(&app)?.backend().to_string();
    assert!(screen.contains("Set up HTTPS and hostnames?"), "{screen}");
    // No keeps the TUI as it is, for the rest of the session.
    app.update(key(KeyCode::Char('n')));
    assert!(app.dialog.is_none());
    assert_eq!(app.exit, None);
    app.update(Msg::Tick);
    daemon_reports(&mut app, &["hosts"]);
    assert!(app.dialog.is_none());
    Ok(())
}

#[tokio::test]
async fn confirming_closes_the_tui_to_set_up() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 40).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: vec![WELCOME.into()],
    });
    daemon_reports(&mut app, &[]);
    app.update(Msg::SetupMissing(vec!["ca".into()]));
    // Enter confirms: nothing stops.
    let effects = app.update(key(KeyCode::Enter));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Exit(ExitReason::Setup))),
        "{effects:?}"
    );
    let (reason, reopen) = app.relaunch().ok_or("no relaunch")?;
    assert_eq!(reason, ExitReason::Setup);
    assert_eq!(reopen.lane.as_deref(), Some("acme@0"));
    Ok(())
}

#[tokio::test]
async fn it_waits_for_both_checks_and_shows_nothing_when_all_is_set_up() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 40).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: vec![WELCOME.into()],
    });
    daemon_reports(&mut app, &["resolver"]);
    assert!(app.dialog.is_none(), "this side has not answered yet");
    app.update(Msg::SetupMissing(Vec::new()));
    let (_, detail) = confirm(&app).ok_or("no setup prompt")?;
    assert_eq!(
        detail[0],
        "Hostnames: hostnames.tld does not reach the devlanes DNS server."
    );

    let (mut app, _) = loaded(&api, 140, 40).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: vec![WELCOME.into()],
    });
    app.update(Msg::SetupMissing(Vec::new()));
    daemon_reports(&mut app, &[]);
    app.update(Msg::Tick);
    assert!(app.dialog.is_none());
    Ok(())
}

#[tokio::test]
async fn it_never_opens_over_another_dialog() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 40).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: vec![WELCOME.into()],
    });
    app.update(key(KeyCode::Char('?')));
    app.update(Msg::SetupMissing(vec!["ca".into()]));
    daemon_reports(&mut app, &[]);
    assert!(matches!(app.dialog, Some(Dialog::Help)));
    app.update(key(KeyCode::Esc));
    assert!(app.dialog.is_none());
    // The next second brings it.
    app.update(Msg::Tick);
    assert!(confirm(&app).is_some(), "{:?}", app.dialog);
    Ok(())
}

/// A loaded app over one acme lane whose api serves `urls`, with the
/// welcome gone and `missing` reported by this side and the daemon.
async fn serving(urls: &[&str], held: &[&str], missing: &[&str], daemon: &[&str]) -> App {
    let mut api_unit = unit("acme@0", "api", "ready");
    api_unit.urls = urls.iter().map(|url| (*url).to_owned()).collect();
    let fake = FakeApi::with_lanes(vec![lane(
        "acme",
        Some(0),
        "main",
        "running",
        vec![api_unit],
    )]);
    let (mut app, _) = loaded(&fake, 140, 40).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: std::iter::once(WELCOME)
            .chain(held.iter().copied())
            .map(str::to_owned)
            .collect(),
    });
    app.update(Msg::SetupMissing(
        missing.iter().map(|step| (*step).to_owned()).collect(),
    ));
    daemon_reports(&mut app, daemon);
    app.update(Msg::Tick);
    app
}

/// The prompt names only what the project on screen needs: the CA for an
/// `https://` address, the hosts helper or resolver for a hostname.
#[tokio::test]
async fn it_shows_only_what_the_lanes_need() -> TestResult {
    let plain = ["http://127.0.0.1:20102", "http://localhost:3000"];
    let app = serving(&plain, &[], &["ca"], &["hosts", "resolver"]).await;
    assert!(app.dialog.is_none(), "{:?}", app.dialog);

    let app = serving(&["https://127.0.0.1:8443"], &[], &["ca"], &["hosts"]).await;
    let (_, detail) = confirm(&app).ok_or("no setup prompt")?;
    assert!(detail[0].starts_with("HTTPS:"), "{detail:?}");
    assert!(!detail.iter().any(|line| line.starts_with("Hostnames:")));

    let app = serving(
        &["http://api.acme.localhost:8080/x"],
        &[],
        &["ca"],
        &["hosts"],
    )
    .await;
    let (_, detail) = confirm(&app).ok_or("no setup prompt")?;
    assert!(detail[0].starts_with("Hostnames:"), "{detail:?}");
    assert!(!detail.iter().any(|line| line.starts_with("HTTPS:")));
    Ok(())
}

/// The notice log key lists only the steps the lanes need, as the one
/// `devlanes start` records, so either holds the other back.
#[tokio::test]
async fn the_key_names_only_the_steps_the_lanes_need() -> TestResult {
    let http = ["http://api.acme.localhost:8080"];
    let mut app = serving(&http, &[], &["ca"], &["hosts"]).await;
    let effects = app.update(key(KeyCode::Esc));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::NoticeShown(key) if key == "setup:hosts")),
        "{effects:?}"
    );
    let app = serving(&http, &["setup:hosts"], &["ca"], &["hosts"]).await;
    assert!(app.dialog.is_none(), "{:?}", app.dialog);
    Ok(())
}

/// Turned down, the same steps wait a day: the key goes to the notice log,
/// which also holds it back when `devlanes start` named them.
#[tokio::test]
async fn turning_it_down_waits_a_day() -> TestResult {
    let https = ["https://api.acme.localhost"];
    let mut app = serving(&https, &[], &["ca"], &["hosts"]).await;
    assert!(confirm(&app).is_some());
    let effects = app.update(key(KeyCode::Esc));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::NoticeShown(key) if key == "setup:ca,hosts")),
        "{effects:?}"
    );
    let app = serving(&https, &["setup:ca,hosts"], &["ca"], &["hosts"]).await;
    assert!(app.dialog.is_none(), "{:?}", app.dialog);
    // Other missing steps are another notice.
    let app = serving(&https, &["setup:ca,hosts"], &["ca"], &[]).await;
    assert!(confirm(&app).is_some());
    // Confirming records nothing: setup runs.
    let mut app = serving(&https, &[], &["ca"], &[]).await;
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::NoticeShown(_))),
        "{effects:?}"
    );
    Ok(())
}

#[tokio::test]
async fn the_loop_asks_this_side_and_closes_for_setup() -> TestResult {
    let mut fake = FakeApi::with_lanes(mixed_lanes());
    fake.setup = vec!["ca".into()];
    fake.daemon_setup = vec!["hosts".into()];
    let api = Arc::new(fake);
    let (keys_tx, keys_rx) = mpsc::channel::<std::io::Result<TermEvent>>(8);
    let events = Box::pin(stream::unfold(keys_rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    }));
    let backend = SharedBackend::new(140, 40);
    let mut terminal = Terminal::new(backend.clone())?;
    let driver: Arc<dyn devlanes_tui::DaemonApi> = api.clone();
    let script = {
        let backend = backend.clone();
        async move {
            // `daemon.status` comes with the first tick.
            backend.wait_for("Set up HTTPS and hostnames?").await;
            backend.wait_for("/etc/hosts helper").await;
            backend.wait_for("CA is not trusted").await;
            let yes = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
            keys_tx.send(Ok(TermEvent::Key(yes))).await.ok();
            keys_tx
        }
    };
    let options = hermetic_options();
    let run = devlanes_tui::drive(
        &mut terminal,
        driver,
        &options,
        events,
        Box::new(std::io::sink()),
    );
    let (result, _keys) =
        tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(run, script) }).await?;
    let (reason, _) = result?.ok_or("closed without a reason")?;
    assert_eq!(reason, ExitReason::Setup);
    let calls = api.calls();
    assert_eq!(
        calls.iter().filter(|c| *c == "setup.missing").count(),
        1,
        "{calls:?}"
    );
    Ok(())
}
