//! Input arbitration between clients, releases of held input, and `Sync`.

use devlanes_gui_host::{Command, Echo};
use devlanes_gui_proto::{Buttons, HELD_INPUT_TIMEOUT, Point, Pointer, WindowId};

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
    let host = Host::default();
    let window = host.pattern.first_window();
    let (ada, _ada_events) = host.viewer("ada").await;
    let (bob, mut bob_events) = host.viewer("bob").await;
    let (ours, theirs) = (Point::new(10, 10), Point::new(200, 150));

    ada.pointer(pointer(window, ours, Buttons::LEFT)).expect("sends");
    ada.sync().await.expect("synced");
    bob.click(window, theirs, Buttons::LEFT).expect("sends");
    let holder = until(&mut bob_events, |event| match event {
        Event::InputHolder(Some(holder)) => Some(holder.clone()),
        _ => None,
    })
    .await;
    assert_eq!((holder.session, holder.name.as_str()), (ada.session(), "ada"));
    bob.sync().await.expect("synced");
    assert_eq!(presses(&host), vec![ours], "bob's click was dropped");

    ada.pointer(pointer(window, ours, Buttons::NONE)).expect("sends");
    ada.sync().await.expect("synced");
    bob.click(window, theirs, Buttons::LEFT).expect("sends");
    bob.sync().await.expect("synced");
    assert_eq!(presses(&host), vec![ours, theirs]);
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_holder_that_goes_silent_is_released_and_one_that_leaves_at_once() {
    let host = Host::default();
    let window = host.pattern.first_window();
    let (bob, _events) = host.tool("bob").await;
    let (held, theirs) = (Point::new(40, 40), Point::new(250, 120));

    // A raw client sends no keep-alives, so it goes silent while it holds the button.
    let mut ada = Raw::welcomed(&host, Role::Tool).await;
    ada.send(&ClientMessage::Pointer(pointer(window, held, Buttons::LEFT)))
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
    ada.send(&ClientMessage::Pointer(pointer(window, held, Buttons::LEFT)))
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
async fn sync_answers_only_after_a_frame_captured_after_the_input() {
    let host = Host::default();
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
