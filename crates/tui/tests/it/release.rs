//! The header's version notice: a newer release, a daemon on another
//! version, and `U`, which asks before it closes the TUI to update, or
//! reloads it at once for a newer daemon. A daemon on another protocol
//! closes the TUI or says what fixes it. Opening the TUI asks the daemon for
//! a release check. The welcome of the first launch is kept with the
//! notice dismissals.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use devlanes_proto::Hello;
use devlanes_tui::welcome::{DOCS_URL, KEY as WELCOME};
use devlanes_tui::{
    ApiError, DaemonApi, ExitReason, Reopen,
    app::{App, DaemonState, Dialog, Effect, Msg, ReleaseNotice, View},
};
use devlanes_update::InstallMethod;
use serde_json::json;

use super::{TestResult, draw, footer, key, loaded, shifted};
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

/// `update` as `daemon.status` reports a release `latest`, `bump` away, in
/// `state`.
fn update(latest: &str, bump: &str, state: &str) -> serde_json::Value {
    json!({
        "latest": latest, "channel": "stable", "bump": bump, "state": state,
        "checked_at": 1,
        "notes": [
            {"version": latest, "level": "patch", "line": "Fixed a thing."},
            {"version": latest, "level": bump, "line": "Changed `start` to do more."},
        ],
    })
}

/// `daemon.status` from a daemon of this version, with `update` for the
/// release, bump and state when given.
fn status(version: &str, latest: Option<(&str, &str, &str)>) -> serde_json::Value {
    let mut status = daemon_status();
    status["version"] = json!(version);
    if let Some((latest, bump, state)) = latest {
        status["update"] = update(latest, bump, state);
    }
    status
}

/// An app on mixed lanes whose daemon reported `status`, the welcome closed.
async fn reported(status: serde_json::Value) -> App {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    app.update(Msg::Notices {
        method: InstallMethod::Homebrew,
        dismissed: vec![WELCOME.into()],
    });
    app.update(Msg::DaemonInfo {
        status: Ok(status),
        doctor: Ok(json!({})),
    });
    app
}

/// The top row of the screen.
fn header(app: &App) -> Result<String, Box<dyn std::error::Error>> {
    let screen = draw(app)?.backend().to_string();
    Ok(screen.lines().next().unwrap_or_default().to_owned())
}

#[tokio::test]
async fn a_newer_release_shows_in_the_header_by_its_state() -> TestResult {
    let own = devlanes_version::VERSION;
    for (bump, state, text) in [
        ("patch", "available", "99.0.1 available · U update"),
        (
            "minor",
            "available",
            "99.0.1 available, breaking · U update",
        ),
        ("patch", "installing", "99.0.1 installing…"),
        ("patch", "ready", "99.0.1 ready · U restart"),
        ("minor", "failed", "update failed · U retry"),
    ] {
        let app = reported(status(own, Some(("99.0.1", bump, state)))).await;
        let top = header(&app)?;
        assert!(top.contains(text), "{state}: {top}");
        // The daemon button counts issues only.
        assert!(top.contains("Daemon · 1 issue "), "{top}");
    }
    Ok(())
}

#[tokio::test]
async fn the_daemon_page_leaves_versions_to_the_header() -> TestResult {
    let own = devlanes_version::VERSION;
    let mut app = reported(status(own, Some(("99.0.0", "minor", "available")))).await;
    app.update(shifted('D'));
    let page = draw(&app)?.backend().to_string();
    assert!(!page.contains("warn update"), "{page}");
    assert!(!page.contains("brew upgrade"), "{page}");
    let footer = footer(&app)?;
    assert!(footer.starts_with(" U update"), "{footer}");
    // `u` leaves the release notice in place.
    app.update(key(KeyCode::Char('u')));
    assert!(app.release_notice().is_some());
    Ok(())
}

#[tokio::test]
async fn u_asks_with_the_notes_and_the_lanes_then_closes_to_update() -> TestResult {
    let own = devlanes_version::VERSION;
    let mut app = reported(status(own, Some(("99.0.0", "minor", "available")))).await;
    app.update(shifted('U'));
    let Some(Dialog::Confirm { prompt, detail, .. }) = &app.dialog else {
        panic!("no confirm: {:?}", app.dialog);
    };
    assert_eq!(prompt, "Update devlanes to 99.0.0?");
    assert_eq!(
        detail,
        &[
            "  Changed `start` to do more.",
            "  Fixed a thing.",
            "",
            "The daemon restarts and starts the 2 running lanes again.",
            "This TUI closes and opens again on the same lane.",
        ]
    );
    // Inline code is styled as "What's new" shows it, without the backticks.
    let screen = draw(&app)?.backend().to_string();
    assert!(screen.contains("Changed start to do more."), "{screen}");
    assert!(!screen.contains("`start`"), "{screen}");
    // Enter does not confirm a restart of every lane; n keeps the TUI.
    app.update(key(KeyCode::Enter));
    assert!(app.dialog.is_some());
    app.update(key(KeyCode::Char('n')));
    assert!(app.dialog.is_none());
    assert_eq!(app.exit, None);

    app.update(shifted('U'));
    let lane = app.selected.as_ref().map(|s| s.lane.clone());
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Exit(ExitReason::Update))),
        "{effects:?}"
    );
    assert_eq!(
        app.relaunch(),
        Some((
            ExitReason::Update,
            Reopen {
                lane,
                view: Some("project".into()),
            }
        ))
    );
    Ok(())
}

