//! The daemon going away: its states, the unavailable screen that replaces
//! the body in every view, the keys it takes and the header logo.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use devlanes_client::restart::{Progress, RowState};
use devlanes_proto::{DaemonStopping, StoppingState, StoppingStep, events};
use devlanes_tui::{
    DaemonApi,
    app::{App, DaemonState, Dialog, Effect, Msg, View},
    logfile::{LogChunk, LogPosition},
    steps::{LogoState, logo_style},
};
use serde_json::json;

use crate::daemon::shifted;
use crate::errors::{assert_clean, key, render, row, screen};
use crate::fake::{FakeApi, daemon_status, mixed_lanes};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A loaded, subscribed app on `view` with a daemon log tail and its clock
/// pinned, so step times draw the same on every run.
async fn app_on(width: u16, height: u16, view: View) -> App {
    let api = FakeApi::with_lanes(mixed_lanes());
    let mut app = App::new(width, height);
    app.clock = Some(Instant::now());
    app.update(Msg::Status(api.status().await));
    app.update(Msg::Subscribed(Ok(())));
    app.update(Msg::DaemonInfo {
        status: api.daemon_status().await,
        doctor: api.doctor().await,
    });
    app.update(Msg::DaemonLog(Ok(LogChunk {
        lines: vec![
            "2026-10-21T09:12:01.003Z  INFO devlanes_daemon: listening socket=/home/acme/.devlanes/run/daemon.sock".into(),
            "2026-10-21T09:14:02.990Z  INFO devlanes_daemon::engine: shutdown step started step=units count=3".into(),
            "2026-10-21T09:14:04.210Z  INFO devlanes_daemon::lifecycle: unit took long to stop unit=acme@0:api elapsed_ms=2100".into(),
        ],
        position: LogPosition {
            offset: 300,
            file: (1, 2),
        },
        reset: true,
    })));
    if view != View::Project {
        let letter = match view {
            View::Monitor => 'M',
            View::Daemon => 'D',
            _ => 'B',
        };
        app.update(shifted(letter));
    }
    assert_eq!(app.daemon_state, DaemonState::Online);
    app
}

/// Moves the pinned clock on by `ms`.
fn later(app: &mut App, ms: u64) {
    app.clock = app.clock.map(|now| now + Duration::from_millis(ms));
}

/// A `daemon.stopping` report of the `units` step.
fn units(restart: bool, state: StoppingState, left: Option<u64>) -> DaemonStopping {
    DaemonStopping {
        reason: "explicit".into(),
        restart,
        step: StoppingStep::Units,
        state,
        total: Some(26),
        left,
        lanes: Some(1),
        elapsed_ms: (state == StoppingState::Done).then_some(5800),
    }
}

/// `step` as the subscription delivers it.
fn stopping_event(step: &DaemonStopping) -> Result<Msg, serde_json::Error> {
    Ok(Msg::Event(devlanes_proto::Event {
        seq: 9,
        event: events::DAEMON_STOPPING.into(),
        data: serde_json::to_value(step)?,
    }))
}

/// The step rows as `(text, state)`.
fn rows(app: &App) -> Vec<(String, RowState)> {
    app.daemon_state
        .steps()
        .map(|steps| {
            steps
                .rows()
                .iter()
                .map(|row| (row.text.clone(), row.state))
                .collect()
        })
        .unwrap_or_default()
}

/// How many of `effects` match `pick`.
fn count(effects: &[Effect], pick: impl Fn(&Effect) -> bool) -> usize {
    effects.iter().filter(|effect| pick(effect)).count()
}

