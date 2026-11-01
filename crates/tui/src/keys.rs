//! The key and mouse events the app reacts to.
//!
//! Natively these are crossterm's own types, so the terminal's events reach
//! the app unchanged. crossterm does not build for the browser, so the web
//! demo (crates/tui-web) gets copies with the same names, fields and values,
//! covering what the demo's key mapping produces and the app matches on.

#[cfg(not(target_family = "wasm"))]
pub use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};

#[cfg(target_family = "wasm")]
pub use browser::*;

#[cfg(target_family = "wasm")]
mod browser {
    bitflags::bitflags! {
        /// Modifier keys held with a key or mouse event.
        #[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
        pub struct KeyModifiers: u8 {
            const SHIFT = 0b0000_0001;
            const CONTROL = 0b0000_0010;
            const ALT = 0b0000_0100;
            const SUPER = 0b0000_1000;
            const HYPER = 0b0001_0000;
            const META = 0b0010_0000;
            const NONE = 0b0000_0000;
        }
    }

    bitflags::bitflags! {
        /// Lock and keypad state of a key event.
        #[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
        pub struct KeyEventState: u8 {
            const KEYPAD = 0b0000_0001;
            const CAPS_LOCK = 0b0000_0010;
            const NUM_LOCK = 0b0000_0100;
            const NONE = 0b0000_0000;
        }
    }

    /// Whether a key went down, repeats or came up.
    #[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
    pub enum KeyEventKind {
        Press,
        Repeat,
        Release,
    }

    /// A key, as crossterm names it.
    #[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
    pub enum KeyCode {
        Backspace,
        Enter,
        Left,
        Right,
        Up,
        Down,
        Home,
        End,
        PageUp,
        PageDown,
        Tab,
        BackTab,
        Delete,
        Insert,
        F(u8),
        Char(char),
        Null,
        Esc,
        CapsLock,
        ScrollLock,
        NumLock,
        PrintScreen,
        Pause,
        Menu,
        KeypadBegin,
    }

    /// One key event.
    #[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
    pub struct KeyEvent {
        pub code: KeyCode,
        pub modifiers: KeyModifiers,
        pub kind: KeyEventKind,
        pub state: KeyEventState,
    }

    impl KeyEvent {
        /// A press of `code` with `modifiers`, as crossterm's constructor makes it.
        pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
            Self {
                code,
                modifiers,
                kind: KeyEventKind::Press,
                state: KeyEventState::empty(),
            }
        }
    }

    /// A mouse button.
    #[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
    pub enum MouseButton {
        Left,
        Right,
        Middle,
    }

    /// What a mouse event did.
    #[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
    pub enum MouseEventKind {
        Down(MouseButton),
        Up(MouseButton),
        Drag(MouseButton),
        Moved,
        ScrollDown,
        ScrollUp,
        ScrollLeft,
        ScrollRight,
    }

    /// One mouse event at a cell of the screen.
    #[derive(Debug, PartialOrd, PartialEq, Eq, Clone, Copy, Hash)]
    pub struct MouseEvent {
        pub kind: MouseEventKind,
        pub column: u16,
        pub row: u16,
        pub modifiers: KeyModifiers,
    }
}