#[tokio::test]
async fn a_ready_or_failed_update_says_so_in_its_dialog() -> TestResult {
    let own = devlanes_version::VERSION;
    let mut ready = status(own, Some(("99.0.1", "patch", "ready")));
    ready["update"]["installed"] = json!("99.0.1");
    let mut app = reported(ready).await;
    app.update(shifted('U'));
    assert!(
        matches!(&app.dialog, Some(Dialog::Confirm { prompt, action: "restart", .. })
            if prompt == "Restart into devlanes 99.0.1?"),
        "{:?}",
        app.dialog
    );

    let mut failed = status(own, Some(("99.0.1", "patch", "failed")));
    failed["update"]["error"] = json!("npm exited with 1");
    let mut app = reported(failed).await;
    app.update(shifted('U'));
    let Some(Dialog::Confirm { detail, .. }) = &app.dialog else {
        panic!("no confirm: {:?}", app.dialog);
    };
    assert_eq!(detail[0], "The last try failed: npm exited with 1");
    Ok(())
}

#[tokio::test]
async fn the_header_click_presses_u() -> TestResult {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let own = devlanes_version::VERSION;
    let mut app = reported(status(own, Some(("99.0.1", "patch", "available")))).await;
    let top = header(&app)?;
    let column = top.find("99.0.1").ok_or("no notice")?;
    let column = u16::try_from(top[..column].chars().count())?;
    app.update(Msg::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row: 0,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(matches!(app.dialog, Some(Dialog::Confirm { .. })));
    Ok(())
}

#[tokio::test]
async fn the_release_check_on_open_updates_the_notice() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    app.update(Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: vec![WELCOME.into()],
    });
    // Before `daemon.status` answered, the answer asks for it.
    let effects = app.update(Msg::ReleaseChecked(Ok(update(
        "99.0.1",
        "patch",
        "available",
    ))));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::FetchDaemon)),
        "{effects:?}"
    );
    app.update(Msg::DaemonInfo {
        status: Ok(status(devlanes_version::VERSION, None)),
        doctor: Ok(json!({})),
    });
    assert_eq!(app.release_notice(), None);
    app.update(Msg::ReleaseChecked(Ok(update(
        "99.0.1",
        "patch",
        "available",
    ))));
    assert!(app.release_notice().is_some());
    // No newer release any more, or a failed check: the answer decides.
    app.update(Msg::ReleaseChecked(Ok(serde_json::Value::Null)));
    assert_eq!(app.release_notice(), None);
    Ok(())
}

#[tokio::test]
async fn opening_the_tui_asks_for_a_release_check_once() -> TestResult {
    use crossterm::event::Event as TermEvent;
    use std::sync::Arc;

    let mut fake = FakeApi::with_lanes(Vec::new());
    *fake.update.get_mut().map_err(|_| "poisoned")? = Some(update("99.0.1", "patch", "available"));
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
    assert_eq!(result?, None, "a plain quit relaunches nothing");
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
    let own = devlanes_version::VERSION;
    let mut app = reported(status(own, Some(("0.0.1", "patch", "available")))).await;
    assert_eq!(app.release_notice(), None);
    // Without a notice, `U` does nothing.
    let effects = app.update(shifted('U'));
    assert!(app.dialog.is_none());
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Exit(_))),
        "{effects:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_newer_daemon_reloads_the_tui_without_asking() -> TestResult {
    let own = devlanes_version::VERSION;
    let mut app = reported(status("99.0.0", None)).await;
    assert_eq!(
        app.release_notice(),
        Some(ReleaseNotice::Reload {
            daemon: "99.0.0".into()
        })
    );
    let top = header(&app)?;
    assert!(
        top.contains(&format!("daemon 99.0.0, TUI {own} · U reload")),
        "{top}"
    );
    app.update(shifted('M'));
    // It reads the installed version first: the same one stays open.
    let effects = app.update(shifted('U'));
    assert!(app.dialog.is_none());
    assert!(
        effects.iter().any(|e| matches!(e, Effect::CheckReload))
            && !effects.iter().any(|e| matches!(e, Effect::Exit(_))),
        "{effects:?}"
    );
    let effects = app.update(Msg::ReloadChecked(Some(own.into())));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Exit(_))),
        "{effects:?}"
    );
    assert_eq!(app.exit, None);
    let notice = app
        .notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default();
    assert!(
        notice.contains(&format!("the installed devlanes is {own}, not newer")),
        "{notice}"
    );
    app.update(shifted('U'));
    let effects = app.update(Msg::ReloadChecked(Some("99.0.0".into())));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Exit(ExitReason::Reload))),
        "{effects:?}"
    );
    let (reason, reopen) = app.relaunch().ok_or("no relaunch")?;
    assert_eq!(reason, ExitReason::Reload);
    assert_eq!(reopen.view.as_deref(), Some("monitor"));
    Ok(())
}

