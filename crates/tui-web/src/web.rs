//! The JavaScript API: a [`Demo`] mounted into an element of the page,
//! drawn with Ratzilla's DOM backend.

use std::{cell::RefCell, rc::Rc};

use devlanes_tui::ui;
use ratzilla::{
    DomBackend,
    ratatui::{Terminal, TerminalOptions, Viewport, backend::Backend, layout::Rect},
};
use wasm_bindgen::{JsCast, prelude::*};

use devlanes_tui::keys::{KeyModifiers, MouseButton, MouseEventKind};

use crate::{mouse, palette, session::Session};

/// Marks the elements a demo is mounted into, for [`STYLE`].
const MARK: &str = "data-devlanes-tui-web";

/// Ratzilla measures a cell with a bare `<pre>` probe and draws rows as bare
/// `<pre>`s, which take the browser's monospace font and margins instead of
/// the element's; this makes them inherit the element's font.
const STYLE: &str = "[data-devlanes-tui-web] pre { font: inherit; margin: 0; }";

/// The id of the `<style>` element holding [`STYLE`], added once per page.
const STYLE_ID: &str = "devlanes-tui-web-style";

/// The devlanes TUI on a fake daemon, mounted into one element.
#[wasm_bindgen]
pub struct Demo {
    screen: Rc<RefCell<Option<Screen>>>,
    element_id: String,
    /// Mounts again after a window resize; removed when the demo is freed.
    on_resize: Closure<dyn FnMut()>,
    /// Pointer and wheel listeners on the element, by event type; on the
    /// element rather than Ratzilla's grid, which every resize replaces.
    on_pointer: Vec<Listener>,
}

/// An event listener and the event type it was added for.
type Listener = (&'static str, Closure<dyn FnMut(web_sys::Event)>);

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

    /// Builds a new grid in the element at its current size, keeps the
    /// session at that size, and draws every cell.
    fn remount(&mut self, element_id: &str) -> Result<(), JsError> {
        // A hidden element measures as nothing, and Ratzilla would then size
        // the grid by the window; the old grid stays until it shows again.
        if !visible(element_id)? {
            return Ok(());
        }
        self.terminal = mount(element_id)?;
        let size = self.terminal.get_frame().area();
        self.session.resize(size.width, size.height);
        self.draw()
    }
}

/// A terminal on a new Ratzilla grid in the element `element_id`, emptied
/// first, with the viewport pinned to the grid: Ratzilla 0.3's `size()` is
/// the window's, not the element's.
fn mount(element_id: &str) -> Result<Terminal<DomBackend>, JsError> {
    let element = element(element_id)?;
    element.set_inner_html("");
    element.set_attribute(MARK, "").map_err(js_error)?;
    let mut backend = DomBackend::new_by_id(element_id).map_err(error)?;
    let size = backend.window_size().map_err(error)?.columns_rows;
    Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, size.width, size.height)),
        },
    )
    .map_err(error)
}

#[wasm_bindgen]
impl Demo {
    /// Mounts the TUI into the element with id `element_id`; the backend
    /// replaces its content. The element's CSS size, over the size of one
    /// cell in its font, gives the TUI's size.
    #[wasm_bindgen(constructor)]
    pub fn new(element_id: &str) -> Result<Demo, JsError> {
        console_error_panic_hook::set_once();
        add_style()?;
        let screen: Rc<RefCell<Option<Screen>>> = Rc::new(RefCell::new(None));
        let weak = Rc::downgrade(&screen);
        let id = element_id.to_owned();
        // Registered before any backend's own resize listener, so this one
        // runs first and replaces the backend before Ratzilla empties its
        // grid; a listener added during that dispatch is not called for it.
        let on_resize = Closure::<dyn FnMut()>::new(move || {
            let Some(screen) = weak.upgrade() else {
                return;
            };
            let Ok(mut screen) = screen.try_borrow_mut() else {
                return;
            };
            if let Some(screen) = screen.as_mut()
                && let Err(failure) = screen.remount(&id)
            {
                web_sys::console::error_1(&failure.into());
            }
        });
        window()?
            .add_event_listener_with_callback("resize", on_resize.as_ref().unchecked_ref())
            .map_err(js_error)?;
        let on_pointer = listen_pointer(&screen, element_id)?;
        let demo = Demo {
            screen,
            element_id: element_id.to_owned(),
            on_resize,
            on_pointer,
        };
        let mut terminal = mount(element_id)?;
        let size = terminal.get_frame().area();
        let mut mounted = Screen {
            session: Session::new(size.width, size.height),
            terminal,
        };
        mounted.draw()?;
        *demo.screen.borrow_mut() = Some(mounted);
        Ok(demo)
    }

