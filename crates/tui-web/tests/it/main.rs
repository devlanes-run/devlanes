//! The web demo's session on the fake daemon: keys reach the TUI, ticks move
//! the monitor's graphs, resizes keep the view, and every state draws.

use devlanes_tui::{
    app::View,
    demo::web,
    doctor::{self, CheckState},
    ui,
};
use devlanes_tui_web::Session;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

/// The size the website gives the demo on a wide screen.
const COLUMNS: u16 = 120;
const ROWS: u16 = 40;

/// The session as the page shows it, at the session's size.
fn draw(session: &Session) -> Buffer {
    let (columns, rows) = session.size();
    let mut terminal = match Terminal::new(TestBackend::new(columns, rows)) {
        Ok(terminal) => terminal,
        Err(error) => panic!("test backend: {error}"),
    };
    if let Err(error) = terminal.draw(|frame| ui::render(session.app(), frame)) {
        panic!("draw: {error}");
    }
    terminal.backend().buffer().clone()
}

/// The buffer's text, row by row.
fn text(buffer: &Buffer) -> String {
    buffer
        .content()
        .chunks(usize::from(buffer.area.width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn opens_on_the_website_project_at_this_release() {
    let session = Session::new(COLUMNS, ROWS);
    assert_eq!(session.app().version, devlanes_version::VERSION);
    let screen = text(&draw(&session));
    assert!(screen.contains("myapp"), "{screen}");
    assert!(!screen.contains("acme"), "{screen}");
    // The daemon runs this release too, so nothing offers an update.
    assert!(!screen.contains("update"), "{screen}");
}

#[test]
fn the_website_daemon_passes_every_doctor_check() {
    let failing: Vec<_> = doctor::checks(&web::doctor())
        .into_iter()
        .filter(|check| check.state != CheckState::Ok)
        .collect();
    assert!(failing.is_empty(), "{failing:?}");
}

#[test]
fn a_resize_keeps_the_view_and_lays_it_out_again() {
    let mut session = Session::new(COLUMNS, ROWS);
    assert!(session.key("M", false, false, true, false));
    session.resize(96, 32);
    assert_eq!(session.size(), (96, 32));
    assert_eq!(session.app().view, View::Monitor);
    assert!(text(&draw(&session)).contains("units"));
    session.resize(COLUMNS, ROWS);
    assert_eq!(session.app().view, View::Monitor);
    draw(&session);
}

#[test]
fn shift_m_opens_the_monitor_and_ticks_move_its_graphs() {
    let mut session = Session::new(COLUMNS, ROWS);
    assert!(session.key("M", false, false, true, false));
    assert_eq!(session.app().view, View::Monitor);
    let newest = |session: &Session| {
        session
            .app()
            .metrics
            .as_ref()
            .and_then(|metrics| metrics.units.first())
            .and_then(|unit| unit.history.last())
            .map(|point| point.0)
    };
    session.tick();
    let before = newest(&session);
    session.tick();
    let after = newest(&session);
    assert!(before.is_some());
    assert_eq!(after, before.map(|ms| ms + 1_000));
    draw(&session);
}

#[test]
fn the_page_keeps_tab_and_browser_shortcuts() {
    let mut session = Session::new(COLUMNS, ROWS);
    assert!(!session.key("Tab", false, false, false, false));
    assert!(!session.key("r", true, false, false, false));
    assert!(!session.key("Shift", false, false, true, false));
    // Command+Shift+M on macOS: the browser's, so the monitor stays closed.
    assert!(!session.key("M", false, false, true, true));
    assert_eq!(session.app().view, View::Project);
}

#[test]
fn every_key_of_the_hints_draws() {
    let mut session = Session::new(COLUMNS, ROWS);
    for (key, shift) in [
        ("j", false),
        ("k", false),
        ("Enter", false),
        ("Escape", false),
        ("?", true),
        ("Escape", false),
        ("M", true),
        ("q", false),
        ("Escape", false),
    ] {
        session.key(key, false, false, shift, false);
        session.tick();
        draw(&session);
    }
}
