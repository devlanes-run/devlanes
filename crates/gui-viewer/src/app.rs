//! The winit and softbuffer layer: it turns window events into calls on the
//! [`Viewer`] and the viewer's [`Windows`] calls into OS windows. Everything
//! it decides is a translation; the decisions are the model's.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::Instant;

use devlanes_gui_client::Event;
use devlanes_gui_proto::{CursorShape, Edge, Point, Size};
use softbuffer::{Context, Surface};
use tokio::sync::mpsc;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, OwnedDisplayHandle};
use winit::keyboard::{Key, ModifiersState, PhysicalKey};
use winit::window::{
    Cursor, CursorIcon, CustomCursor, ResizeDirection, Window, WindowId as OsWindowId, WindowLevel,
};

use crate::frame::{Decoration, Drag};
use crate::link::{Frames, FromLink, Tell};
use crate::model::{KeyInput, LocalId, Modifiers, Out, Spec, Viewer, Windows, overlay_text};
use crate::{ViewerError, input, paint};

/// Size of a window whose app reported no size yet; replaced by the first.
const PLACEHOLDER: Size = Size::new(640, 480);

/// One open local window.
struct Local {
    window: Rc<Window>,
    surface: Option<Surface<OwnedDisplayHandle, Rc<Window>>>,
}

/// The local windows by both ids.
#[derive(Default)]
struct Locals {
    open: HashMap<LocalId, Local>,
    by_os: HashMap<OsWindowId, LocalId>,
}

/// The application: the model, the windows and the way to the connection.
pub(crate) struct App {
    viewer: Viewer,
    locals: Locals,
    context: Option<Context<OwnedDisplayHandle>>,
    clipboard: Option<arboard::Clipboard>,
    to_link: mpsc::UnboundedSender<Out>,
    /// The newest frame of each window, which [`FromLink::Frame`] points at.
    frames: Frames,
    /// Back to this thread, for what blocking threads read.
    tell: Tell,
    /// The connection's runtime, whose blocking threads read the clipboard.
    runtime: tokio::runtime::Handle,
    /// The modifiers held now, from the windows' reports.
    modifiers: ModifiersState,
    /// Why the viewer ended, when it failed.
    pub(crate) error: Option<ViewerError>,
}

impl App {
    /// An application for `viewer`, sending what it asks on `to_link`,
    /// taking frames from `frames`, and reading the clipboard on `runtime`'s
    /// blocking threads with the answer through `tell`.
    pub(crate) fn new(
        viewer: Viewer,
        to_link: mpsc::UnboundedSender<Out>,
        frames: Frames,
        tell: Tell,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            viewer,
            locals: Locals::default(),
            context: None,
            clipboard: None,
            to_link,
            frames,
            tell,
            runtime,
            modifiers: ModifiersState::empty(),
            error: None,
        }
    }

    /// Reads the local clipboard on a blocking thread, since an X11 or
    /// Wayland clipboard owner may take its time to answer, and hands the
    /// text back to this thread.
    fn read_clipboard(&self) {
        let tell = self.tell.clone();
        let started = self.viewer.host_clipboards();
        self.runtime.spawn_blocking(move || {
            let text = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text());
            if let Ok(text) = text {
                tell(FromLink::LocalClipboard(text, started));
            }
        });
    }

    /// Runs `f` on the model with the OS windows behind [`Windows`], then
    /// sends what the model asked of the connection.
    fn with_ui<R>(
        &mut self,
        event_loop: &ActiveEventLoop,
        f: impl FnOnce(&mut Viewer, &mut Os<'_>) -> R,
    ) -> R {
        if self.context.is_none() {
            match Context::new(event_loop.owned_display_handle()) {
                Ok(context) => self.context = Some(context),
                Err(error) => tracing::warn!(%error, "cannot draw into windows"),
            }
        }
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new()
                .inspect_err(|error| tracing::debug!(%error, "no clipboard"))
                .ok();
        }
        let mut os = Os {
            event_loop,
            locals: &mut self.locals,
            context: self.context.as_ref(),
            clipboard: self.clipboard.as_mut(),
        };
        let result = f(&mut self.viewer, &mut os);
        for out in self.viewer.take_out() {
            let _ = self.to_link.send(out);
        }
        result
    }

    /// Draws `local` from the model.
    fn draw(&mut self, local: LocalId) {
        let Some(open) = self.locals.open.get_mut(&local) else {
            return;
        };
        let Some(view) = self.viewer.view(local, Instant::now()) else {
            return;
        };
        let Some(surface) = open.surface.as_mut() else {
            return;
        };
        let size = open.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return;
        };
        let scale = (open.window.scale_factor() * 2.0).round() as u32;
        let drawn = surface.resize(width, height).and_then(|()| {
            let mut buffer = surface.buffer_mut()?;
            paint::paint(
                &mut buffer,
                size.width,
                size.height,
                view.frame,
                overlay_text(view.lock),
                scale,
            );
            buffer.present()
        });
        if let Err(error) = drawn {
            tracing::debug!(%error, "a window was not drawn");
        }
    }
}

