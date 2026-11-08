//! What the TUI writes to its terminal besides the screen: the tab title,
//! notifications while unfocused, links and synchronized frames, and that it
//! writes none of them when they are off.

use std::{sync::Arc, time::Duration};

use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyModifiers};
use devlanes_proto::{Event, events, features};
use devlanes_term::{Links, Notifications, NotifyStyle, POP_TITLE, PUSH_TITLE};
use devlanes_tui::{
    DaemonApi, Options,
    app::{App, Effect, Msg},
    drive,
};
use futures_util::stream;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;
use tokio::sync::mpsc;

use crate::{
    fake::{FakeApi, lane, mixed_lanes, unit},
    progress::Captured,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// `lane` as a `lane.changed` event.
fn changed(lane: devlanes_proto::LaneSummary) -> Msg {
    Msg::Event(Event {
        seq: 2,
        event: events::LANE_CHANGED.into(),
        data: json!({ "lane": lane }),
    })
}

/// acme@0 in `state` with `api` and `web` as given.
fn main_lane(state: &str, api: &str, web: &str) -> devlanes_proto::LaneSummary {
    lane(
        "acme",
        Some(0),
        "main",
        state,
        vec![unit("acme@0", "api", api), unit("acme@0", "web", web)],
    )
}

/// An app that loaded the fake's lanes, showing acme@0.
async fn loaded() -> Result<App, Box<dyn std::error::Error>> {
    let api = FakeApi::with_lanes(mixed_lanes());
    let mut app = App::new(100, 24);
    app.update(Msg::Status(api.status().await));
    let shown = app.selected_lane().map(|lane| lane.reference.clone());
    assert_eq!(shown.as_deref(), Some("acme@0"));
    Ok(app)
}

#[tokio::test]
async fn the_tab_title_says_which_lane_and_how_it_stands() -> TestResult {
    assert_eq!(App::new(100, 24).tab_title(), "devlanes");
    let mut app = loaded().await?;
    // The fake's worker failed.
    assert_eq!(app.tab_title(), "devlanes · acme@0 · 1 failed");
    app.update(changed(main_lane("starting", "ready", "starting")));
    assert_eq!(app.tab_title(), "devlanes · acme@0 · starting 1/2");
    app.update(changed(main_lane("stopping", "ready", "ready")));
    assert_eq!(app.tab_title(), "devlanes · acme@0 · stopping");
    app.update(changed(main_lane("running", "ready", "ready")));
    assert_eq!(app.tab_title(), "devlanes · acme@0");
    Ok(())
}

/// The notifications among `effects`.
fn notices(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Notify { title, body } => Some(format!("{title}: {body}")),
            _ => None,
        })
        .collect()
}

/// A `lane.phase` event of start `id` of acme@0.
fn phase_of(id: u64) -> Msg {
    Msg::Event(Event {
        seq: 3,
        event: events::PHASE.into(),
        data: json!({"lane": "acme@0", "phase": "services", "state": "started", "start_id": id}),
    })
}

/// The end of start `id` of acme@0.
fn ended(id: u64) -> Msg {
    Msg::Event(Event {
        seq: 4,
        event: events::START_ENDED.into(),
        data: json!({"lane": "acme@0", "start_id": id}),
    })
}

/// acme@0's api exiting with 137.
fn api_crashed() -> Msg {
    let mut crashed = unit("acme@0", "api", "failed");
    crashed.exit_code = Some(137);
    Msg::Event(Event {
        seq: 9,
        event: events::UNIT_CHANGED.into(),
        data: json!({ "unit": crashed }),
    })
}

/// A subscription to a daemon that sends `start.ended`.
fn tracking() -> Msg {
    Msg::Subscribed(Ok(vec![features::START_ENDED.into()]))
}

const CRASH: &str = "acme@0:api crashed: exited 137";

#[tokio::test]
async fn notifications_need_an_unfocused_terminal_and_a_long_start() -> TestResult {
    let mut app = loaded().await?;
    app.update(tracking());
    app.notify_after = Duration::ZERO;
    // Focused: the user sees it happen.
    app.update(phase_of(1));
    app.update(changed(main_lane("running", "ready", "ready")));
    assert!(notices(&app.update(ended(1))).is_empty());

    app.update(Msg::Focus(false));
    app.update(phase_of(2));
    app.update(changed(main_lane("running", "ready", "ready")));
    assert_eq!(
        notices(&app.update(ended(2))),
        ["acme@0 is up: 2 of 2 ready"]
    );
    // A failed start says so when it ends; its failure is no crash.
    app.update(phase_of(3));
    assert!(notices(&app.update(changed(main_lane("failed", "ready", "failed")))).is_empty());
    assert_eq!(
        notices(&app.update(ended(3))),
        ["start of acme@0 failed: 1 failed"]
    );
    // A start that answered with web still starting: the lane still reads
    // `starting`, but no start runs, so api failing is a crash.
    app.update(phase_of(4));
    app.update(changed(main_lane("starting", "ready", "starting")));
    app.update(ended(4));
    assert_eq!(notices(&app.update(api_crashed())), [CRASH]);
    // Two starts of the lane: one ending leaves the other running.
    app.update(changed(main_lane("running", "ready", "ready")));
    app.update(phase_of(5));
    app.update(phase_of(6));
    app.update(ended(5));
    assert!(notices(&app.update(api_crashed())).is_empty());
    // A lost connection may have lost start 6's end: after reconnecting, a
    // crash is a crash until a start shows itself again.
    app.update(changed(main_lane("running", "ready", "ready")));
    app.update(Msg::EventsEnded(None));
    app.update(tracking());
    assert_eq!(notices(&app.update(api_crashed())), [CRASH]);
    // A daemon without `start.ended`: no start is tracked, so a crash during
    // one is a crash and its end, seen only in the lane's word, notifies nobody.
    app.update(changed(main_lane("running", "ready", "ready")));
    app.update(Msg::EventsEnded(None));
    app.update(Msg::Subscribed(Ok(Vec::new())));
    app.update(phase_of(7));
    app.update(changed(main_lane("starting", "ready", "starting")));
    assert_eq!(notices(&app.update(api_crashed())), [CRASH]);
    assert!(notices(&app.update(changed(main_lane("running", "ready", "ready")))).is_empty());

    // A quick start is not worth one.
    let mut quick = loaded().await?;
    quick.update(tracking());
    quick.update(Msg::Focus(false));
    quick.update(phase_of(1));
    quick.update(changed(main_lane("running", "ready", "ready")));
    assert!(notices(&quick.update(ended(1))).is_empty());
    Ok(())
}

