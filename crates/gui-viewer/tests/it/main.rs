//! The viewer's decisions, driven through its model with a fake window
//! system and a fixed clock: no window opens.

use std::time::{Duration, Instant};

use devlanes_gui_client::Event;
use devlanes_gui_proto::{
    Buttons, CursorShape, Edge, KeyCode, LockState, MoveResize, Point, Pointer, Size, WindowId,
    WindowInfo, WindowKind,
};
use devlanes_gui_viewer::frame::{Drag, Os};
use devlanes_gui_viewer::model::{CHECK_AFTER, CLOSED_LINGER, LocalId, Out, Spec, Viewer, Windows};
use devlanes_gui_viewer::resize::SETTLE;

/// What the model asked of the window system.
#[derive(Debug, Clone, PartialEq)]
enum Call {
    Open(LocalId, String),
    Update(LocalId, String),
    Close(LocalId),
    Size(LocalId, Size),
    Drag(LocalId, Drag),
    Focus(LocalId),
    Exit,
}

/// A window system that records the calls that change what the user sees.
#[derive(Debug, Default)]
struct Fake(Vec<Call>);

impl Fake {
    /// The calls since the last time, for the next step's assertions.
    fn take(&mut self) -> Vec<Call> {
        std::mem::take(&mut self.0)
    }
}

impl Windows for Fake {
    /// Records the opened window by its title.
    fn open(&mut self, window: LocalId, spec: &Spec) {
        self.0.push(Call::Open(window, spec.title.clone()));
    }
    /// Records the new title.
    fn update(&mut self, window: LocalId, spec: &Spec) {
        self.0.push(Call::Update(window, spec.title.clone()));
    }
    /// Records the close.
    fn close(&mut self, window: LocalId) {
        self.0.push(Call::Close(window));
    }
    /// Records the size.
    fn set_size(&mut self, window: LocalId, size: Size) {
        self.0.push(Call::Size(window, size));
    }
    /// Redraws are not what the tests look at; the view is.
    fn redraw(&mut self, _: LocalId) {}
    /// Not looked at.
    fn set_cursor(&mut self, _: LocalId, _: &CursorShape) {}
    /// Records the drag.
    fn drag(&mut self, window: LocalId, drag: Drag) {
        self.0.push(Call::Drag(window, drag));
    }
    /// Records the focus.
    fn focus(&mut self, window: LocalId) {
        self.0.push(Call::Focus(window));
    }
    /// Not looked at.
    fn set_clipboard(&mut self, _: &str) {}
    /// Records the end.
    fn exit(&mut self) {
        self.0.push(Call::Exit);
    }
}

/// A main window `id` titled `title` that draws its own frame and reports
/// moves, as Wayland apps on the Linux host do.
fn window(id: u32, title: &str) -> WindowInfo {
    WindowInfo {
        id: WindowId(id),
        title: title.into(),
        app_id: String::new(),
        size: Size::new(800, 600),
        min_size: None,
        max_size: None,
        draws_own_frame: true,
        reports_moves: true,
        parent: None,
        offset: Point::new(0, 0),
        lock: LockState::Unlocked,
        kind: WindowKind::Toplevel,
    }
}

/// `n` milliseconds after `t0`.
fn at(t0: Instant, n: u64) -> Instant {
    t0 + Duration::from_millis(n)
}

/// A dropped link shows Reconnecting and keeps every local window; the next
/// host's windows take them over by id, or in order when a new host
/// numbers its windows anew, and only what nothing takes over closes.
#[test]
fn windows_survive_a_reconnect_to_the_same_or_a_new_host() {
    let t0 = Instant::now();
    let mut ui = Fake::default();
    let mut viewer = Viewer::new(Os::Linux, true, false);
    viewer.connected(vec![window(1, "main"), window(2, "second")], t0, &mut ui);
    let [Call::Open(main, _), Call::Open(second, _)] = ui.take()[..] else {
        panic!("two windows open");
    };
    assert_eq!(
        viewer.view(main, t0).map(|v| v.lock),
        Some(LockState::Unlocked)
    );

    viewer.disconnected(at(t0, 10), &mut ui);
    assert_eq!(ui.take(), [], "nothing closes while reconnecting");
    assert_eq!(viewer.take_out(), [], "and nothing stops the reconnect");
    assert_eq!(
        viewer.view(second, at(t0, 10)).map(|v| v.lock),
        Some(LockState::Reconnecting)
    );

    // The app restarted under the same host: ids kept.
    viewer.connected(
        vec![window(2, "second"), window(1, "main")],
        at(t0, 20),
        &mut ui,
    );
    assert!(
        ui.take()
            .iter()
            .all(|call| matches!(call, Call::Update(..)))
    );
    assert_eq!(
        viewer.view(main, at(t0, 20)).map(|v| v.lock),
        Some(LockState::Unlocked)
    );

    // A unit restart: a new host with new ids and one window fewer.
    viewer.disconnected(at(t0, 30), &mut ui);
    viewer.connected(vec![window(7, "main again")], at(t0, 40), &mut ui);
    assert_eq!(
        ui.take(),
        [Call::Update(main, "main again".into()), Call::Close(second)]
    );
    // The new id now names the old local window.
    viewer.event(
        Event::WindowChanged(WindowInfo {
            size: Size::new(1024, 768),
            ..window(7, "main again")
        }),
        at(t0, 50),
        &mut ui,
    );
    assert_eq!(ui.take(), [Call::Size(main, Size::new(1024, 768))]);
}

