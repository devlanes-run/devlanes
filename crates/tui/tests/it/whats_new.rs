//! Release highlights: the notes the binary hands over open by themselves
//! once, after the welcome and never stacked on it, scroll when long, open
//! again from the Esc menu and open the release page on `o`.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use crossterm::event::{
    Event as TermEvent, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
};
use devlanes_tui::app::{App, Dialog, Msg};
use devlanes_tui::welcome::KEY as WELCOME;
use devlanes_update::{
    InstallMethod,
    notes::{Entry, Highlight, Level, Release, WhatsNew},
};
use futures_util::stream;
use ratatui::Terminal;
use tokio::sync::mpsc;

use super::{SharedBackend, TestResult, draw, hermetic_options, key, loaded};
use crate::fake::{FakeApi, mixed_lanes};

/// Notes since 0.8.0 for `count` releases, each with a minor entry and a
/// patch entry that is a highlight.
fn notes(count: usize) -> WhatsNew {
    WhatsNew {
        since: Some("0.8.0".into()),
        releases: (0..count)
            .map(|index| Release {
                version: format!("0.9.{}", count - 1 - index),
                entries: vec![
                    Entry {
                        level: Level::Minor,
                        line: "Removed `--acme`.".into(),
                        body: String::new(),
                        highlight: None,
                    },
                    Entry {
                        level: Level::Patch,
                        line: format!("Fixed `devlanes logs` in release {index}."),
                        body: String::new(),
                        highlight: Some(Highlight {
                            title: format!("Logs in release {index}"),
                            body: "Run `devlanes logs` to follow a unit.".into(),
                        }),
                    },
                ],
            })
            .collect(),
    }
}

/// A loaded app given `notes`, opening them by itself when `show`.
async fn given(notes: WhatsNew, show: bool) -> App {
    let api = FakeApi::with_lanes(mixed_lanes());
    let (mut app, _) = loaded(&api, 100, 30).await;
    app.whats_new = Some(notes);
    app.whats_new_pending = show;
    app
}

/// The notices of a home whose welcome was closed when `welcomed`.
fn notices(welcomed: bool) -> Msg {
    Msg::Notices {
        method: InstallMethod::Npm,
        dismissed: if welcomed {
            vec![WELCOME.into()]
        } else {
            Vec::new()
        },
    }
}

#[tokio::test]
async fn it_follows_the_welcome_once() -> TestResult {
    let mut app = given(notes(2), true).await;
    app.update(notices(false));
    assert!(matches!(app.dialog, Some(Dialog::Welcome(_))));
    // Closing the welcome brings it; they never show together.
    app.update(key(KeyCode::Enter));
    assert!(matches!(app.dialog, Some(Dialog::WhatsNew(_))));
    let screen = draw(&app)?.backend().to_string();
    for wanted in [
        "Release highlights since 0.8.0",
        "breaking Removed --acme.",
        "Logs in release 0",
        "Run devlanes logs to follow a unit.",
    ] {
        assert!(screen.contains(wanted), "{wanted}\n{screen}");
    }
    app.update(key(KeyCode::Esc));
    assert!(app.dialog.is_none());
    // Nothing brings it back by itself.
    app.update(Msg::Status(Ok(mixed_lanes())));
    app.update(notices(true));
    assert!(app.dialog.is_none());
    Ok(())
}

#[tokio::test]
async fn it_waits_for_the_notices_and_other_dialogs() -> TestResult {
    let mut app = given(notes(1), true).await;
    assert!(app.dialog.is_none(), "not before the notices load");
    app.update(key(KeyCode::Char('?')));
    app.update(notices(true));
    assert!(matches!(app.dialog, Some(Dialog::Help)));
    app.update(key(KeyCode::Esc));
    // The next status brings it.
    app.update(Msg::Status(Ok(mixed_lanes())));
    assert!(matches!(app.dialog, Some(Dialog::WhatsNew(_))));
    let screen = draw(&app)?.backend().to_string();
    assert!(screen.contains("Release highlights in 0.9.0"), "{screen}");
    Ok(())
}