/// R on the daemon page, then y: the screen shows the restart's steps as
/// this TUI's own restart reports them, the connection closing moves on to
/// starting, and the finished restart subscribes once, after which the page
/// draws again as it was.
#[tokio::test]
async fn an_own_restart_shows_its_steps_until_the_new_daemon_answers() -> TestResult {
    // Wide enough for the whole header message.
    let mut app = app_on(120, 40, View::Daemon).await;
    app.update(shifted('R'));
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(
        matches!(effects.last(), Some(Effect::RestartDaemon)),
        "{effects:?}"
    );
    assert_eq!(
        count(&effects, |e| matches!(e, Effect::ReadDaemonLog { .. })),
        1,
        "the log is read at once: {effects:?}"
    );
    assert!(matches!(
        app.daemon_state,
        DaemonState::Stopping { restart: true, .. }
    ));
    assert!(app.unavailable());
    app.update(Msg::DaemonStep(Progress::Stopping { processes: 3 }));
    later(&mut app, 400);
    let buffer = render(&app)?;
    assert!(
        row(&buffer, 0).contains("daemon restarting · stopping the daemon and 3 processes"),
        "{}",
        row(&buffer, 0)
    );
    assert!(screen(&buffer).contains("devlanes is restarting"));
    // The same step from this TUI's request and from the subscription.
    let started = units(true, StoppingState::Started, None);
    app.update(Msg::DaemonStep(Progress::Step(started.clone())));
    app.update(stopping_event(&started)?);
    app.update(Msg::DaemonStep(Progress::Step(units(
        true,
        StoppingState::Progress,
        Some(4),
    ))));
    assert_eq!(
        rows(&app),
        [("stopping 26 processes in 1 lane".into(), RowState::Running)]
    );
    app.update(Msg::DaemonStep(Progress::Step(units(
        true,
        StoppingState::Done,
        None,
    ))));
    // The old daemon closes the subscription: expected, so no error.
    let effects = app.update(Msg::EventsEnded(None));
    assert_eq!(
        count(&effects, |e| matches!(e, Effect::ScheduleReconnect { .. })),
        1
    );
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    assert!(matches!(
        app.daemon_state,
        DaemonState::Starting { restart: true, .. }
    ));
    app.update(Msg::DaemonStep(Progress::Stopped));
    app.update(Msg::DaemonStep(Progress::Starting(
        "waiting for the old daemon".into(),
    )));
    later(&mut app, 300);
    app.update(Msg::DaemonStep(Progress::Started {
        version: "0.0.1".into(),
        pid: Some(4343),
    }));
    assert_eq!(
        rows(&app),
        [
            ("stopped 26 processes in 1 lane".into(), RowState::Done),
            ("started devlanes 0.0.1".into(), RowState::Done),
        ]
    );
    let mut status = daemon_status();
    status["resuming"] = json!(["acme@0"]);
    let effects = app.update(Msg::DaemonRestarted(Ok(status)));
    assert_eq!(count(&effects, |e| matches!(e, Effect::Subscribe)), 1);
    // The retry that was waiting finds nothing left to do.
    assert!(app.update(Msg::Reconnect).is_empty());
    let effects = app.update(Msg::Subscribed(Ok(())));
    assert_eq!(app.daemon_state, DaemonState::Online);
    assert!(effects.iter().any(|e| matches!(e, Effect::Refresh)));
    assert!(effects.iter().any(|e| matches!(e, Effect::FetchDaemon)));
    assert_eq!(app.view, View::Daemon, "nothing switched views");
    assert_eq!(
        app.notice.as_ref().map(|n| n.text.as_str()),
        Some("daemon restarted, starting 1 lane again")
    );
    let text = screen(&render(&app)?);
    assert!(!text.contains("devlanes is restarting"), "{text}");
    assert!(text.contains("daemon.log"), "{text}");
    Ok(())
}

