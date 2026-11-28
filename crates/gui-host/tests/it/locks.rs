//! Lock states end to end: resizing with a clamp and on a static window,
//! frozen lanes, an app that restarts or closes, a unit that exits, and a
//! backend that disappears.

use devlanes_gui_host::Echo;
use devlanes_gui_proto::{Buttons, LockState, Point, RESIZE_TIMEOUT, Size, WAITING_GRACE, WindowId};

use super::*;

/// The next lock state reported for `window`.
async fn lock_of(events: &mut Events, window: WindowId) -> LockState {
    until(events, |event| match event {
        Event::Lock { window: w, state } if *w == window => Some(*state),
        _ => None,
    })
    .await
}

/// Waits until `window` reports `state`.
async fn locked(events: &mut Events, window: WindowId, state: LockState) {
    while lock_of(events, window).await != state {}
}

/// What a viewer saw during a resize, in order.
#[derive(Debug, PartialEq)]
enum Seen {
    Lock(LockState),
    Applied { request: u32, from_seq: u64 },
    Frame { seq: u64, size: Size },
}

/// Records what `window` shows until it is unlocked again.
async fn until_unlocked(events: &mut Events, window: WindowId) -> Vec<Seen> {
    let mut seen = Vec::new();
    let wait = async {
        while let Some(event) = events.next().await {
            match event {
                Event::Lock { window: w, state } if w == window => {
                    seen.push(Seen::Lock(state));
                    if state == LockState::Unlocked {
                        return;
                    }
                }
                Event::ResizeApplied {
                    window: w,
                    request,
                    from_seq,
                } if w == window => seen.push(Seen::Applied { request, from_seq }),
                Event::Frame(frame) if frame.window == window => seen.push(Seen::Frame {
                    seq: frame.seq,
                    size: Size::new(frame.frame.width(), frame.frame.height()),
                }),
                _ => {}
            }
        }
    };
    tokio::time::timeout(PATIENCE, wait).await.expect("unlocked");
    seen
}

#[tokio::test(start_paused = true)]
async fn a_resize_locks_the_window_until_the_first_frame_after_it_whatever_its_size() {
    let min = Size::new(300, 180);
    let options = TestPatternOptions {
        min_size: Some(min),
        ..TestPatternOptions::default()
    };
    let host = Host::start(options, |_| {});
    let window = host.pattern.first_window();
    let (ada, mut events) = host.viewer("ada").await;
    next_frame(&mut events).await;

    let request = ada.resize(window, Size::new(200, 100)).expect("sends");
    let seen = until_unlocked(&mut events, window).await;
    assert_eq!(seen.first(), Some(&Seen::Lock(LockState::Resizing)), "{seen:?}");
    let from = seen
        .iter()
        .find_map(|s| match s {
            Seen::Applied { request: r, from_seq } if *r == request => Some(*from_seq),
            _ => None,
        })
        .expect("the resize was answered");
    // The app clamped the size; the first frame after the change ends the lock anyway.
    let after = seen
        .iter()
        .position(|s| matches!(s, Seen::Frame { seq, size } if *seq >= from && *size == min));
    assert_eq!(after, Some(seen.len() - 2), "{seen:?}");
    assert_eq!(host.pattern.window_size(window), Some(min));
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_resize_of_a_static_window_that_keeps_its_size_unlocks_on_the_timeout() {
    let size = Size::new(320, 200);
    let options = TestPatternOptions {
        size,
        min_size: Some(size),
        changes_only: true,
        ..TestPatternOptions::default()
    };
    let host = Host::start(options, |_| {});
    let window = host.pattern.first_window();
    let (ada, mut events) = host.viewer("ada").await;
    next_frame(&mut events).await;

    let asked = tokio::time::Instant::now();
    let request = ada.resize(window, Size::new(200, 100)).expect("sends");
    let seen = until_unlocked(&mut events, window).await;
    assert!(asked.elapsed() >= RESIZE_TIMEOUT);
    assert!(seen.iter().any(|s| matches!(s, Seen::Applied { request: r, .. } if *r == request)));
    assert!(!seen.iter().any(|s| matches!(s, Seen::Frame { .. })), "{seen:?}");
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn the_lane_and_the_app_lock_the_window_end_to_end() {
    let host = Host::default();
    let window = host.pattern.first_window();
    let (ada, mut events) = host.viewer("ada").await;
    let (daemon, mut control) = host.control().await;
    let at = Point::new(30, 30);
    let pressed = |host: &Host| {
        host.pattern
            .echoes()
            .into_iter()
            .filter_map(|echo| match echo {
                Echo::Pressed { window, position, .. } => Some((window, position)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };

    // Frozen: a click reaches the host as activity, once, but not the app.
    daemon.lane_frozen(true).expect("sends");
    locked(&mut events, window, LockState::Frozen).await;
    for _ in 0..3 {
        ada.click(window, at, Buttons::LEFT).expect("sends");
    }
    ada.sync().await.expect("synced");
    until(&mut control, |e| matches!(e, Event::Activity).then_some(())).await;
    assert_eq!(host.activity.load(Ordering::SeqCst), 1);
    assert!(pressed(&host).is_empty());
    daemon.lane_frozen(false).expect("sends");
    locked(&mut events, window, LockState::Unlocked).await;
    ada.click(window, at, Buttons::LEFT).expect("sends");
    ada.sync().await.expect("synced");
    assert_eq!(pressed(&host), vec![(window, at)]);

    // The app closes its only window and opens another: it takes the old one's place.
    host.pattern.close_window(window);
    locked(&mut events, window, LockState::Waiting).await;
    let reopened = host.pattern.open_window(Size::new(320, 200));
    let info = until(&mut events, |e| match e {
        Event::WindowChanged(info) if info.id == window => Some(info.clone()),
        _ => None,
    })
    .await;
    assert_eq!(info.lock, LockState::Unlocked);
    assert_eq!(ada.lock_state(window), Some(LockState::Unlocked));
    let there = Point::new(90, 50);
    ada.click(window, there, Buttons::LEFT).expect("sends");
    ada.sync().await.expect("synced");
    assert_eq!(pressed(&host).last(), Some(&(reopened, there)));

    // Gone for longer than the grace period, it is closed and removed.
    host.pattern.close_window(reopened);
    locked(&mut events, window, LockState::Waiting).await;
    let gone = tokio::time::Instant::now();
    locked(&mut events, window, LockState::Closed).await;
    assert!(gone.elapsed() >= WAITING_GRACE);
    until(&mut events, |e| matches!(e, Event::WindowRemoved(w) if *w == window).then_some(()))
        .await;

    // A new window after that is a window of its own, closed for good when the unit exits.
    let next = host.pattern.open_window(Size::new(320, 200));
    until(&mut events, |e| matches!(e, Event::WindowAdded(info) if info.id == next).then_some(()))
        .await;
    daemon.unit_exited(Some(0)).expect("sends");
    locked(&mut events, next, LockState::Closed).await;
    host.stop().await;
}

#[tokio::test(start_paused = true)]
async fn a_backend_that_disappears_closes_the_windows_and_the_host() {
    let host = Host::default();
    let window = host.pattern.first_window();
    let (_ada, mut events) = host.viewer("ada").await;
    host.pattern.vanish();
    locked(&mut events, window, LockState::Closed).await;
    until(&mut events, |e| matches!(e, Event::Closed(_)).then_some(())).await;
    let result = tokio::time::timeout(PATIENCE, host.served)
        .await
        .expect("the host stops")
        .expect("no panic");
    assert!(matches!(result, Err(ServeError::BackendStopped)), "{result:?}");
}
