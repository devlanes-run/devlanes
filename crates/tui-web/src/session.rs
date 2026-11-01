//! The demo's state without the page: the TUI on the fake daemon, fed keys
//! and seconds. The browser build draws it; tests drive it natively.

use devlanes_tui::{
    app::Msg,
    demo::{Fake, Scene},
    theme::{Glyphs, Theme, Tier},
};

use crate::keymap;

/// The TUI running on the fake daemon.
pub struct Session {
    fake: Fake,
}

impl Session {
    /// A TUI of `columns` x `rows` cells on the website's scene
    /// ([`Scene::web`]: `myapp`, healthy, at this release) with its lanes
    /// loaded.
    pub fn new(columns: u16, rows: u16) -> Self {
        let mut fake = Fake::with_scene(columns, rows, Scene::web());
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