/// A restart someone else ran: `daemon.stopping` events say so on every
/// view, the connection closing means the new daemon is on its way, and the
/// project view draws again once the subscription opens on it.
#[tokio::test]
async fn a_restart_someone_else_ran_follows_its_events() -> TestResult {
    let mut app = app_on(120, 40, View::Project).await;
    app.update(key(KeyCode::Char('?')));
    assert!(matches!(app.dialog, Some(Dialog::Help)));
    app.update(stopping_event(&units(true, StoppingState::Started, None))?);
    assert!(app.dialog.is_none(), "dialogs close when the daemon goes");
    assert!(matches!(
        app.daemon_state,
        DaemonState::Stopping { restart: true, .. }
    ));
    app.update(stopping_event(&units(
        true,
        StoppingState::Progress,
        Some(4),
    ))?);
    let buffer = render(&app)?;
    assert!(
        row(&buffer, 0).contains("daemon restarting · stopping 26 processes in 1 lane"),
        "{}",
        row(&buffer, 0)
    );
    assert!(screen(&buffer).contains("4 left"));
    app.update(Msg::EventsEnded(None));
    assert!(matches!(
        app.daemon_state,
        DaemonState::Starting { restart: true, .. }
    ));
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    // Nothing answers the first retry; the second finds the new daemon.
    let effects = app.update(Msg::Reconnect);
    assert!(matches!(effects.as_slice(), [Effect::Subscribe]));
    app.update(Msg::Subscribed(Err(devlanes_tui::ApiError::Disconnected(
        "gone".into(),
    ))));
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    app.update(Msg::Reconnect);
    app.update(Msg::Subscribed(Ok(())));
    assert_eq!(app.daemon_state, DaemonState::Online);
    assert_eq!(app.view, View::Project);
    assert!(screen(&render(&app)?).contains("┌ @0 · "));
    Ok(())
}

/// A restart someone else ran whose new daemon never answers ends as not
/// answering after a minute, where s starts one.
#[tokio::test]
async fn a_restart_that_never_answers_gives_up_waiting() -> TestResult {
    let mut app = app_on(120, 40, View::Project).await;
    app.update(stopping_event(&units(true, StoppingState::Started, None))?);
    app.update(Msg::EventsEnded(None));
    assert!(matches!(app.daemon_state, DaemonState::Starting { .. }));
    later(&mut app, 30_000);
    app.update(Msg::Tick);
    assert!(matches!(app.daemon_state, DaemonState::Starting { .. }));
    later(&mut app, 31_000);
    app.update(Msg::Tick);
    assert_eq!(app.daemon_state, DaemonState::Unreachable);
    let effects = app.update(key(KeyCode::Char('s')));
    assert!(
        effects.iter().any(|e| matches!(e, Effect::StartDaemon)),
        "{effects:?}"
    );
    Ok(())
}

/// A stop someone else asked for ends as stopped once the connection
/// closes; s starts the daemon again.
#[tokio::test]
async fn a_stop_someone_else_asked_for_ends_stopped() -> TestResult {
    let mut app = app_on(80, 24, View::Monitor).await;
    app.update(stopping_event(&units(false, StoppingState::Done, None))?);
    assert!(matches!(
        app.daemon_state,
        DaemonState::Stopping { restart: false, .. }
    ));
    app.update(Msg::EventsEnded(None));
    assert!(matches!(app.daemon_state, DaemonState::Stopped { .. }));
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    let effects = app.update(key(KeyCode::Char('s')));
    assert!(matches!(effects.as_slice(), [Effect::StartDaemon]));
    assert!(matches!(
        app.daemon_state,
        DaemonState::Starting { restart: false, .. }
    ));
    let effects = app.update(Msg::DaemonStarted(Ok(daemon_status())));
    assert_eq!(count(&effects, |e| matches!(e, Effect::Subscribe)), 1);
    app.update(Msg::Subscribed(Ok(())));
    assert_eq!(app.daemon_state, DaemonState::Online);
    assert_eq!(app.view, View::Monitor);
    Ok(())
}