    /// Sends a `KeyboardEvent.key` with its Ctrl, Alt, Shift and Meta flags
    /// and redraws. Returns false when the page should keep the key (Tab,
    /// modifiers alone, Meta and other browser shortcuts), so the caller
    /// only prevents the browser's default when this is true. A missing
    /// `meta` from older callers reads as false.
    pub fn key(
        &mut self,
        key: &str,
        ctrl: bool,
        alt: bool,
        shift: bool,
        meta: bool,
    ) -> Result<bool, JsError> {
        self.with(|screen| {
            let used = screen.session.key(key, ctrl, alt, shift, meta);
            if used {
                screen.draw()?;
            }
            Ok(used)
        })
    }

    /// Advances the fake daemon by one second and redraws; the page calls
    /// it every second while the demo is on screen.
    pub fn tick(&mut self) -> Result<(), JsError> {
        self.with(|screen| {
            screen.session.tick();
            screen.draw()
        })
    }

    /// Measures the element again and lays the TUI out at its new size,
    /// keeping the view, the selection and the open unit. Window resizes do
    /// this by themselves; the page calls it after it changed the element's
    /// size or showed it again without one.
    pub fn resize(&mut self) -> Result<(), JsError> {
        let id = self.element_id.clone();
        self.with(|screen| screen.remount(&id))
    }

    /// The TUI's width in cells.
    #[wasm_bindgen(getter)]
    pub fn columns(&self) -> u16 {
        self.size().0
    }

    /// The TUI's height in cells.
    #[wasm_bindgen(getter)]
    pub fn rows(&self) -> u16 {
        self.size().1
    }
}

impl Demo {
    /// Runs `call` on the mounted screen.
    fn with<T>(
        &mut self,
        call: impl FnOnce(&mut Screen) -> Result<T, JsError>,
    ) -> Result<T, JsError> {
        match self.screen.borrow_mut().as_mut() {
            Some(screen) => call(screen),
            None => Err(JsError::new("the demo is not mounted")),
        }
    }

    /// The TUI's size in cells, or nothing before it mounted.
    fn size(&self) -> (u16, u16) {
        self.screen
            .borrow()
            .as_ref()
            .map_or((0, 0), |screen| screen.session.size())
    }
}

impl Drop for Demo {
    /// Unmounts when the page frees the demo: stops listening for resizes,
    /// the pointer and the wheel, and empties the element.
    fn drop(&mut self) {
        let Ok(window) = window() else {
            return;
        };
        let _ = window
            .remove_event_listener_with_callback("resize", self.on_resize.as_ref().unchecked_ref());
        if let Ok(element) = element(&self.element_id) {
            for (kind, listener) in &self.on_pointer {
                let _ = element
                    .remove_event_listener_with_callback(kind, listener.as_ref().unchecked_ref());
            }
            element.set_inner_html("");
            let _ = element.remove_attribute(MARK);
        }
    }
}

/// Listens on the element for presses, releases and, while the element has
/// focus, wheel turns, and sends them to the TUI at the cell under the
/// pointer. Moves and drags stay with the browser, so text in the demo can
/// be selected and copied as on any page; the right button keeps its menu.
fn listen_pointer(
    screen: &Rc<RefCell<Option<Screen>>>,
    element_id: &str,
) -> Result<Vec<Listener>, JsError> {
    let target = element(element_id)?;
    let mut listeners = Vec::new();
    for kind in ["pointerdown", "pointerup", "wheel"] {
        let weak = Rc::downgrade(screen);
        let id = element_id.to_owned();
        let mut wheel = mouse::Wheel::default();
        let listener = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
            let Some(screen) = weak.upgrade() else {
                return;
            };
            let Ok(mut screen) = screen.try_borrow_mut() else {
                return;
            };
            let Some(screen) = screen.as_mut() else {
                return;
            };
            if let Err(failure) = pointer(screen, &id, &event, &mut wheel) {
                web_sys::console::error_1(&failure.into());
            }
        });
        // Not passive: a wheel turn the TUI takes must not scroll the page.
        let options = web_sys::AddEventListenerOptions::new();
        options.set_passive(false);
        target
            .add_event_listener_with_callback_and_add_event_listener_options(
                kind,
                listener.as_ref().unchecked_ref(),
                &options,
            )
            .map_err(js_error)?;
        listeners.push((kind, listener));
    }
    Ok(listeners)
}

