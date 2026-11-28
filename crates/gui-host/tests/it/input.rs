//! Input arbitration between clients, releases of held input, and `Sync`.

use devlanes_gui_host::{CLIPBOARD_GAP, Command, Echo, MAX_PENDING_SYNCS};
use devlanes_gui_proto::{
    Buttons, HELD_INPUT_TIMEOUT, HOLD_AFTER_TYPING, Key, KeyCode, LockState, Point, Pointer,
    WindowId,
};

use super::*;

/// A pointer message.
fn pointer(window: WindowId, position: Point, buttons: Buttons) -> Pointer {
    Pointer {
        window,
        position,
        buttons,
        screen_delta: None,
    }
}

/// Where the app saw presses.
fn presses(host: &Host) -> Vec<Point> {
    host.pattern
        .echoes()
        .into_iter()
        .filter_map(|echo| match echo {
            Echo::Pressed { position, .. } => Some(position),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_viewer_holding_a_button_keeps_input_and_the_other_is_told_who_has_it() {
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let (ada, _ada_events) = host.viewer("ada").await;
    let (bob, mut bob_events) = host.viewer("bob").await;
    let (ours, theirs) = (Point::new(10, 10), Point::new(200, 150));

    ada.pointer(pointer(window, ours, Buttons::LEFT))
        .expect("sends");
    ada.sync().await.expect("synced");
    bob.click(window, theirs, Buttons::LEFT).expect("sends");
    let holder = until(&mut bob_events, |event| match event {
        Event::InputHolder(Some(holder)) => Some(holder.clone()),
        _ => None,
    })
    .await;
    assert_eq!(
        (holder.session, holder.name.as_str()),
        (ada.session(), "ada")
    );
    bob.sync().await.expect("synced");
    assert_eq!(presses(&host), vec![ours], "bob's click was dropped");

    ada.pointer(pointer(window, ours, Buttons::NONE))
        .expect("sends");
    ada.sync().await.expect("synced");
    bob.click(window, theirs, Buttons::LEFT).expect("sends");
    bob.sync().await.expect("synced");
    assert_eq!(presses(&host), vec![ours, theirs]);
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_holder_that_goes_silent_is_released_and_one_that_leaves_at_once() {
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let (bob, _events) = host.tool("bob").await;
    let (held, theirs) = (Point::new(40, 40), Point::new(250, 120));

    // A raw client sends no keep-alives, so it goes silent while it holds the button.
    let mut ada = Raw::welcomed(&host, Role::Tool).await;
    ada.send(&ClientMessage::Pointer(pointer(
        window,
        held,
        Buttons::LEFT,
    )))
    .await;
    ada.round_trip().await;
    assert_eq!(presses(&host), vec![held]);
    bob.click(window, theirs, Buttons::LEFT).expect("sends");
    bob.sync().await.expect("synced");
    assert_eq!(presses(&host), vec![held], "ada still holds input");

    tokio::time::sleep(HELD_INPUT_TIMEOUT).await;
    bob.click(window, theirs, Buttons::LEFT).expect("sends");
    bob.sync().await.expect("synced");
    assert_eq!(presses(&host), vec![held, theirs]);
    let buttons: Vec<_> = host
        .pattern
        .commands()
        .into_iter()
        .filter_map(|command| match command {
            Command::Pointer(p) => Some(p.buttons),
            _ => None,
        })
        .collect();
    // Ada's press, its release by the host, then bob's whole click.
    let (left, none) = (Buttons::LEFT, Buttons::NONE);
    assert_eq!(buttons, vec![left, none, none, left, none]);

    // Back from silence, ada's stale "still held" does not press again.
    ada.send(&ClientMessage::Pointer(pointer(
        window,
        held,
        Buttons::LEFT,
    )))
    .await;
    ada.round_trip().await;
    assert_eq!(presses(&host), vec![held, theirs]);

    // A holder whose connection closes is released without the wait.
    let mut cy = Raw::welcomed(&host, Role::Tool).await;
    let at = Point::new(70, 20);
    cy.send(&ClientMessage::Pointer(pointer(window, at, Buttons::LEFT)))
        .await;
    cy.round_trip().await;
    assert_eq!(presses(&host).last(), Some(&at));
    let left_at = tokio::time::Instant::now();
    drop(cy);
    eventually("cy's button is released", || {
        matches!(
            host.pattern.commands().last(),
            Some(Command::Pointer(p)) if p.buttons == Buttons::NONE
        )
    })
    .await;
    assert!(left_at.elapsed() < HELD_INPUT_TIMEOUT);
    assert_eq!(presses(&host), vec![held, theirs, at]);
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_key_released_on_a_locked_window_reaches_the_app_and_frees_input() {
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let (ada, mut ada_events) = host.viewer("ada").await;
    let (bob, _bob_events) = host.tool("bob").await;
    let (daemon, _control) = host.control().await;
    let f11 = KeyCode::keyboard(0x44);
    let lock = |state| {
        move |event: &Event| match event {
            Event::Lock { state: s, .. } if *s == state => Some(()),
            _ => None,
        }
    };

    ada.key(window, f11, true).expect("sends");
    ada.sync().await.expect("synced");
    daemon.lane_frozen(true).expect("sends");
    until(&mut ada_events, lock(LockState::Frozen)).await;
    ada.key(window, f11, false).expect("sends");
    ada.sync().await.expect("synced");
    let up = Command::Key(Key {
        window,
        code: f11,
        down: false,
    });
    assert_eq!(
        host.pattern.commands().iter().filter(|c| **c == up).count(),
        1
    );

    daemon.lane_frozen(false).expect("sends");
    until(&mut ada_events, lock(LockState::Unlocked)).await;
    // Typing holds input for a moment after the last key, the release included.
    tokio::time::sleep(HOLD_AFTER_TYPING).await;
    bob.text(window, "bob").expect("sends");
    bob.sync().await.expect("synced");
    let typed = Echo::Text {
        window,
        text: "bob".into(),
    };
    assert!(
        host.pattern.echoes().contains(&typed),
        "ada still held input"
    );
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn syncs_share_one_frame_request_per_window_and_are_capped() {
    let host = Host::default().await;
    let (tool, _events) = host.tool("agent").await;
    let requests = |host: &Host| {
        host.pattern
            .commands()
            .iter()
            .filter(|c| matches!(c, Command::RequestFrame { .. }))
            .count()
    };
    host.pattern.hold_frames(true);
    let release = async {
        // Answered at once, after every sync above it was handled.
        let _ = tool.screenshot(Some(WindowId(999))).await;
        assert_eq!(
            requests(&host),
            1,
            "the later syncs wait for the first request"
        );
        host.pattern.hold_frames(false);
    };
    let (first, second, third, ()) = tokio::join!(tool.sync(), tool.sync(), tool.sync(), release);
    for synced in [first, second, third] {
        synced.expect("synced");
    }
    // One request answered the first sync; one more covered the two after it.
    assert_eq!(requests(&host), 2);

    // A client that does not wait for its syncs is closed past the cap.
    host.pattern.hold_frames(true);
    let mut flood = Raw::welcomed(&host, Role::Tool).await;
    for request in 0..=MAX_PENDING_SYNCS as u32 {
        flood.send(&ClientMessage::Sync { request }).await;
    }
    let after = flood.closed().await;
    assert!(
        !after
            .iter()
            .any(|m| matches!(m, HostMessage::Synced { .. })),
        "{after:?}"
    );
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn clipboard_text_within_the_gap_reaches_the_app_once_and_newest() {
    let host = Host::default().await;
    let (tool, _events) = host.tool("agent").await;
    let texts = |host: &Host| {
        host.pattern
            .commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Clipboard { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    for text in ["zero", "one", "two"] {
        tool.clipboard(text).expect("sends");
    }
    // Answered at once, after the clipboard messages were handled.
    let _ = tool.screenshot(Some(WindowId(999))).await;
    assert_eq!(texts(&host), vec!["zero"]);
    let sent = tokio::time::Instant::now();
    eventually("the newest text follows", || texts(&host).len() == 2).await;
    assert!(sent.elapsed() >= CLIPBOARD_GAP - Duration::from_millis(10));
    assert_eq!(texts(&host), vec!["zero", "two"]);
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn sync_answers_only_after_a_frame_captured_after_the_input() {
    let host = Host::default().await;
    let window = host.pattern.first_window();
    let (tool, _events) = host.tool("agent").await;
    let at = Point::new(100, 60);

    host.pattern.hold_frames(true);
    tool.click(window, at, Buttons::LEFT).expect("sends");
    let sync = tool.sync();
    tokio::pin!(sync);
    let early = tokio::time::timeout(Duration::from_secs(1), &mut sync).await;
    assert!(early.is_err(), "answered before the app drew");
    host.pattern.hold_frames(false);
    sync.await.expect("synced");

    // The frame the sync waited for is the one screenshots now show.
    let shot = tool.screenshot(Some(window)).await.expect("a screenshot");
    let row = shot.frame.row(at.y as u32).expect("the row");
    let x = at.x as usize * 4;
    assert_eq!(&row[x..x + 4], &[0, 0, 0, 0xff], "the click marker");
    host.stop().await;
}