/// A loss nothing announced shows nothing until a tick passed: a blip that
/// heals first leaves the screen as it was. Past the tick the daemon is
/// not answering, and s starts one.
#[tokio::test]
async fn an_unannounced_loss_waits_one_tick() -> TestResult {
    let mut app = app_on(80, 24, View::Project).await;
    app.update(Msg::EventsEnded(None));
    assert_eq!(app.errors.len(), 1, "an unannounced loss is an error");
    assert_eq!(app.daemon_state, DaemonState::Online);
    let text = screen(&render(&app)?);
    assert!(
        text.contains("┌ @0 · ") && !text.contains("not answering"),
        "{text}"
    );
    app.update(Msg::Reconnect);
    app.update(Msg::Subscribed(Ok(())));
    app.update(Msg::Tick);
    assert_eq!(app.daemon_state, DaemonState::Online);
    assert!(!app.unavailable());

    app.update(Msg::EventsEnded(None));
    let effects = app.update(Msg::Tick);
    assert_eq!(app.daemon_state, DaemonState::Unreachable);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::ReadDaemonLog { .. }))
    );
    let text = screen(&render(&app)?);
    assert!(text.contains("the daemon is not answering"), "{text}");
    let effects = app.update(key(KeyCode::Char('s')));
    assert!(matches!(effects.as_slice(), [Effect::StartDaemon]));
    Ok(())
}

/// S then y stops the daemon; the stop's own connection closing is no
/// error, the screen says it is stopped, and s starts it again.
#[tokio::test]
async fn an_own_stop_then_start() -> TestResult {
    let mut app = app_on(120, 40, View::Daemon).await;
    app.update(shifted('S'));
    let effects = app.update(key(KeyCode::Char('y')));
    assert!(matches!(effects.last(), Some(Effect::StopDaemon)));
    assert!(matches!(
        app.daemon_state,
        DaemonState::Stopping { restart: false, .. }
    ));
    app.update(Msg::DaemonStep(Progress::Stopping { processes: 3 }));
    app.update(Msg::EventsEnded(None));
    app.update(Msg::DaemonStep(Progress::Stopped));
    app.update(Msg::DaemonStopped(Ok(json!(null))));
    assert!(matches!(app.daemon_state, DaemonState::Stopped { .. }));
    assert_eq!(rows(&app), [("stopped the daemon".into(), RowState::Done)]);
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    // The retries go on; a failed one is no error either.
    app.update(Msg::Reconnect);
    app.update(Msg::Subscribed(Err(devlanes_tui::ApiError::Disconnected(
        "gone".into(),
    ))));
    app.update(Msg::Tick);
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    let buffer = render(&app)?;
    assert!(
        row(&buffer, 0).contains("daemon stopped · s starts it"),
        "{}",
        row(&buffer, 0)
    );
    assert!(
        row(&buffer, 39).contains("s start daemon"),
        "{}",
        row(&buffer, 39)
    );
    let effects = app.update(key(KeyCode::Char('s')));
    assert!(matches!(effects.as_slice(), [Effect::StartDaemon]));
    // s again while it starts does not start a second one.
    assert!(app.update(key(KeyCode::Char('s'))).is_empty());
    let effects = app.update(Msg::DaemonStarted(Ok(daemon_status())));
    assert_eq!(count(&effects, |e| matches!(e, Effect::Subscribe)), 1);
    app.update(Msg::Subscribed(Ok(())));
    assert_eq!(app.daemon_state, DaemonState::Online);
    assert_eq!(
        app.notice.as_ref().map(|n| n.text.as_str()),
        Some("daemon started")
    );
    Ok(())
}