impl ApplicationHandler<FromLink> for App {
    /// Nothing to do: windows open when the host lists them.
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    /// Handles what the connection reports.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: FromLink) {
        let now = Instant::now();
        match event {
            FromLink::Connected(windows) => {
                self.with_ui(event_loop, |viewer, ui| viewer.connected(windows, now, ui));
            }
            FromLink::Event(event) => {
                self.with_ui(event_loop, |viewer, ui| viewer.event(event, now, ui));
            }
            FromLink::Disconnected => {
                self.with_ui(event_loop, |viewer, ui| viewer.disconnected(now, ui));
            }
            FromLink::Frame(window) => {
                if let Some(frame) = self.frames.take(window) {
                    self.with_ui(event_loop, |viewer, ui| {
                        viewer.event(Event::Frame(frame), now, ui)
                    });
                }
            }
            FromLink::UnitChecked(check) => {
                self.with_ui(event_loop, |viewer, ui| viewer.unit_checked(check, now, ui));
            }
            FromLink::Show => self.with_ui(event_loop, |viewer, ui| viewer.show(ui)),
            FromLink::LocalClipboard(text, started) => {
                self.viewer.local_clipboard(text, started);
                self.with_ui(event_loop, |_, _| ());
            }
            FromLink::Failed(error) => {
                self.error = Some(ViewerError::Connect(error));
                event_loop.exit();
            }
            FromLink::Crashed(message) => {
                self.error = Some(ViewerError::Crashed(message));
                event_loop.exit();
            }
        }
    }

    /// Handles what a local window reports.
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: OsWindowId, event: WindowEvent) {
        let Some(&local) = self.locals.by_os.get(&id) else {
            return;
        };
        let now = Instant::now();
        match event {
            WindowEvent::RedrawRequested => self.draw(local),
            WindowEvent::Resized(size) => {
                let size = Size::new(size.width, size.height);
                self.with_ui(event_loop, |viewer, ui| {
                    viewer.resized(local, size, now, ui)
                });
            }
            WindowEvent::CloseRequested => {
                self.with_ui(event_loop, |viewer, ui| viewer.close_requested(local, ui));
            }
            WindowEvent::Focused(focused) => {
                if focused {
                    self.read_clipboard();
                }
                self.viewer.focus(local, focused, now);
                self.with_ui(event_loop, |_, _| ());
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::CursorMoved { position, .. } => {
                let point = Point::new(position.x.floor() as i32, position.y.floor() as i32);
                self.viewer.pointer_moved(local, point, now);
                self.with_ui(event_loop, |_, _| ());
            }
            WindowEvent::CursorLeft { .. } => {
                self.viewer.pointer_left(local, now);
                self.with_ui(event_loop, |_, _| ());
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = input::button(button) {
                    let pressed = state == ElementState::Pressed;
                    self.viewer.button(local, button, pressed, now);
                    self.with_ui(event_loop, |_, _| ());
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(delta) = input::wheel(delta) {
                    self.viewer.wheel(local, delta, now);
                    self.with_ui(event_loop, |_, _| ());
                }
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                let key = match event.physical_key {
                    PhysicalKey::Code(key) => Some(key),
                    PhysicalKey::Unidentified(_) => None,
                };
                // X11 reports keys held while the window gained focus as
                // synthetic presses: only modifiers count, so a Control held
                // while clicking into the window reaches the app.
                if is_synthetic && !key.is_some_and(input::is_modifier) {
                    return;
                }
                let input = KeyInput {
                    code: key.and_then(input::key_code),
                    pressed: event.state == ElementState::Pressed,
                    repeat: event.repeat,
                    text: event.text.as_ref().map(|text| text.to_string()),
                    dead: matches!(event.logical_key, Key::Dead(_)),
                    navigation: key.and_then(|key| input::navigation(key, &event.logical_key)),
                };
                let modifiers = Modifiers {
                    ctrl: self.modifiers.control_key(),
                    alt: self.modifiers.alt_key(),
                    meta: self.modifiers.super_key(),
                };
                self.viewer.key(local, input, modifiers, now);
                self.with_ui(event_loop, |_, _| ());
            }
            _ => {}
        }
    }

    /// Runs what is due and sleeps until the model's next deadline.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        // Every event of this turn is in, so a focus loss without a gain of
        // another window of the service means focus left it.
        self.viewer.settle_focus(now);
        self.with_ui(event_loop, |viewer, ui| viewer.tick(now, ui));
        event_loop.set_control_flow(match self.viewer.next_deadline() {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}

/// The OS windows as the model sees them, for one call.
struct Os<'a> {
    event_loop: &'a ActiveEventLoop,
    locals: &'a mut Locals,
    context: Option<&'a Context<OwnedDisplayHandle>>,
    clipboard: Option<&'a mut arboard::Clipboard>,
}

impl Os<'_> {
    /// The attributes of a new window for `spec`.
    fn attributes(&self, spec: &Spec) -> winit::window::WindowAttributes {
        let size = if spec.size.is_nonzero() {
            spec.size
        } else {
            PLACEHOLDER
        };
        let mut attributes = Window::default_attributes()
            .with_title(spec.title.clone())
            .with_inner_size(PhysicalSize::new(size.width, size.height))
            .with_active(spec.focus);
        if let Some(min) = spec.min_size {
            attributes = attributes.with_min_inner_size(PhysicalSize::new(min.width, min.height));
        }
        if let Some(max) = spec.max_size {
            attributes = attributes.with_max_inner_size(PhysicalSize::new(max.width, max.height));
        }
        match spec.decoration {
            Decoration::Framed => attributes,
            Decoration::Borderless => borderless(attributes.with_decorations(false)),
            Decoration::HiddenTitleBar => hidden_title_bar(attributes),
            Decoration::Popup => {
                let mut attributes = popup(
                    attributes
                        .with_decorations(false)
                        .with_resizable(false)
                        .with_window_level(WindowLevel::AlwaysOnTop),
                );
                // Where the OS lets a window be placed; Wayland does not.
                let parent = spec
                    .parent
                    .and_then(|(parent, offset)| Some((self.locals.open.get(&parent)?, offset)));
                if let Some((parent, offset)) = parent
                    && let Ok(origin) = parent.window.inner_position()
                {
                    attributes = attributes.with_position(PhysicalPosition::new(
                        origin.x + offset.x,
                        origin.y + offset.y,
                    ));
                }
                attributes
            }
        }
    }
}

/// A borderless window on Windows keeps the shadow and rounded corners the
/// frame would have drawn.
#[cfg(windows)]
fn borderless(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
    attributes
        .with_undecorated_shadow(true)
        .with_corner_preference(CornerPreference::Round)
}

/// Elsewhere the compositor rounds and shadows windows as it does.
#[cfg(not(windows))]
fn borderless(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    attributes
}

/// A popup has no taskbar button of its own on Windows.
#[cfg(windows)]
fn popup(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    use winit::platform::windows::WindowAttributesExtWindows;
    attributes.with_skip_taskbar(true)
}

/// Elsewhere a popup is a plain undecorated window.
#[cfg(not(windows))]
fn popup(attributes: winit::window::WindowAttributes) -> winit::window::WindowAttributes {
    attributes
}

/// macOS: a resizable window whose title bar is hidden, transparent and
/// without buttons, so the app's own frame shows edge to edge.
#[cfg(target_os = "macos")]
fn hidden_title_bar(
    attributes: winit::window::WindowAttributes,
) -> winit::window::WindowAttributes {
    use winit::platform::macos::WindowAttributesExtMacOS;
    attributes
        .with_titlebar_transparent(true)
        .with_title_hidden(true)
        .with_titlebar_buttons_hidden(true)
        .with_fullsize_content_view(true)
}

/// Only macOS asks for it; elsewhere it is borderless.
#[cfg(not(target_os = "macos"))]
fn hidden_title_bar(
    attributes: winit::window::WindowAttributes,
) -> winit::window::WindowAttributes {
    borderless(attributes.with_decorations(false))
}

/// winit's direction for a resize from `edge`.
fn direction(edge: Edge) -> ResizeDirection {
    match edge {
        Edge::Top => ResizeDirection::North,
        Edge::Bottom => ResizeDirection::South,
        Edge::Left => ResizeDirection::West,
        Edge::Right => ResizeDirection::East,
        Edge::TopLeft => ResizeDirection::NorthWest,
        Edge::TopRight => ResizeDirection::NorthEast,
        Edge::BottomLeft => ResizeDirection::SouthWest,
        Edge::BottomRight => ResizeDirection::SouthEast,
    }
}

/// winit's icon for a named cursor shape; `None` for a hidden cursor or a
/// picture.
fn icon(shape: &CursorShape) -> Option<CursorIcon> {
    Some(match shape {
        CursorShape::Default => CursorIcon::Default,
        CursorShape::Pointer => CursorIcon::Pointer,
        CursorShape::Text => CursorIcon::Text,
        CursorShape::Crosshair => CursorIcon::Crosshair,
        CursorShape::Move => CursorIcon::Move,
        CursorShape::Grab => CursorIcon::Grab,
        CursorShape::Grabbing => CursorIcon::Grabbing,
        CursorShape::NotAllowed => CursorIcon::NotAllowed,
        CursorShape::Wait => CursorIcon::Wait,
        CursorShape::Progress => CursorIcon::Progress,
        CursorShape::Help => CursorIcon::Help,
        CursorShape::EwResize => CursorIcon::EwResize,
        CursorShape::NsResize => CursorIcon::NsResize,
        CursorShape::NeswResize => CursorIcon::NeswResize,
        CursorShape::NwseResize => CursorIcon::NwseResize,
        CursorShape::ColResize => CursorIcon::ColResize,
        CursorShape::RowResize => CursorIcon::RowResize,
        CursorShape::Hidden | CursorShape::Image(_) => return None,
    })
}

impl Windows for Os<'_> {
    /// Creates the window and its surface.
    fn open(&mut self, local: LocalId, spec: &Spec) {
        let window = match self.event_loop.create_window(self.attributes(spec)) {
            Ok(window) => Rc::new(window),
            Err(error) => {
                tracing::warn!(%error, title = %spec.title, "a window did not open");
                return;
            }
        };
        if !spec.takes_input {
            let _ = window.set_cursor_hittest(false);
        }
        let surface = self.context.and_then(|context| {
            Surface::new(context, window.clone())
                .inspect_err(|error| tracing::warn!(%error, "cannot draw into a window"))
                .ok()
        });
        self.locals.by_os.insert(window.id(), local);
        window.request_redraw();
        self.locals.open.insert(local, Local { window, surface });
    }

    /// Applies the title and size limits.
    fn update(&mut self, local: LocalId, spec: &Spec) {
        let Some(open) = self.locals.open.get(&local) else {
            return;
        };
        open.window.set_title(&spec.title);
        let size = |size: Option<Size>| size.map(|s| PhysicalSize::new(s.width, s.height));
        open.window.set_min_inner_size(size(spec.min_size));
        open.window.set_max_inner_size(size(spec.max_size));
    }

    /// Drops the window, which closes it.
    fn close(&mut self, local: LocalId) {
        if let Some(open) = self.locals.open.remove(&local) {
            self.locals.by_os.remove(&open.window.id());
        }
    }

    /// Asks the OS for the size.
    fn set_size(&mut self, local: LocalId, size: Size) {
        if let Some(open) = self.locals.open.get(&local) {
            let _ = open
                .window
                .request_inner_size(PhysicalSize::new(size.width, size.height));
        }
    }

    /// Asks the OS for a redraw.
    fn redraw(&mut self, local: LocalId) {
        if let Some(open) = self.locals.open.get(&local) {
            open.window.request_redraw();
        }
    }

    /// Sets the cursor over the window.
    fn set_cursor(&mut self, local: LocalId, shape: &CursorShape) {
        let Some(open) = self.locals.open.get(&local) else {
            return;
        };
        open.window
            .set_cursor_visible(!matches!(shape, CursorShape::Hidden));
        if let Some(icon) = icon(shape) {
            open.window.set_cursor(Cursor::Icon(icon));
        } else if let CursorShape::Image(image) = shape {
            // winit takes straight RGBA; the protocol has premultiplied BGRA.
            let rgba: Vec<u8> = image
                .pixels
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|bgra| {
                    let alpha = u32::from(bgra[3]);
                    let straight = |c: u8| match alpha {
                        0 => 0,
                        a => (u32::from(c) * 255 / a).min(255) as u8,
                    };
                    [
                        straight(bgra[2]),
                        straight(bgra[1]),
                        straight(bgra[0]),
                        bgra[3],
                    ]
                })
                .collect();
            let (width, height) = (image.size.width as u16, image.size.height as u16);
            let hotspot = |v: i32, max: u16| v.clamp(0, i32::from(max.saturating_sub(1))) as u16;
            match CustomCursor::from_rgba(
                rgba,
                width,
                height,
                hotspot(image.hotspot.x, width),
                hotspot(image.hotspot.y, height),
            ) {
                Ok(source) => {
                    let cursor = self.event_loop.create_custom_cursor(source);
                    open.window.set_cursor(Cursor::Custom(cursor));
                }
                Err(error) => tracing::debug!(%error, "the app's cursor picture was refused"),
            }
        }
    }

    /// Starts the OS drag.
    fn drag(&mut self, local: LocalId, drag: Drag) {
        let Some(open) = self.locals.open.get(&local) else {
            return;
        };
        let started = match drag {
            Drag::Move => open.window.drag_window(),
            Drag::Resize(edge) => open.window.drag_resize_window(direction(edge)),
        };
        if let Err(error) = started {
            tracing::debug!(%error, "the OS did not start a drag");
        }
    }

    /// Brings the window forward.
    fn focus(&mut self, local: LocalId) {
        if let Some(open) = self.locals.open.get(&local) {
            open.window.focus_window();
        }
    }

    /// Writes the local clipboard.
    fn set_clipboard(&mut self, text: &str) {
        if let Some(clipboard) = self.clipboard.as_mut()
            && let Err(error) = clipboard.set_text(text)
        {
            tracing::debug!(%error, "the clipboard was not written");
        }
    }

    /// Ends the event loop.
    fn exit(&mut self) {
        self.event_loop.exit();
    }
}
