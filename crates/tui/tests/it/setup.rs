//! The setup prompt: once per session, after the welcome and never over
//! another dialog, when the daemon reports a setup step missing or this side
//! finds the CA untrusted. Confirming closes the TUI to run `devlanes setup`.

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
use crate::fake::{FakeApi, daemon_status, mixed_lanes};

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