/// A restart refused before anything stopped leaves the daemon online with
/// the error; one that failed after the old daemon went ends stopped with
/// the reason on the failed row.
#[tokio::test]
async fn a_failed_restart_says_where_it_stopped() -> TestResult {
    let config = devlanes_tui::ApiError::Rpc(devlanes_proto::RpcError::new(
        "config_error",
        json!({"message": "unknown key tui.colour"}),
        1,
    ));
    let mut app = app_on(80, 24, View::Daemon).await;
    app.update(shifted('R'));
    app.update(key(KeyCode::Char('y')));
    app.update(Msg::DaemonRestarted(Err(config)));
    assert_eq!(app.daemon_state, DaemonState::Online);
    assert_eq!(app.errors.len(), 1);
    assert_eq!(app.errors[0].what, "restart the daemon");

    app.update(shifted('R'));
    app.update(key(KeyCode::Char('y')));
    app.update(Msg::DaemonStep(Progress::Stopping { processes: 3 }));
    app.update(Msg::EventsEnded(None));
    app.update(Msg::DaemonStep(Progress::Starting("state database".into())));
    app.update(Msg::DaemonRestarted(Err(
        devlanes_tui::ApiError::Disconnected("the new daemon exited".into()),
    )));
    assert!(matches!(app.daemon_state, DaemonState::Stopped { .. }));
    assert_eq!(
        rows(&app).last().map(|(_, state)| *state),
        Some(RowState::Failed)
    );
    assert_eq!(app.errors.len(), 2, "{:?}", app.errors);
    Ok(())
}

/// Only q, Ctrl+C, s and the log's keys work: views, lanes, dialogs and
/// help wait for the daemon, and q quits without asking about the lanes on
/// screen, which are stale.
#[tokio::test]
async fn the_unavailable_screen_takes_only_its_keys() -> TestResult {
    let mut app = app_on(80, 24, View::Project).await;
    app.update(stopping_event(&units(false, StoppingState::Started, None))?);
    let selected = app.selected.clone();
    for msg in [
        key(KeyCode::Char('x')),
        key(KeyCode::Char('r')),
        key(KeyCode::Char('s')),
        key(KeyCode::Char('?')),
        key(KeyCode::Char('e')),
        key(KeyCode::Enter),
        key(KeyCode::Esc),
        key(KeyCode::Tab),
        key(KeyCode::Char('2')),
        shifted('D'),
        shifted('M'),
        shifted('S'),
        shifted('R'),
    ] {
        let effects = app.update(msg);
        assert!(effects.is_empty(), "{effects:?}");
    }
    assert!(app.dialog.is_none());
    assert_eq!(app.view, View::Project);
    assert_eq!(app.selected, selected);
    // The log scrolls, pauses and follows again; / takes typing, q included.
    render(&app)?;
    app.update(key(KeyCode::Char('w')));
    app.update(key(KeyCode::Char('w')));
    app.update(key(KeyCode::Char('w')));
    app.update(key(KeyCode::Char('/')));
    assert!(app.daemon.filter_editing);
    for c in "quit".chars() {
        assert!(app.update(key(KeyCode::Char(c))).is_empty());
    }
    assert_eq!(app.daemon.filters.text, "quit");
    let buffer = render(&app)?;
    assert!(
        row(&buffer, 23).contains("type to filter"),
        "{}",
        row(&buffer, 23)
    );
    app.update(key(KeyCode::Esc));
    assert!(app.daemon.filters.text.is_empty());
    app.update(key(KeyCode::Char('F')));
    assert!(app.daemon.paused);
    app.update(key(KeyCode::Char('G')));
    assert!(!app.daemon.paused);
    let footer = row(&render(&app)?, 23);
    assert!(footer.contains("j k scroll"), "{footer}");
    assert!(!footer.contains("all keys"), "{footer}");
    // Lanes of the project run, yet q and Ctrl+C quit without asking.
    assert!(matches!(
        app.update(key(KeyCode::Char('q'))).as_slice(),
        [Effect::Quit]
    ));
    assert!(matches!(
        app.update(Msg::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )))
        .as_slice(),
        [Effect::Quit]
    ));
    assert!(app.dialog.is_none());
    Ok(())
}