/// The viewer ends only once the service is gone: every window Closed by
/// the host when the link drops, or the daemon saying the unit no longer
/// runs. Then it shows Closed for a moment, stops reconnecting and exits.
#[test]
fn the_viewer_ends_when_the_service_is_gone() {
    let t0 = Instant::now();
    let closed = |id| Event::Lock {
        window: WindowId(id),
        state: LockState::Closed,
    };

    // The host said Closed: a stop or a unit that exited.
    let mut ui = Fake::default();
    let mut viewer = Viewer::new(Os::Windows, true, false);
    viewer.connected(vec![window(1, "main")], t0, &mut ui);
    let [Call::Open(main, _)] = ui.take()[..] else {
        panic!("one window");
    };
    viewer.event(closed(1), t0, &mut ui);
    viewer.disconnected(t0, &mut ui);
    assert_eq!(viewer.take_out(), [Out::StopReconnecting]);
    assert_eq!(
        viewer.view(main, t0).map(|v| v.lock),
        Some(LockState::Closed)
    );
    viewer.tick(at(t0, 100), &mut ui);
    assert_eq!(ui.take(), [], "Closed shows for a moment");
    assert_eq!(viewer.next_deadline(), Some(t0 + CLOSED_LINGER));
    viewer.tick(t0 + CLOSED_LINGER, &mut ui);
    assert_eq!(ui.take(), [Call::Close(main), Call::Exit]);

    // A restart whose new start failed leaves no host to say Closed: a
    // viewer started by `gui open` asks the daemon after a while.
    let mut viewer = Viewer::new(Os::Windows, true, true);
    viewer.connected(vec![window(1, "main")], t0, &mut ui);
    ui.take();
    viewer.disconnected(t0, &mut ui);
    viewer.tick(t0 + CHECK_AFTER - Duration::from_millis(1), &mut ui);
    assert_eq!(viewer.take_out(), []);
    viewer.tick(t0 + CHECK_AFTER, &mut ui);
    assert_eq!(viewer.take_out(), [Out::CheckUnit]);
    viewer.unit_checked(true, t0 + CHECK_AFTER, &mut ui);
    assert_eq!(
        viewer.next_deadline(),
        Some(t0 + CHECK_AFTER * 2),
        "asks again"
    );
    let later = t0 + CHECK_AFTER * 2;
    viewer.tick(later, &mut ui);
    assert_eq!(viewer.take_out(), [Out::CheckUnit]);
    viewer.unit_checked(false, later, &mut ui);
    assert_eq!(viewer.take_out(), [Out::StopReconnecting]);
    viewer.tick(later + CLOSED_LINGER, &mut ui);
    assert_eq!(ui.take().last(), Some(&Call::Exit));

    // One window still open on the host: only the link failed.
    let mut viewer = Viewer::new(Os::Windows, true, false);
    viewer.connected(vec![window(1, "a"), window(2, "b")], t0, &mut ui);
    viewer.event(closed(1), t0, &mut ui);
    viewer.disconnected(t0, &mut ui);
    assert_eq!(viewer.take_out(), []);
    assert_eq!(
        viewer.next_deadline(),
        None,
        "no check without a way to ask"
    );
}

/// The outer close button closes only the local window when the app has a
/// frame of its own inside, and it stays closed when the host describes the
/// window again, until `gui open` asks; without the app's frame it asks the
/// app to close.
#[test]
fn the_outer_close_button_follows_the_close_rule() {
    let t0 = Instant::now();
    let mut ui = Fake::default();
    let mut viewer = Viewer::new(Os::Linux, false, false);
    let x11 = WindowInfo {
        draws_own_frame: false,
        reports_moves: false,
        ..window(2, "xeyes")
    };
    let popup = WindowInfo {
        kind: WindowKind::Popup,
        parent: Some(WindowId(1)),
        ..window(3, "menu")
    };
    viewer.connected(vec![window(1, "main"), x11, popup], t0, &mut ui);
    let [
        Call::Open(main, _),
        Call::Open(xeyes, _),
        Call::Open(menu, _),
    ] = ui.take()[..]
    else {
        panic!("three windows");
    };

    viewer.close_requested(xeyes, &mut ui);
    assert_eq!(ui.take(), []);
    assert_eq!(viewer.take_out(), [Out::Close(WindowId(2))]);

    viewer.close_requested(main, &mut ui);
    assert_eq!(
        ui.take(),
        [Call::Close(main), Call::Close(menu)],
        "with its popup"
    );
    assert_eq!(viewer.take_out(), []);
    viewer.event(Event::WindowChanged(window(1, "renamed")), t0, &mut ui);
    assert_eq!(ui.take(), [], "stays closed");

    viewer.show(&mut ui);
    let calls = ui.take();
    assert!(
        calls.contains(&Call::Open(main, "renamed".into())),
        "{calls:?}"
    );
    assert!(
        calls.contains(&Call::Open(menu, "menu".into())),
        "{calls:?}"
    );
    assert!(calls.contains(&Call::Focus(main)), "{calls:?}");
    assert!(
        !calls.contains(&Call::Focus(menu)),
        "popups take no focus: {calls:?}"
    );
}