/// Sends one pointer or wheel `event` on the element `element_id` to the TUI
/// and redraws.
fn pointer(
    screen: &mut Screen,
    element_id: &str,
    event: &web_sys::Event,
    wheel: &mut mouse::Wheel,
) -> Result<(), JsError> {
    let Some(mouse_event) = event.dyn_ref::<web_sys::MouseEvent>() else {
        return Ok(());
    };
    let target = element(element_id)?;
    // Ratzilla's grid is the element's first child; the element itself can
    // be a little larger than the cells.
    let grid = target
        .first_element_child()
        .unwrap_or_else(|| target.clone());
    let rect = grid.get_bounding_client_rect();
    let bounds = mouse::Bounds {
        left: rect.left(),
        top: rect.top(),
        width: rect.width(),
        height: rect.height(),
    };
    let (columns, rows) = screen.session.size();
    let Some((column, row)) = mouse::cell(
        f64::from(mouse_event.client_x()),
        f64::from(mouse_event.client_y()),
        bounds,
        columns,
        rows,
    ) else {
        return Ok(());
    };
    let mut modifiers = KeyModifiers::NONE;
    if mouse_event.ctrl_key() {
        modifiers |= KeyModifiers::CONTROL;
    }
    if mouse_event.alt_key() {
        modifiers |= KeyModifiers::ALT;
    }
    if mouse_event.shift_key() {
        modifiers |= KeyModifiers::SHIFT;
    }
    let kinds = if let Some(turn) = event.dyn_ref::<web_sys::WheelEvent>() {
        // Until the demo was clicked, the wheel scrolls the page past it.
        let focused = document()?
            .active_element()
            .is_some_and(|active| active == target);
        if !focused {
            return Ok(());
        }
        event.prevent_default();
        let steps = wheel.steps(turn.delta_y(), turn.delta_mode(), bounds.height);
        let kind = if steps > 0 {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        };
        vec![kind; steps.unsigned_abs() as usize]
    } else {
        let button = match mouse_event.button() {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            _ => return Ok(()),
        };
        match event.type_().as_str() {
            "pointerdown" => vec![MouseEventKind::Down(button)],
            _ => vec![MouseEventKind::Up(button)],
        }
    };
    if kinds.is_empty() {
        return Ok(());
    }
    for kind in kinds {
        screen.session.mouse(kind, column, row, modifiers);
    }
    screen.draw()
}

/// Adds [`STYLE`] to the page once.
fn add_style() -> Result<(), JsError> {
    let document = document()?;
    if document.get_element_by_id(STYLE_ID).is_some() {
        return Ok(());
    }
    let style = document.create_element("style").map_err(js_error)?;
    style.set_id(STYLE_ID);
    style.set_text_content(Some(STYLE));
    document
        .head()
        .ok_or_else(|| JsError::new("no document head"))?
        .append_child(&style)
        .map_err(js_error)?;
    Ok(())
}

/// Whether the element has a size to measure.
fn visible(element_id: &str) -> Result<bool, JsError> {
    let rect = element(element_id)?.get_bounding_client_rect();
    Ok(rect.width() > 0.0 && rect.height() > 0.0)
}

/// The page's window.
fn window() -> Result<web_sys::Window, JsError> {
    web_sys::window().ok_or_else(|| JsError::new("no window"))
}

/// The page's document.
fn document() -> Result<web_sys::Document, JsError> {
    window()?
        .document()
        .ok_or_else(|| JsError::new("no document"))
}

/// The element with id `element_id`.
fn element(element_id: &str) -> Result<web_sys::Element, JsError> {
    document()?
        .get_element_by_id(element_id)
        .ok_or_else(|| JsError::new(&format!("no element with id {element_id}")))
}

/// Any error as a JavaScript `Error` with its message.
fn error(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}

/// A thrown JavaScript value as an `Error`.
fn js_error(value: JsValue) -> JsError {
    JsError::new(&value.as_string().unwrap_or_else(|| format!("{value:?}")))
}