/// The brand word in the header takes the daemon's state: its own colors
/// online, the logo's grey while it changes, its red while it is down.
#[tokio::test]
async fn the_header_logo_follows_the_daemon_state() -> TestResult {
    let mut app = app_on(80, 24, View::Project).await;
    let theme = app.theme;
    let brand =
        |app: &App| -> Result<_, Box<dyn std::error::Error>> { Ok(render(app)?[(1, 0)].fg) };
    assert_eq!(brand(&app)?, theme.brand(0, 8).fg.unwrap_or_default());
    app.update(stopping_event(&units(true, StoppingState::Started, None))?);
    assert_eq!(app.daemon_state.logo(), LogoState::Busy);
    assert_eq!(
        brand(&app)?,
        logo_style(&theme, LogoState::Busy, 0)
            .fg
            .unwrap_or_default()
    );
    app.update(Msg::EventsEnded(None));
    assert_eq!(app.daemon_state.logo(), LogoState::Busy, "starting");
    app.daemon_state = DaemonState::Unreachable;
    assert_eq!(app.daemon_state.logo(), LogoState::Down);
    assert_eq!(
        brand(&app)?,
        logo_style(&theme, LogoState::Down, 0)
            .fg
            .unwrap_or_default()
    );
    app.daemon_state = DaemonState::Stopped {
        steps: Default::default(),
    };
    assert_eq!(app.daemon_state.logo(), LogoState::Down);
    Ok(())
}

/// Brings `app` into `state` the way it gets there: a restart this TUI
/// asked for, part way through stopping or starting; a stop; an
/// unannounced loss.
fn reach(app: &mut App, state: &str) -> Result<(), Box<dyn std::error::Error>> {
    match state {
        "stopping" => {
            app.update(stopping_event(&units(true, StoppingState::Started, None))?);
            later(app, 3400);
            app.update(stopping_event(&units(
                true,
                StoppingState::Progress,
                Some(4),
            ))?);
        }
        "starting" => {
            app.update(stopping_event(&units(true, StoppingState::Started, None))?);
            app.update(stopping_event(&units(true, StoppingState::Done, None))?);
            app.update(Msg::EventsEnded(None));
            app.update(Msg::DaemonStep(Progress::Starting("state database".into())));
            later(app, 1200);
        }
        "stopped" => {
            app.update(stopping_event(&units(false, StoppingState::Started, None))?);
            app.update(stopping_event(&units(false, StoppingState::Done, None))?);
            app.update(Msg::EventsEnded(None));
        }
        _ => {
            app.update(Msg::EventsEnded(None));
            app.update(Msg::Tick);
        }
    }
    Ok(())
}

#[tokio::test]
async fn unavailable_screens() -> TestResult {
    for state in ["stopping", "starting", "stopped", "unreachable"] {
        for (width, height) in [(80, 24), (120, 40)] {
            let mut app = app_on(width, height, View::Project).await;
            reach(&mut app, state)?;
            assert!(app.unavailable(), "{state}");
            let buffer = render(&app)?;
            assert_clean(&buffer, state);
            let name = format!("unavailable_{state}_{width}x{height}");
            assert_screen!(name.as_str(), screen(&buffer));
        }
    }
    Ok(())
}

/// The fake's stop and restart report their steps through the loop before
/// their result, so the screen shows them as they come.
#[tokio::test]
async fn drive_delivers_the_steps_before_the_result() -> TestResult {
    use std::sync::Arc;
    let api = Arc::new(FakeApi::with_lanes(mixed_lanes()));
    let steps = crate::daemon::presses(vec![shifted('D'), shifted('S'), key(KeyCode::Char('y'))]);
    let terminal = crate::daemon::run_keys(
        Arc::clone(&api),
        steps,
        &["the daemon is stopped", "stopped the daemon"],
        |calls| calls.iter().any(|c| c == "daemon.shutdown"),
    )
    .await?;
    let text = screen(terminal.buffer());
    assert!(text.contains("s start daemon"), "{text}");
    Ok(())
}