/// Input reaches the host only while the window is unlocked or frozen (a
/// click thaws a frozen lane), releases of what is held always do, focus
/// loss releases everything held, and a drag the app starts becomes a
/// local drag after the button is released to the app.
#[test]
fn input_follows_the_lock_and_never_leaves_anything_held() {
    let t0 = Instant::now();
    let mut ui = Fake::default();
    let mut viewer = Viewer::new(Os::Linux, true, false);
    viewer.connected(vec![window(1, "main")], t0, &mut ui);
    let [Call::Open(main, _)] = ui.take()[..] else {
        panic!("one window");
    };
    let id = WindowId(1);
    let pointer = |x, buttons, screen_delta| {
        Out::Pointer(Pointer {
            window: id,
            position: Point::new(x, 10),
            buttons,
            screen_delta,
        })
    };
    let a = KeyCode::keyboard(0x04);

    viewer.pointer_moved(main, Point::new(5, 10), t0);
    viewer.button(main, Buttons::LEFT, true, t0);
    viewer.key(main, a, true, t0);
    viewer.pointer_moved(main, Point::new(8, 10), t0);
    assert_eq!(
        viewer.take_out(),
        [
            pointer(5, Buttons::NONE, None),
            pointer(5, Buttons::LEFT, None),
            Out::Key(id, a, true),
            pointer(8, Buttons::LEFT, Some(Point::new(3, 0))),
        ]
    );

    // The user starts resizing the local window: presses stop, releases go.
    viewer.resized(main, Size::new(900, 600), at(t0, 10), &mut ui);
    viewer.button(main, Buttons::RIGHT, true, at(t0, 20));
    viewer.key(main, KeyCode::keyboard(0x05), true, at(t0, 20));
    viewer.key(main, a, false, at(t0, 20));
    viewer.button(main, Buttons::LEFT, false, at(t0, 20));
    assert_eq!(
        viewer.take_out(),
        [Out::Key(id, a, false), pointer(8, Buttons::NONE, None)]
    );
    viewer.tick(at(t0, 10) + SETTLE, &mut ui);
    assert_eq!(viewer.take_out(), [Out::Resize(id, Size::new(900, 600))]);
    viewer.event(
        Event::Lock {
            window: id,
            state: LockState::Unlocked,
        },
        at(t0, 400),
        &mut ui,
    );

    // Frozen: a click still goes out, to wake the lane.
    viewer.event(
        Event::Lock {
            window: id,
            state: LockState::Frozen,
        },
        at(t0, 500),
        &mut ui,
    );
    viewer.button(main, Buttons::LEFT, true, at(t0, 500));
    assert_eq!(viewer.take_out(), [pointer(8, Buttons::LEFT, None)]);
    viewer.event(
        Event::Lock {
            window: id,
            state: LockState::Unlocked,
        },
        at(t0, 600),
        &mut ui,
    );

    // Focus moves away with a key and a button held: both are released.
    viewer.key(main, a, true, at(t0, 600));
    viewer.focused(main, false, None);
    assert_eq!(
        viewer.take_out(),
        [
            Out::Key(id, a, true),
            Out::Key(id, a, false),
            pointer(8, Buttons::NONE, None),
            Out::Focus(id, false),
        ]
    );

    // A press on the app's title bar: the app starts a move.
    viewer.button(main, Buttons::LEFT, true, at(t0, 700));
    viewer.event(
        Event::MoveResizeStarted(MoveResize {
            window: id,
            edge: Some(Edge::Right),
            grab: Point::new(8, 10),
        }),
        at(t0, 710),
        &mut ui,
    );
    assert_eq!(
        viewer.take_out(),
        [
            pointer(8, Buttons::LEFT, None),
            pointer(8, Buttons::NONE, None)
        ]
    );
    assert_eq!(ui.take(), [Call::Drag(main, Drag::Resize(Edge::Right))]);
}
