//! The viewer's decisions, driven through its model with a fake window
//! system and a fixed clock: no window opens.

use std::time::{Duration, Instant};

use devlanes_gui_client::Event;
use devlanes_gui_proto::{
    Buttons, CursorShape, Edge, KeyCode, LockState, MoveResize, Point, Pointer, Size, WindowId,
    WindowInfo, WindowKind, keys,
};
use devlanes_gui_viewer::frame::{Drag, Os};
use devlanes_gui_viewer::model::{
    CHECK_AFTER, CLOSED_LINGER, Check, DAEMON_AWAY, KeyInput, LocalId, Modifiers, Out, Spec,
    Viewer, Windows,
};
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

/// A viewer on `os` connected to a host with one main window, and its
/// local window.
fn one_window(os: Os, check: bool) -> (Viewer, Fake, LocalId) {
    let mut ui = Fake::default();
    let mut viewer = Viewer::new(os, true, check);
    viewer.connected(vec![window(1, "main")], Instant::now(), &mut ui);
    let [Call::Open(main, _)] = ui.take()[..] else {
        panic!("one window");
    };
    // The window's own first size event, as every OS sends one.
    viewer.resized(main, Size::new(800, 600), Instant::now(), &mut ui);
    (viewer, ui, main)
}

/// The lock event for window 1.
fn lock(state: LockState) -> Event {
    Event::Lock {
        window: WindowId(1),
        state,
    }
}

/// The pointer of window 1 at `x`,10 with `buttons`.
fn pointer(x: i32, buttons: Buttons, screen_delta: Option<Point>) -> Out {
    Out::Pointer(Pointer {
        window: WindowId(1),
        position: Point::new(x, 10),
        buttons,
        screen_delta,
    })
}

