//! The JavaScript API: a [`Demo`] mounted into an element of the page,
//! drawn with Ratzilla's DOM backend.

use std::{cell::RefCell, rc::Rc};

use devlanes_tui::ui;
use ratzilla::{
    DomBackend,
    ratatui::{Terminal, TerminalOptions, Viewport, backend::Backend, layout::Rect},
};
use wasm_bindgen::{JsCast, prelude::*};

use crate::{palette, session::Session};

/// The devlanes TUI on a fake daemon, mounted into one element.
#[wasm_bindgen]
pub struct Demo {
    screen: Rc<RefCell<Screen>>,
    /// Redraws everything after a window resize; removed when the demo is freed.
    on_resize: Closure<dyn FnMut()>,
}

/// The session and the terminal it draws on.
struct Screen {
    session: Session,
    terminal: Terminal<DomBackend>,
}

impl Screen {
    /// Draws the TUI as it stands, in the demo's palette.
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

    /// Draws every cell again. Ratzilla rebuilds its grid empty on a window
    /// resize, while the terminal would only send the cells that changed.
    fn redraw(&mut self) -> Result<(), JsError> {
        self.terminal.clear().map_err(error)?;
        self.draw()
    }
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
        let size = backend.window_size().map_err(error)?.columns_rows;
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, size.width, size.height)),
            },
        )
        .map_err(error)?;
        let screen = Rc::new(RefCell::new(Screen {
            session: Session::new(size.width, size.height),
            terminal,
        }));
        screen.borrow_mut().draw()?;
        let weak = Rc::downgrade(&screen);
        // Registered after the backend's own resize listener, so the backend
        // has marked its grid for rebuilding by the time this one runs.
        let on_resize = Closure::<dyn FnMut()>::new(move || {
            let Some(screen) = weak.upgrade() else {
                return;
            };
            if let Ok(mut screen) = screen.try_borrow_mut()
                && let Err(failure) = screen.redraw()
            {
                web_sys::console::error_1(&failure.into());
            }
        });
        window()?
            .add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref())
            .map_err(js_error)?;
        Ok(Demo { screen, on_resize })
    }

    /// Sends a `KeyboardEvent.key` with its Ctrl, Alt and Shift flags and
    /// redraws. Returns false when the page should keep the key (Tab,
    /// modifiers alone, browser shortcuts), so the caller only prevents the
    /// browser's default when this is true.
    pub fn key(&mut self, key: &str, ctrl: bool, alt: bool, shift: bool) -> Result<bool, JsError> {
        let mut screen = self.screen.borrow_mut();
        let used = screen.session.key(key, ctrl, alt, shift);
        if used {
            screen.draw()?;
        }
        Ok(used)
    }

    /// Advances the fake daemon by one second and redraws; the page calls
    /// it every second while the demo is on screen.
    pub fn tick(&mut self) -> Result<(), JsError> {
        let mut screen = self.screen.borrow_mut();
        screen.session.tick();
        screen.draw()
    }
}

impl Drop for Demo {
    /// Stops listening for resizes once the page frees the demo.
    fn drop(&mut self) {
        if let Ok(window) = window() {
            let _ = window.remove_event_listener_with_callback(
                "resize",
                self.on_resize.as_ref().unchecked_ref(),
            );
        }
    }
}

/// The page's window.
fn window() -> Result<web_sys::Window, JsError> {
    web_sys::window().ok_or_else(|| JsError::new("no window"))
}

/// Any error as a JavaScript `Error` with its message.
fn error(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}

/// A thrown JavaScript value as an `Error`.
fn js_error(value: JsValue) -> JsError {
    JsError::new(&value.as_string().unwrap_or_else(|| format!("{value:?}")))
}