#[tokio::test]
async fn an_older_daemon_offers_a_restart_into_this_version() -> TestResult {
    let own = devlanes_version::VERSION;
    let mut app = reported(status("0.0.9", None)).await;
    let top = header(&app)?;
    assert!(top.contains(&format!("{own} ready · U restart")), "{top}");
    app.update(shifted('U'));
    assert!(
        matches!(&app.dialog, Some(Dialog::Confirm { prompt, .. })
            if *prompt == format!("Restart the daemon into devlanes {own}?")),
        "{:?}",
        app.dialog
    );
    Ok(())
}

/// The hello of a daemon on protocol `protocol`, running devlanes `version`.
fn hello(protocol: u32, version: &str) -> Hello {
    Hello {
        protocol,
        version: version.into(),
        build_id: "test".into(),
        features: Vec::new(),
        managed_processes: 3,
    }
}

#[tokio::test]
async fn a_daemon_on_a_newer_protocol_reloads_the_tui() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    let peer = hello(devlanes_proto::PROTOCOL_VERSION + 1, "99.0.0");
    let effects = app.update(Msg::Subscribed(Err(ApiError::Incompatible {
        peer: peer.clone(),
    })));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::CheckReload)),
        "{effects:?}"
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::ScheduleReconnect { .. })),
        "{effects:?}"
    );
    let effects = app.update(Msg::ReloadChecked(Some("99.0.0".into())));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Exit(ExitReason::Reload))),
        "{effects:?}"
    );
    assert_eq!(app.exit, Some(ExitReason::Reload));
    assert!(app.errors.is_empty(), "nothing to report: {:?}", app.errors);

    // An install no newer than this TUI would only open it again: it stays,
    // says so and keeps trying to reach the daemon.
    let (mut app, _) = loaded(&api, 140, 24).await;
    app.update(Msg::Subscribed(Err(ApiError::Incompatible {
        peer: peer.clone(),
    })));
    let effects = app.update(Msg::ReloadChecked(None));
    assert_eq!(app.exit, None);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::ScheduleReconnect { .. })),
        "{effects:?}"
    );
    let notice = app
        .notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default();
    assert!(notice.contains("does not answer"), "{notice}");
    // The next loss to that daemon asks no more.
    app.update(Msg::Reconnect);
    let effects = app.update(Msg::Subscribed(Err(ApiError::Incompatible { peer })));
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::CheckReload)),
        "{effects:?}"
    );
    Ok(())
}

/// A daemon that ends the connection while the reload `U` asked for still
/// reads the installed version is not forgotten: when the TUI stays, it
/// reconnects.
#[tokio::test]
async fn a_loss_during_a_reload_check_still_reconnects() -> TestResult {
    let own = devlanes_version::VERSION;
    let mut app = reported(status("99.0.0", None)).await;
    let effects = app.update(shifted('U'));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::CheckReload)),
        "{effects:?}"
    );
    let peer = hello(devlanes_proto::PROTOCOL_VERSION + 1, "99.0.0");
    let effects = app.update(Msg::Subscribed(Err(ApiError::Incompatible { peer })));
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::CheckReload | Effect::ScheduleReconnect { .. })),
        "one check at a time: {effects:?}"
    );
    let effects = app.update(Msg::ReloadChecked(Some(own.into())));
    assert_eq!(app.exit, None);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::ScheduleReconnect { .. })),
        "{effects:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_daemon_on_an_older_protocol_says_what_fixes_it() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    let peer = hello(devlanes_proto::PROTOCOL_VERSION - 1, "0.0.1");
    let effects = app.update(Msg::EventsEnded(Some(ApiError::Incompatible {
        peer: peer.clone(),
    })));
    assert_eq!(
        app.daemon_state,
        DaemonState::Outdated {
            version: "0.0.1".into()
        }
    );
    // It keeps retrying, in case someone else restarts the daemon.
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::ScheduleReconnect { .. })),
        "{effects:?}"
    );
    let screen = draw(&app)?.backend().to_string();
    assert!(
        screen.contains("daemon 0.0.1 speaks an older protocol · U update · R restart"),
        "{screen}"
    );
    let footer = footer(&app)?;
    assert!(
        footer.starts_with(" U update   R restart daemon"),
        "{footer}"
    );
    // The retry finds the same daemon: nothing changes.
    app.update(Msg::Reconnect);
    app.update(Msg::Subscribed(Err(ApiError::Incompatible { peer })));
    assert!(matches!(app.daemon_state, DaemonState::Outdated { .. }));

    // R restarts it from the installed binary, without asking.
    let effects = app.update(shifted('R'));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::RestartDaemon)),
        "{effects:?}"
    );
    assert!(matches!(
        app.daemon_state,
        DaemonState::Stopping { restart: true, .. }
    ));
    Ok(())
}