#[tokio::test]
async fn without_new_notes_it_opens_only_from_the_menu() -> TestResult {
    let mut app = given(notes(1), false).await;
    app.update(notices(true));
    assert!(app.dialog.is_none());
    app.update(key(KeyCode::Esc));
    assert!(matches!(app.dialog, Some(Dialog::Menu(_))));
    app.update(key(KeyCode::Char('l')));
    assert!(matches!(app.dialog, Some(Dialog::WhatsNew(_))));
    let screen = draw(&app)?.backend().to_string();
    assert!(screen.contains("Release highlights in 0.9.0"), "{screen}");
    app.update(key(KeyCode::Enter));
    assert!(app.dialog.is_none());

    // A build without notes says so.
    app.whats_new = None;
    app.update(key(KeyCode::Esc));
    app.update(key(KeyCode::Char('l')));
    let screen = draw(&app)?.backend().to_string();
    assert!(
        screen.contains("This build carries no release highlights."),
        "{screen}"
    );
    Ok(())
}

/// `o` asks for the release notes in the browser and the dialog stays open.
#[tokio::test]
async fn o_opens_the_release_notes_and_keeps_the_dialog() -> TestResult {
    use devlanes_tui::app::Effect;
    let mut app = given(notes(1), true).await;
    app.update(notices(true));
    let effects = app.update(key(KeyCode::Char('o')));
    let urls: Vec<&str> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::OpenUrl(url) => Some(url.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        urls,
        [format!(
            "{}/releases/tag/v0.9.0",
            devlanes_version::REPOSITORY
        )]
    );
    assert!(matches!(app.dialog, Some(Dialog::WhatsNew(_))));

    // Several releases point at the list of releases.
    let mut app = given(notes(2), true).await;
    app.update(notices(true));
    let effects = app.update(key(KeyCode::Char('o')));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::OpenUrl(url) if url.ends_with("/releases")
    )));
    Ok(())
}

#[tokio::test]
async fn long_notes_scroll_with_keys_and_the_wheel() -> TestResult {
    let mut app = given(notes(20), true).await;
    app.update(notices(true));
    let first = |app: &App| -> Result<String, Box<dyn std::error::Error>> {
        let screen = draw(app)?.backend().to_string();
        Ok(screen
            .lines()
            .find(|line| line.contains("lines "))
            .unwrap_or_default()
            .to_owned())
    };
    assert!(first(&app)?.contains("lines 1-24 of"), "{}", first(&app)?);
    app.update(key(KeyCode::Char('j')));
    assert!(first(&app)?.contains("lines 2-25 of"), "{}", first(&app)?);
    app.update(key(KeyCode::PageDown));
    assert!(first(&app)?.contains("lines 12-35 of"), "{}", first(&app)?);
    app.update(key(KeyCode::PageUp));
    app.update(Msg::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 50,
        row: 15,
        modifiers: KeyModifiers::NONE,
    }));
    assert!(first(&app)?.contains("lines 5-28 of"), "{}", first(&app)?);
    // The dialog keeps every key while open.
    app.update(key(KeyCode::Char('k')));
    assert!(matches!(app.dialog, Some(Dialog::WhatsNew(_))));
    app.update(key(KeyCode::Esc));
    assert!(app.dialog.is_none());
    Ok(())
}

#[tokio::test]
async fn the_options_open_it_after_the_welcome() -> TestResult {
    // No lanes run, so q quits without asking.
    let api = Arc::new(FakeApi::with_lanes(Vec::new()));
    let (keys_tx, keys_rx) = mpsc::channel::<std::io::Result<TermEvent>>(8);
    let events = Box::pin(stream::unfold(keys_rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    }));
    let backend = SharedBackend::new(120, 40);
    let mut terminal = Terminal::new(backend.clone())?;
    let driver: Arc<dyn devlanes_tui::DaemonApi> = api.clone();
    let script = {
        let backend = backend.clone();
        async move {
            backend.wait_for("Release highlights in 0.9.0").await;
            backend.wait_for("breaking Removed --acme.").await;
            let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
            keys_tx.send(Ok(TermEvent::Key(quit))).await.ok();
            keys_tx
        }
    };
    // The binary records the notes as seen in the claim, so only a TUI that
    // runs calls it, once.
    let claims = Arc::new(AtomicUsize::new(0));
    let mut options = hermetic_options();
    options.whats_new = Some({
        let claims = claims.clone();
        Arc::new(move || {
            claims.fetch_add(1, Ordering::SeqCst);
            (Some(notes(1)), true)
        })
    });
    assert_eq!(
        claims.load(Ordering::SeqCst),
        0,
        "options alone claim nothing"
    );
    let run = devlanes_tui::drive(
        &mut terminal,
        driver,
        &options,
        events,
        Box::new(std::io::sink()),
    );
    let (result, _keys) =
        tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(run, script) }).await?;
    assert_eq!(result?, None);
    assert_eq!(claims.load(Ordering::SeqCst), 1);
    Ok(())
}
