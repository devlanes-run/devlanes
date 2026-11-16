//! The demo's state without the page: the TUI on the fake daemon, fed keys
//! and seconds. The browser build draws it; tests drive it natively.

use devlanes_tui::{
    app::{Msg, View},
    keys::{KeyModifiers, MouseEvent, MouseEventKind},
    theme::{Glyphs, Theme, Tier},
};

use crate::{fake::Fake, keymap};

/// The views the page opens by name. The page says `lanes` for the project
/// view, as the website's docs do.
const VIEWS: [(&str, View); 4] = [
    ("lanes", View::Project),
    ("monitor", View::Monitor),
    ("daemon", View::Daemon),
    ("boxes", View::Boxes),
];

/// A view name [`Session::open`] does not know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownView(pub String);

impl std::fmt::Display for UnknownView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<_> = VIEWS.iter().map(|(name, _)| *name).collect();
        write!(
            f,
            "unknown view {:?}, expected one of: {}",
            self.0,
            names.join(", ")
        )
    }
}

impl std::error::Error for UnknownView {}

/// The TUI running on the fake daemon.
pub struct Session {
    fake: Fake,
}

impl Session {
    /// A TUI of `columns` x `rows` cells on the demo's scene (`myapp`,
    /// healthy, at this release) with its lanes loaded.
    pub fn new(columns: u16, rows: u16) -> Self {
        let mut fake = Fake::new(columns, rows);
        // The page is dark and the browser has no COLORTERM to detect from;
        // the ANSI tier would get Ratzilla's dim default palette.
        fake.app.theme = Theme {
            tier: Tier::Dark,
            glyphs: Glyphs::Unicode,
        };
        Self { fake }
    }

    /// The TUI's state, for drawing.
    pub fn app(&self) -> &devlanes_tui::app::App {
        &self.fake.app
    }

    /// Sends a browser key to the TUI; false when the page keeps the key
    /// (see [`keymap::key_event`]) and the TUI never saw it.
    pub fn key(&mut self, key: &str, ctrl: bool, alt: bool, shift: bool, meta: bool) -> bool {
        match keymap::key_event(key, ctrl, alt, shift, meta) {
            Some(event) => {
                self.fake.send(Msg::Key(event));
                true
            }
            None => false,
        }
    }

    /// One second passes: the fake daemon has a new metrics sample and the
    /// TUI ticks (spinners, notices, the monitor's refetch).
    pub fn tick(&mut self) {
        self.fake.tick();
    }

    /// Sends a mouse event at cell (`column`, `row`) to the TUI: a press,
    /// a release or a scroll step.
    pub fn mouse(&mut self, kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) {
        self.fake.send(Msg::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers,
        }));
    }

    /// Opens the view the page names (`lanes`, `monitor`, `daemon` or
    /// `boxes`) as if the user went there; nothing changes when it is open.
    pub fn open(&mut self, name: &str) -> Result<(), UnknownView> {
        let view = VIEWS
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, view)| *view)
            .ok_or_else(|| UnknownView(name.to_owned()))?;
        self.fake.open(view);
        Ok(())
    }

    /// The page's name for the open view, as [`Session::open`] takes it.
    pub fn view(&self) -> &'static str {
        VIEWS
            .iter()
            .find(|(_, view)| *view == self.fake.app.view)
            .map_or("lanes", |(name, _)| name)
    }

    /// The TUI's size in cells.
    pub fn size(&self) -> (u16, u16) {
        self.fake.app.size
    }

    /// The screen is now `columns` x `rows` cells; the TUI keeps its view,
    /// selection and open unit and lays them out again.
    pub fn resize(&mut self, columns: u16, rows: u16) {
        if self.size() != (columns, rows) {
            self.fake.send(Msg::Resize(columns, rows));
        }
    }
}