#[tokio::test]
async fn u_on_an_outdated_daemon_closes_to_update() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    let peer = hello(devlanes_proto::PROTOCOL_VERSION - 1, "0.0.1");
    app.update(Msg::EventsEnded(Some(ApiError::Incompatible { peer })));
    let effects = app.update(shifted('U'));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Exit(ExitReason::Update))),
        "{effects:?}"
    );
    // A plain loss afterwards is a daemon gone, not an outdated one.
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 140, 24).await;
    let peer = hello(devlanes_proto::PROTOCOL_VERSION - 1, "0.0.1");
    app.update(Msg::EventsEnded(Some(ApiError::Incompatible { peer })));
    app.update(Msg::Subscribed(Err(ApiError::Disconnected(
        "refused".into(),
    ))));
    assert_eq!(app.daemon_state, DaemonState::Unreachable);
    Ok(())
}

#[tokio::test]
async fn a_reopened_tui_opens_on_the_lane_and_view_it_had() -> TestResult {
    let api = FakeApi::with_lanes(mixed_lanes());
    let mut app = App::new(140, 24);
    app.prefer_path("/work/acme/main".into());
    let mut effects = Vec::new();
    app.reopen(
        &Reopen {
            lane: Some("shop@0".into()),
            view: Some("daemon".into()),
        },
        &mut effects,
    );
    assert_eq!(app.view, View::Daemon);
    assert!(
        effects.iter().any(|e| matches!(e, Effect::FetchDaemon)),
        "{effects:?}"
    );
    app.update(Msg::Status(api.status().await));
    assert_eq!(
        app.selected.as_ref().map(|s| s.lane.as_str()),
        Some("shop@0")
    );

    // A lane that is gone falls back to the working directory's.
    let mut app = App::new(140, 24);
    app.prefer_path("/work/acme/main".into());
    app.reopen(
        &Reopen {
            lane: Some("gone@9".into()),
            view: Some("sideways".into()),
        },
        &mut Vec::new(),
    );
    assert_eq!(app.view, View::Project);
    app.update(Msg::Status(api.status().await));
    assert_eq!(
        app.selected.as_ref().map(|s| s.lane.as_str()),
        Some("acme@0")
    );
    Ok(())
}

#[tokio::test]
async fn drive_returns_the_exit_reason_and_skips_nothing_else() -> TestResult {
    use crossterm::event::Event as TermEvent;
    use std::sync::Arc;

    let mut fake = FakeApi::with_lanes(mixed_lanes());
    fake.installed = Some("99.0.0".into());
    let api = Arc::new(fake);
    let (keys_tx, keys_rx) = tokio::sync::mpsc::channel::<std::io::Result<TermEvent>>(8);
    let events = Box::pin(futures_util::stream::unfold(keys_rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    }));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 24))?;
    let driver: Arc<dyn devlanes_tui::DaemonApi> = api.clone();
    let mut options = super::hermetic_options();
    options.reopen = Some(Reopen {
        lane: Some("shop@0".into()),
        view: Some("boxes".into()),
    });
    let script = {
        let api = api.clone();
        async move {
            super::wait_for_calls(&api, |calls| calls.iter().any(|c| c == "status")).await;
            // A daemon newer than this TUI answers the next request.
            let peer = hello(devlanes_proto::PROTOCOL_VERSION + 1, "99.0.0");
            if let Ok(Some(events)) = api.events.lock().map(|e| e.clone()) {
                events.send(Err(ApiError::Incompatible { peer })).await.ok();
            }
            keys_tx
        }
    };
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
    assert_eq!(
        result?,
        Some((
            ExitReason::Reload,
            Reopen {
                lane: Some("shop@0".into()),
                view: Some("boxes".into()),
            }
        ))
    );
    Ok(())
}
