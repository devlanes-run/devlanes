//! The JavaScript API: a [`Demo`] mounted into an element of the page,
//! drawn with Ratzilla's DOM backend.

use devlanes_tui::ui;
use ratzilla::{
    DomBackend,
    ratatui::{Terminal, TerminalOptions, Viewport, layout::Rect},
};
use wasm_bindgen::prelude::*;

use crate::{palette, session::Session};

/// The devlanes TUI on a fake daemon, mounted into one element.
#[wasm_bindgen]
pub struct Demo {
    session: Session,
    terminal: Terminal<DomBackend>,
}

#[wasm_bindgen]
impl Demo {
    /// Mounts the TUI into the element with id `element_id`, which must be
    /// empty: the backend replaces its content. The element's CSS size,
    /// over the size of one cell in its font, gives the TUI's size.
    #[wasm_bindgen(constructor)]
    pub fn new(element_id: &str) -> Result<Demo, JsError> {
        console_error_panic_hook::set_once();
        let mut backend = DomBackend::new_by_id(element_id).map_err(error)?;
        // Ratzilla 0.3's `size()` is the window's, not the element's, so the
        // viewport is pinned to the grid the backend built in the element.
        let size = ratzilla::ratatui::backend::Backend::window_size(&mut backend)
            .map_err(error)?
            .columns_rows;
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, size.width, size.height)),
            },
        )
        .map_err(error)?;
        let mut demo = Demo {
            session: Session::new(size.width, size.height),
            terminal,
        };
        demo.draw()?;
        Ok(demo)
    }

    /// Sends a `KeyboardEvent.key` with its Ctrl, Alt and Shift flags and
    /// redraws. Returns false when the page should keep the key (Tab,
    /// modifiers alone, browser shortcuts), so the caller only prevents the
    /// browser's default when this is true.
    pub fn key(&mut self, key: &str, ctrl: bool, alt: bool, shift: bool) -> Result<bool, JsError> {
        let used = self.session.key(key, ctrl, alt, shift);
        if used {
            self.draw()?;
        }
        Ok(used)
    }

    /// Advances the fake daemon by one second and redraws; the page calls
    /// it every second while the demo is on screen.
    pub fn tick(&mut self) -> Result<(), JsError> {
        self.session.tick();
        self.draw()
    }

    /// Draws the TUI as it stands.
    fn draw(&mut self) -> Result<(), JsError> {
        let app = self.session.app();
        self.terminal
            .draw(|frame| {
                ui::render(app, frame);
                palette::apply(frame.buffer_mut());
            })
            .map(drop)
            .map_err(error)
    }
}

/// Any error as a JavaScript `Error` with its message.
fn error(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}