/// Runs the loop with `options` until `script` is done, then quits; returns
/// what was written to the terminal besides the screen.
async fn run(
    options: Options,
    script: impl AsyncFnOnce(Arc<FakeApi>, mpsc::Sender<std::io::Result<TermEvent>>, Captured),
) -> Result<String, Box<dyn std::error::Error>> {
    let api = Arc::new(FakeApi::with_lanes(mixed_lanes()));
    let (keys_tx, keys_rx) = mpsc::channel::<std::io::Result<TermEvent>>(8);
    let events = Box::pin(stream::unfold(keys_rx, |mut rx| async move {
        rx.recv().await.map(|event| (event, rx))
    }));
    let mut terminal = Terminal::new(TestBackend::new(120, 30))?;
    let driver_api: Arc<dyn DaemonApi> = api.clone();
    let out = Captured::default();
    let steps = {
        let (api, out) = (Arc::clone(&api), out.clone());
        async move {
            crate::wait_for_calls(&api, |calls| calls.iter().any(|c| c == "status")).await;
            script(api, keys_tx.clone(), out).await;
            let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
            keys_tx.send(Ok(TermEvent::Key(quit))).await.ok();
        }
    };
    let options = Options {
        on_quit: Some("keep".into()),
        ..options
    };
    let run = drive(
        &mut terminal,
        driver_api,
        &options,
        events,
        Box::new(out.clone()),
    );
    let (result, ()) =
        tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(run, steps) }).await?;
    result?;
    Ok(out.text())
}

#[tokio::test]
async fn the_loop_sets_the_title_links_urls_notifies_and_puts_the_title_back() -> TestResult {
    let options = Options {
        title: true,
        links: Some(Links::new(false)),
        notifications: Some(Notifications::new(NotifyStyle::Osc777)),
        sync_output: true,
        ..crate::hermetic_options()
    };
    let text = run(options, async |api, keys, out| {
        out.wait_for(0, "\x1b]0;devlanes · acme@0").await;
        // The lane opens on its failed worker; api, two rows up, shows its URL.
        for _ in 0..2 {
            let up = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
            keys.send(Ok(TermEvent::Key(up))).await.ok();
        }
        out.wait_for(0, "\x1b]8;;https://api.acme.localhost\x1b\\")
            .await;
        // The user switched away; then a running service crashed. The focus
        // report and the daemon's events reach the loop on separate channels,
        // so api crashes again until one crash lands after the report: those
        // seen while focused notify nobody and leave the rate limit alone.
        keys.send(Ok(TermEvent::FocusLost)).await.ok();
        let mut crashed = unit("acme@0", "api", "failed");
        crashed.exit_code = Some(1);
        let mut ready = unit("acme@0", "api", "ready");
        ready.urls = vec!["https://api.acme.localhost".into()];
        for _ in 0..200 {
            api.emit(events::UNIT_CHANGED, json!({ "unit": crashed }))
                .await;
            tokio::time::sleep(Duration::from_millis(10)).await;
            if out.text().contains("\x1b]777;notify;") {
                break;
            }
            api.emit(events::UNIT_CHANGED, json!({ "unit": ready }))
                .await;
        }
    })
    .await?;
    assert!(text.starts_with(PUSH_TITLE), "{text:?}");
    assert!(
        text.contains("\x1b]0;devlanes · acme@0 · 1 failed\x1b\\"),
        "{text:?}"
    );
    assert!(
        text.contains("\x1b[?2026h") && text.contains("\x1b[?2026l"),
        "{text:?}"
    );
    assert!(
        text.contains("\x1b]8;;https://api.acme.localhost\x1b\\"),
        "the pane's URL is a link: {text:?}"
    );
    assert!(
        text.contains("\x1b]777;notify;devlanes: acme@0:api crashed;exited 1\x1b\\"),
        "{text:?}"
    );
    // Quitting puts the previous title back, last.
    assert!(text.ends_with(POP_TITLE), "{text:?}");
    assert_eq!(text.matches(POP_TITLE).count(), 1);
    Ok(())
}

#[tokio::test]
async fn the_loop_writes_none_of_it_when_it_is_off() -> TestResult {
    let text = run(crate::hermetic_options(), async |api, keys, _| {
        keys.send(Ok(TermEvent::FocusLost)).await.ok();
        let crashed = unit("acme@0", "api", "failed");
        api.emit(events::UNIT_CHANGED, json!({ "unit": crashed }))
            .await;
        // Nothing is written to wait for; give the loop a moment to not write it.
        tokio::time::sleep(Duration::from_millis(100)).await;
    })
    .await?;
    for sequence in [
        PUSH_TITLE,
        POP_TITLE,
        "\x1b]0;",
        "\x1b]8;",
        "\x1b]777;",
        "\x1b[?2026h",
        "\x07",
    ] {
        assert!(!text.contains(sequence), "{sequence:?} in {text:?}");
    }
    Ok(())
}
