//! The header's release notice: a newer release with its upgrade command, or a
//! daemon that runs another version. `u` hides it for good. Opening the TUI
//! asks the daemon for a release check. The welcome of the first launch is
//! kept with the same dismissals.

use crossterm::event::KeyCode;
use devlanes_tui::app::{Dialog, Effect, Msg};
use devlanes_tui::welcome::{DOCS_URL, KEY as WELCOME};
use devlanes_update::InstallMethod;
use serde_json::json;

use super::{TestResult, draw, key, loaded};
use crate::fake::{FakeApi, daemon_status, mixed_lanes};

#[tokio::test]
async fn the_welcome_shows_once_per_home() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 120, 40).await;
    assert!(app.dialog.is_none(), "not before the dismissals load");
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: Vec::new(),
    });
    assert!(matches!(app.dialog, Some(Dialog::Welcome(_))));
    let screen = draw(&app)?.backend().to_string();
    assert!(screen.contains(DOCS_URL), "{screen}");
    assert_eq!(app.link_targets(), [DOCS_URL]);
    // `o` opens the docs and keeps the welcome; other keys do nothing.
    let effects = app.update(key(KeyCode::Char('o')));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::OpenUrl(url) if url == DOCS_URL)),
        "{effects:?}"
    );
    app.update(key(KeyCode::Char('j')));
    assert!(matches!(app.dialog, Some(Dialog::Welcome(_))));
    let effects = app.update(key(KeyCode::Enter));
    assert!(app.dialog.is_none());
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::DismissNotice(k) if k == WELCOME)),
        "{effects:?}"
    );
    // A later status does not bring it back.
    app.update(Msg::Status(Ok(mixed_lanes())));
    assert!(app.dialog.is_none());

    // Dismissed in this home before: it never opens.
    let (mut app, _) = loaded(&api, 120, 40).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: vec![WELCOME.into()],
    });
    assert!(app.dialog.is_none());
    Ok(())
}

/// `update` as `daemon.status` reports a release `latest`, `bump` away.
fn update(latest: &str, bump: &str) -> serde_json::Value {
    json!({
        "latest": latest, "channel": "stable", "bump": bump, "state": "available",
        "checked_at": 1, "notes": [{"version": latest, "level": bump, "line": "Fixed a thing."}],
    })
}

/// `daemon.status` from a daemon of this version, with `update` for the
/// release and bump when given.
fn status(version: &str, latest: Option<(&str, &str)>) -> serde_json::Value {
    let mut status = daemon_status();
    status["version"] = json!(version);
    if let Some((latest, bump)) = latest {
        status["update"] = update(latest, bump);
    }
    status
}

#[tokio::test]
async fn newer_release_shows_until_dismissed() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    let own = devlanes_version::VERSION;
    app.update(Msg::DaemonInfo {
        status: Ok(status(own, Some(("99.0.0", "minor")))),
        doctor: Ok(json!({})),
    });
    assert_eq!(
        app.release_badge(),
        None,
        "no command before the install method is known"
    );
    app.update(Msg::Notices {
        method: InstallMethod::Homebrew,
        dismissed: vec![WELCOME.into()],
    });
    let (notice, text) = app.release_badge().ok_or("no notice")?;
    assert_eq!(notice, "update:99.0.0");
    assert_eq!(
        text,
        "99.0.0 available, breaking changes: brew upgrade devlanes-run/tap/devlanes"
    );
    // The top bar marks the daemon button; the daemon page says what to run.
    let screen = draw(&app)?.backend().to_string();
    let top = screen.lines().next().unwrap_or_default();
    // The empty doctor reply has one problem by `devlanes doctor`'s rules:
    // the state schema is unknown.
    assert!(top.contains("Daemon · 1 issue, update"), "{top}");
    app.update(Msg::Key(crossterm::event::KeyEvent::new(
        KeyCode::Char('D'),
        crossterm::event::KeyModifiers::SHIFT,
    )));
    let page = draw(&app)?.backend().to_string();
    assert!(
        page.contains("warn update  99.0.0 available, breaking changes · u hides it"),
        "{page}"
    );
    assert!(
        page.contains("fix: brew upgrade devlanes-run/tap/devlanes"),
        "{page}"
    );

    let effects = app.update(key(KeyCode::Char('u')));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::DismissNotice(k) if k == "update:99.0.0")),
        "{effects:?}"
    );
    assert_eq!(app.release_badge(), None);

    // A dismissal loaded from disk hides the same release, not a newer one.
    app.update(Msg::Notices {
        method: InstallMethod::Homebrew,
        dismissed: vec![WELCOME.into(), "update:99.0.0".into()],
    });
    assert_eq!(app.release_badge(), None);
    app.update(Msg::DaemonInfo {
        status: Ok(status(own, Some(("99.1.0", "minor")))),
        doctor: Ok(json!({})),
    });
    assert!(app.release_badge().is_some());
    Ok(())
}

