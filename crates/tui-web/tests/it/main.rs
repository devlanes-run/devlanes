//! The web demo's session on the fake daemon: keys reach the TUI, ticks move
//! the monitor's graphs, and every state draws at the website's size.

use devlanes_tui::{app::View, ui};
use devlanes_tui_web::Session;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

/// The size the website gives the demo.
const COLUMNS: u16 = 120;
const ROWS: u16 = 40;

/// The session as the page shows it.
fn draw(session: &Session) -> Buffer {
    let mut terminal = match Terminal::new(TestBackend::new(COLUMNS, ROWS)) {
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
        .chunks(usize::from(COLUMNS))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn opens_on_the_fake_lanes_with_this_release() {
    let session = Session::new(COLUMNS, ROWS);
    assert_eq!(session.app().version, devlanes_version::VERSION);
    assert!(text(&draw(&session)).contains("acme"));
}

#[test]
fn shift_m_opens_the_monitor_and_ticks_move_its_graphs() {
    let mut session = Session::new(COLUMNS, ROWS);
    assert!(session.key("M", false, false, true));
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
    assert!(!session.key("Tab", false, false, false));
    assert!(!session.key("r", true, false, false));
    assert!(!session.key("Shift", false, false, true));
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
        session.key(key, false, false, shift);
        session.tick();
        draw(&session);
    }
}