/// A dropped link shows Reconnecting and keeps every local window; the next
/// host's windows take them over by id, or in order when no id matches (a
/// new host numbers its windows anew), and only what nothing takes over
/// closes. Once an id matches, an unknown window is a new one.
#[test]
fn windows_survive_a_reconnect_to_the_same_or_a_new_host() {
    let t0 = Instant::now();
    let mut ui = Fake::default();
    let mut viewer = Viewer::new(Os::Linux, true, false);
    viewer.connected(vec![window(1, "main"), window(2, "second")], t0, &mut ui);
    let [Call::Open(main, _), Call::Open(second, _)] = ui.take()[..] else {
        panic!("two windows open");
    };

    viewer.disconnected(at(t0, 10), &mut ui);
    assert_eq!(ui.take(), [], "nothing closes while reconnecting");
    assert_eq!(viewer.take_out(), [], "and nothing stops the reconnect");
    assert_eq!(
        viewer.view(second, at(t0, 10)).map(|v| v.lock),
        Some(LockState::Reconnecting)
    );

    // The app restarted under the same host: ids kept, and a window it did
    // not have before is a new one, not a stand-in for an old one.
    viewer.connected(
        vec![window(1, "main"), window(9, "about")],
        at(t0, 20),
        &mut ui,
    );
    let calls = ui.take();
    assert!(
        calls.contains(&Call::Update(main, "main".into())),
        "{calls:?}"
    );
    assert!(calls.contains(&Call::Close(second)), "{calls:?}");
    assert!(
        calls
            .iter()
            .any(|call| matches!(call, Call::Open(_, title) if title == "about")),
        "{calls:?}"
    );
    assert_eq!(
        viewer.view(main, at(t0, 20)).map(|v| v.lock),
        Some(LockState::Unlocked)
    );

    // A unit restart: a new host with new ids.
    viewer.disconnected(at(t0, 30), &mut ui);
    viewer.connected(vec![window(7, "main again")], at(t0, 40), &mut ui);
    let calls = ui.take();
    assert_eq!(
        calls[0],
        Call::Update(main, "main again".into()),
        "{calls:?}"
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

/// The viewer ends only once the service is gone: the host showed every
/// window Closed (even when those windows went away before the link did), or
/// the daemon says the unit no longer runs, or the daemon stayed away for
/// [`DAEMON_AWAY`]. Then it shows Closed for a moment, stops reconnecting and
/// exits. A daemon away for a restart ends nothing.
#[test]
fn the_viewer_ends_when_the_service_is_gone() {
    let t0 = Instant::now();

    // The host showed Closed: a stop or a unit that exited.
    let (mut viewer, mut ui, main) = one_window(Os::Windows, false);
    viewer.event(lock(LockState::Closed), t0, &mut ui);
    viewer.tick(t0 + CLOSED_LINGER, &mut ui);
    assert_eq!(ui.take(), [Call::Close(main)], "a closed window goes");
    viewer.disconnected(t0 + CLOSED_LINGER, &mut ui);
    assert_eq!(viewer.take_out(), [Out::StopReconnecting]);
    viewer.tick(t0 + CLOSED_LINGER * 2, &mut ui);
    assert_eq!(ui.take(), [Call::Exit]);

    // A restart whose new start failed leaves no host to say Closed: the
    // viewer asks the daemon after a while, and again while it runs.
    let (mut viewer, mut ui, _) = one_window(Os::Windows, true);
    viewer.disconnected(t0, &mut ui);
    viewer.tick(t0 + CHECK_AFTER - Duration::from_millis(1), &mut ui);
    assert_eq!(viewer.take_out(), []);
    viewer.tick(t0 + CHECK_AFTER, &mut ui);
    assert_eq!(viewer.take_out(), [Out::CheckUnit]);
    viewer.unit_checked(Check::Runs, t0 + CHECK_AFTER, &mut ui);
    assert_eq!(
        viewer.next_deadline(),
        Some(t0 + CHECK_AFTER * 2),
        "asks again"
    );
    viewer.unit_checked(Check::Gone, t0 + CHECK_AFTER * 2, &mut ui);
    assert_eq!(viewer.take_out(), [Out::StopReconnecting]);
    viewer.tick(t0 + CHECK_AFTER * 2 + CLOSED_LINGER, &mut ui);
    assert_eq!(ui.take().last(), Some(&Call::Exit));

    // The daemon restarts: away once, back on the next check.
    let (mut viewer, mut ui, _) = one_window(Os::Windows, true);
    viewer.disconnected(t0, &mut ui);
    viewer.unit_checked(Check::DaemonAway, t0 + CHECK_AFTER, &mut ui);
    viewer.unit_checked(Check::Runs, t0 + CHECK_AFTER * 2, &mut ui);
    viewer.unit_checked(Check::DaemonAway, t0 + CHECK_AFTER * 3, &mut ui);
    assert_eq!(
        viewer.take_out(),
        [],
        "a daemon that came back ends nothing"
    );
    // A daemon that stays away: gone on a check DAEMON_AWAY after the first.
    viewer.unit_checked(
        Check::DaemonAway,
        t0 + CHECK_AFTER * 3 + DAEMON_AWAY,
        &mut ui,
    );
    assert_eq!(viewer.take_out(), [Out::StopReconnecting]);

    // One window still open on the host: only the link failed.
    let mut ui = Fake::default();
    let mut viewer = Viewer::new(Os::Windows, true, false);
    viewer.connected(vec![window(1, "a"), window(2, "b")], t0, &mut ui);
    viewer.event(lock(LockState::Closed), t0, &mut ui);
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

/// Input reaches the host only while the window is unlocked; while the
/// lane is frozen only button presses do (any input the host gets wakes
/// it), and a button held while the lane thaws is moved and released as
/// usual. Releases of what is held always go out, and focus leaving every
/// window of the service releases what is held.
#[test]
fn input_follows_the_lock_and_never_leaves_anything_held() {
    let t0 = Instant::now();
    let (mut viewer, mut ui, main) = one_window(Os::Linux, false);
    let id = WindowId(1);
    let a = KeyInput {
        code: Some(keys::ESCAPE),
        pressed: true,
        repeat: false,
        text: None,
        dead: false,
    };
    let none = Modifiers::default();

    viewer.pointer_moved(main, Point::new(5, 10), t0);
    viewer.button(main, Buttons::LEFT, true, t0);
    viewer.key(main, a.clone(), none, t0);
    viewer.pointer_moved(main, Point::new(8, 10), t0);
    assert_eq!(
        viewer.take_out(),
        [
            pointer(5, Buttons::NONE, None),
            pointer(5, Buttons::LEFT, None),
            Out::Key(id, keys::ESCAPE, true),
            pointer(8, Buttons::LEFT, Some(Point::new(3, 0))),
        ]
    );

    // The user starts resizing the local window: presses stop, releases go.
    viewer.resized(main, Size::new(900, 600), at(t0, 10), &mut ui);
    viewer.button(main, Buttons::RIGHT, true, at(t0, 20));
    viewer.key(
        main,
        KeyInput {
            code: Some(keys::TAB),
            ..a.clone()
        },
        none,
        at(t0, 20),
    );
    viewer.key(
        main,
        KeyInput {
            pressed: false,
            ..a.clone()
        },
        none,
        at(t0, 20),
    );
    viewer.button(main, Buttons::LEFT, false, at(t0, 20));
    assert_eq!(
        viewer.take_out(),
        [
            Out::Key(id, keys::ESCAPE, false),
            pointer(8, Buttons::NONE, None)
        ]
    );
    viewer.tick(at(t0, 10) + SETTLE, &mut ui);
    assert_eq!(viewer.take_out(), [Out::Resize(id, Size::new(900, 600))]);
    viewer.event(lock(LockState::Unlocked), at(t0, 400), &mut ui);

    // Frozen: hovering, scrolling, keys and focus changes send nothing; a
    // press goes out to wake the lane.
    viewer.event(lock(LockState::Frozen), at(t0, 500), &mut ui);
    viewer.pointer_moved(main, Point::new(20, 10), at(t0, 500));
    viewer.wheel(
        main,
        (0.0, 1.0, devlanes_gui_proto::WheelUnit::Lines),
        at(t0, 500),
    );
    viewer.key(main, a.clone(), none, at(t0, 500));
    viewer.pointer_left(main, at(t0, 500));
    viewer.focus(main, false, at(t0, 500));
    viewer.settle_focus(at(t0, 500));
    viewer.focus(main, true, at(t0, 500));
    assert_eq!(viewer.take_out(), []);
    viewer.pointer_moved(main, Point::new(20, 10), at(t0, 500));
    viewer.button(main, Buttons::LEFT, true, at(t0, 510));
    viewer.pointer_moved(main, Point::new(22, 10), at(t0, 520));
    assert_eq!(
        viewer.take_out(),
        [pointer(20, Buttons::LEFT, None)],
        "only the press"
    );
    // The lane thaws with the button still down: the drag goes on and ends.
    viewer.event(lock(LockState::Unlocked), at(t0, 600), &mut ui);
    viewer.pointer_moved(main, Point::new(25, 10), at(t0, 610));
    viewer.button(main, Buttons::LEFT, false, at(t0, 620));
    assert_eq!(
        viewer.take_out(),
        [
            pointer(25, Buttons::LEFT, Some(Point::new(3, 0))),
            pointer(25, Buttons::NONE, None),
        ]
    );

    // Focus leaves with a key and a button held: both are released.
    viewer.button(main, Buttons::LEFT, true, at(t0, 700));
    viewer.key(main, a.clone(), none, at(t0, 700));
    viewer.focus(main, false, at(t0, 700));
    viewer.settle_focus(at(t0, 700));
    assert_eq!(
        viewer.take_out(),
        [
            pointer(25, Buttons::LEFT, None),
            Out::Key(id, keys::ESCAPE, true),
            Out::Key(id, keys::ESCAPE, false),
            pointer(25, Buttons::NONE, None),
            Out::Focus(id, false),
        ]
    );
}

/// Moving focus between the viewer's own windows is no focus loss for the
/// app: no release of what is held, no Focus(false); the new window gets
/// focus.
#[test]
fn focus_moving_between_the_viewers_windows_stays_with_the_app() {
    let t0 = Instant::now();
    let mut ui = Fake::default();
    let mut viewer = Viewer::new(Os::Windows, true, false);
    viewer.connected(vec![window(1, "main"), window(2, "dialog")], t0, &mut ui);
    let [Call::Open(main, _), Call::Open(dialog, _)] = ui.take()[..] else {
        panic!("two windows");
    };
    viewer.pointer_moved(main, Point::new(5, 10), t0);
    viewer.button(main, Buttons::LEFT, true, t0);
    viewer.take_out();
    // The OS reports the loss first, then the gain, in one turn.
    viewer.focus(main, false, t0);
    viewer.focus(dialog, true, t0);
    viewer.settle_focus(t0);
    assert_eq!(viewer.take_out(), [Out::Focus(WindowId(2), true)]);
}

/// A move or resize the app reports becomes a local drag only while the
/// left button is down, after the button is released to the app.
#[test]
fn a_drag_starts_only_with_the_left_button_down() {
    let t0 = Instant::now();
    let (mut viewer, mut ui, main) = one_window(Os::Linux, false);
    let started = || {
        Event::MoveResizeStarted(MoveResize {
            window: WindowId(1),
            edge: Some(Edge::Right),
            grab: Point::new(8, 10),
        })
    };
    viewer.event(started(), t0, &mut ui);
    assert_eq!(ui.take(), [], "no button held");
    viewer.pointer_moved(main, Point::new(8, 10), t0);
    viewer.button(main, Buttons::LEFT, true, t0);
    viewer.take_out();
    viewer.event(started(), t0, &mut ui);
    assert_eq!(viewer.take_out(), [pointer(8, Buttons::NONE, None)]);
    assert_eq!(ui.take(), [Call::Drag(main, Drag::Resize(Edge::Right))]);
}

/// The text a key types goes out as text, so a layout other than the
/// host's types what is on its keys; shortcuts stay physical keys at their
/// positions. One row per case: the key, its text, the modifiers, and what
/// goes out for its press and release.
#[test]
fn keys_type_their_text_and_shortcuts_stay_physical_keys() {
    let t0 = Instant::now();
    let id = WindowId(1);
    let press = |code: KeyCode, text: Option<&str>| KeyInput {
        code: Some(code),
        pressed: true,
        repeat: false,
        text: text.map(str::to_owned),
        dead: false,
    };
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    let altgr = Modifiers {
        ctrl: true,
        alt: true,
        meta: false,
    };
    let cmd = Modifiers {
        meta: true,
        ..Modifiers::default()
    };
    let letter = |c| keys::letter(c).expect("letter");
    let cases: Vec<(&str, KeyInput, Modifiers, Vec<Out>)> = vec![
        (
            "US a",
            press(letter(b'a'), Some("a")),
            Modifiers::default(),
            vec![Out::Text(id, "a".into())],
        ),
        // AZERTY: the key at Q's place types a.
        (
            "AZERTY a",
            press(letter(b'q'), Some("a")),
            Modifiers::default(),
            vec![Out::Text(id, "a".into())],
        ),
        // German AltGr+Q is Control+Alt on Windows.
        (
            "German AltGr+Q",
            press(letter(b'q'), Some("@")),
            altgr,
            vec![Out::Text(id, "@".into())],
        ),
        (
            "Shift+a",
            press(letter(b'a'), Some("A")),
            Modifiers::default(),
            vec![Out::Text(id, "A".into())],
        ),
        (
            "Ctrl+C",
            press(letter(b'c'), Some("\u{3}")),
            ctrl,
            vec![
                Out::Key(id, letter(b'c'), true),
                Out::Key(id, letter(b'c'), false),
            ],
        ),
        (
            "Cmd+C",
            press(letter(b'c'), Some("c")),
            cmd,
            vec![
                Out::Key(id, letter(b'c'), true),
                Out::Key(id, letter(b'c'), false),
            ],
        ),
        (
            "Enter",
            press(keys::ENTER, Some("\r")),
            Modifiers::default(),
            vec![
                Out::Key(id, keys::ENTER, true),
                Out::Key(id, keys::ENTER, false),
            ],
        ),
        (
            "Shift",
            press(keys::SHIFT_LEFT, None),
            Modifiers::default(),
            vec![
                Out::Key(id, keys::SHIFT_LEFT, true),
                Out::Key(id, keys::SHIFT_LEFT, false),
            ],
        ),
    ];
    for (name, input, modifiers, expected) in cases {
        let (mut viewer, _, main) = one_window(Os::Windows, false);
        let release = KeyInput {
            pressed: false,
            text: None,
            ..input.clone()
        };
        viewer.key(main, input, modifiers, t0);
        viewer.key(main, release, modifiers, t0);
        assert_eq!(viewer.take_out(), expected, "{name}");
    }

    // A dead key types nothing; its character comes with the next press.
    let (mut viewer, _, main) = one_window(Os::Linux, false);
    let dead = KeyInput {
        dead: true,
        text: None,
        ..press(keys::BRACKET_LEFT, None)
    };
    viewer.key(main, dead.clone(), Modifiers::default(), t0);
    viewer.key(
        main,
        KeyInput {
            pressed: false,
            ..dead
        },
        Modifiers::default(),
        t0,
    );
    viewer.key(
        main,
        press(letter(b'e'), Some("ê")),
        Modifiers::default(),
        t0,
    );
    assert_eq!(viewer.take_out(), [Out::Text(id, "ê".into())]);

    // A held letter repeats its text; a held arrow is the app's to repeat.
    let (mut viewer, _, main) = one_window(Os::Mac, false);
    let held = press(letter(b'x'), Some("x"));
    viewer.key(main, held.clone(), Modifiers::default(), t0);
    viewer.key(
        main,
        KeyInput {
            repeat: true,
            ..held.clone()
        },
        Modifiers::default(),
        t0,
    );
    viewer.key(
        main,
        KeyInput {
            pressed: false,
            text: None,
            ..held
        },
        Modifiers::default(),
        t0,
    );
    let arrow = press(keys::LEFT, None);
    viewer.key(main, arrow.clone(), Modifiers::default(), t0);
    viewer.key(
        main,
        KeyInput {
            repeat: true,
            ..arrow
        },
        Modifiers::default(),
        t0,
    );
    assert_eq!(
        viewer.take_out(),
        [
            Out::Text(id, "x".into()),
            Out::Text(id, "x".into()),
            Out::Key(id, keys::LEFT, true),
        ]
    );
}

/// The local clipboard read while the link is down is sent once it is up,
/// and text the app set is not sent back.
#[test]
fn clipboard_text_waits_for_the_link_and_never_echoes() {
    let t0 = Instant::now();
    let (mut viewer, mut ui, _) = one_window(Os::Linux, false);
    viewer.event(Event::Clipboard("from the app".into()), t0, &mut ui);
    viewer.local_clipboard("from the app".into());
    assert_eq!(viewer.take_out(), [], "no echo");
    viewer.disconnected(t0, &mut ui);
    viewer.local_clipboard("copied here".into());
    assert_eq!(viewer.take_out(), []);
    viewer.connected(vec![window(1, "main")], at(t0, 100), &mut ui);
    assert_eq!(viewer.take_out(), [Out::Clipboard("copied here".into())]);
}