#[tokio::test]
async fn a_patch_release_is_not_called_breaking() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    app.update(Msg::Notices {
        method: InstallMethod::Homebrew,
        dismissed: vec![WELCOME.into()],
    });
    app.update(Msg::DaemonInfo {
        status: Ok(status(devlanes_version::VERSION, Some(("99.0.1", "patch")))),
        doctor: Ok(json!({})),
    });
    let (_, text) = app.release_badge().ok_or("no notice")?;
    assert_eq!(
        text,
        "99.0.1 available: brew upgrade devlanes-run/tap/devlanes"
    );
    Ok(())
}

#[tokio::test]
async fn the_release_check_on_open_updates_the_badge() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: vec![WELCOME.into()],
    });
    // Before `daemon.status` answered, the answer asks for it.
    let effects = app.update(Msg::ReleaseChecked(Ok(update("99.0.1", "patch"))));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::FetchDaemon)),
        "{effects:?}"
    );
    app.update(Msg::DaemonInfo {
        status: Ok(status(devlanes_version::VERSION, None)),
        doctor: Ok(json!({})),
    });
    assert_eq!(app.release_badge(), None);
    app.update(Msg::ReleaseChecked(Ok(update("99.0.1", "patch"))));
    assert!(app.release_badge().is_some());
    // No newer release any more, or a failed check: the answer decides.
    app.update(Msg::ReleaseChecked(Ok(serde_json::Value::Null)));
    assert_eq!(app.release_badge(), None);
    Ok(())
}

#[tokio::test]
async fn opening_the_tui_asks_for_a_release_check_once() -> TestResult {
    use crossterm::event::{Event as TermEvent, KeyEvent, KeyModifiers};
    use std::sync::Arc;

    let mut fake = FakeApi::with_lanes(Vec::new());
    *fake.update.get_mut().map_err(|_| "poisoned")? = Some(update("99.0.1", "patch"));
    let api = Arc::new(fake);
    let (keys_tx, keys_rx) = tokio::sync::mpsc::channel::<std::io::Result<TermEvent>>(8);
    let events = Box::pin(futures_util::stream::unfold(keys_rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    }));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 24))?;
    let driver: Arc<dyn devlanes_tui::DaemonApi> = api.clone();
    let script = {
        let api = api.clone();
        async move {
            while !api
                .calls()
                .iter()
                .any(|call| call.starts_with("update.check"))
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
            keys_tx.send(Ok(TermEvent::Key(quit))).await.ok();
            keys_tx
        }
    };
    let options = super::hermetic_options();
    let run = devlanes_tui::drive(
        &mut terminal,
        driver,
        &options,
        events,
        Box::new(std::io::sink()),
    );
    let (result, _keys) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(run, script)
    })
    .await?;
    result?;
    let checks: Vec<String> = api
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("update.check"))
        .collect();
    assert_eq!(checks, ["update.check force=false"]);
    Ok(())
}

#[tokio::test]
async fn older_or_equal_release_and_matching_daemon_show_nothing() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: vec![WELCOME.into()],
    });
    app.update(Msg::DaemonInfo {
        status: Ok(status(devlanes_version::VERSION, Some(("0.0.1", "patch")))),
        doctor: Ok(json!({})),
    });
    assert_eq!(app.release_badge(), None);
    // Without a notice, `u` does nothing.
    let effects = app.update(key(KeyCode::Char('u')));
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::DismissNotice(_))),
        "{effects:?}"
    );
    Ok(())
}

#[tokio::test]
async fn daemon_on_another_version_is_reported() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    app.update(Msg::DaemonInfo {
        status: Ok(status("0.0.9", None)),
        doctor: Ok(json!({})),
    });
    let (notice, text) = app.release_badge().ok_or("no notice")?;
    assert_eq!(notice, format!("skew:0.0.9:{}", devlanes_version::VERSION));
    assert!(text.starts_with("daemon 0.0.9, TUI"), "{text}");
    assert!(text.contains("(stops units)"), "{text}");
    Ok(())
}
