//! Local input as the protocol carries it: winit's physical key codes as USB
//! HID usages, mouse buttons, and wheel deltas in the protocol's direction.

use devlanes_gui_proto::{Buttons, KeyCode, WheelUnit, keys};
use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::{Key, KeyCode as Physical, NamedKey};

/// The HID code of a physical key, or `None` for a key the protocol has no
/// code for (such as Fn, which no OS reports to apps either).
pub fn key_code(key: Physical) -> Option<KeyCode> {
    use Physical as P;
    let letter = |c: u8| keys::letter(c);
    let digit = |c: u8| keys::digit(c);
    let function = |n: u8| keys::function(n);
    let keypad = |c: u8| keys::keypad_digit(c);
    Some(match key {
        P::KeyA => letter(b'a')?,
        P::KeyB => letter(b'b')?,
        P::KeyC => letter(b'c')?,
        P::KeyD => letter(b'd')?,
        P::KeyE => letter(b'e')?,
        P::KeyF => letter(b'f')?,
        P::KeyG => letter(b'g')?,
        P::KeyH => letter(b'h')?,
        P::KeyI => letter(b'i')?,
        P::KeyJ => letter(b'j')?,
        P::KeyK => letter(b'k')?,
        P::KeyL => letter(b'l')?,
        P::KeyM => letter(b'm')?,
        P::KeyN => letter(b'n')?,
        P::KeyO => letter(b'o')?,
        P::KeyP => letter(b'p')?,
        P::KeyQ => letter(b'q')?,
        P::KeyR => letter(b'r')?,
        P::KeyS => letter(b's')?,
        P::KeyT => letter(b't')?,
        P::KeyU => letter(b'u')?,
        P::KeyV => letter(b'v')?,
        P::KeyW => letter(b'w')?,
        P::KeyX => letter(b'x')?,
        P::KeyY => letter(b'y')?,
        P::KeyZ => letter(b'z')?,
        P::Digit0 => digit(b'0')?,
        P::Digit1 => digit(b'1')?,
        P::Digit2 => digit(b'2')?,
        P::Digit3 => digit(b'3')?,
        P::Digit4 => digit(b'4')?,
        P::Digit5 => digit(b'5')?,
        P::Digit6 => digit(b'6')?,
        P::Digit7 => digit(b'7')?,
        P::Digit8 => digit(b'8')?,
        P::Digit9 => digit(b'9')?,
        P::F1 => function(1)?,
        P::F2 => function(2)?,
        P::F3 => function(3)?,
        P::F4 => function(4)?,
        P::F5 => function(5)?,
        P::F6 => function(6)?,
        P::F7 => function(7)?,
        P::F8 => function(8)?,
        P::F9 => function(9)?,
        P::F10 => function(10)?,
        P::F11 => function(11)?,
        P::F12 => function(12)?,
        P::F13 => function(13)?,
        P::F14 => function(14)?,
        P::F15 => function(15)?,
        P::F16 => function(16)?,
        P::F17 => function(17)?,
        P::F18 => function(18)?,
        P::F19 => function(19)?,
        P::F20 => function(20)?,
        P::F21 => function(21)?,
        P::F22 => function(22)?,
        P::F23 => function(23)?,
        P::F24 => function(24)?,
        P::Numpad0 => keypad(b'0')?,
        P::Numpad1 => keypad(b'1')?,
        P::Numpad2 => keypad(b'2')?,
        P::Numpad3 => keypad(b'3')?,
        P::Numpad4 => keypad(b'4')?,
        P::Numpad5 => keypad(b'5')?,
        P::Numpad6 => keypad(b'6')?,
        P::Numpad7 => keypad(b'7')?,
        P::Numpad8 => keypad(b'8')?,
        P::Numpad9 => keypad(b'9')?,
        P::Enter => keys::ENTER,
        P::Escape => keys::ESCAPE,
        P::Backspace => keys::BACKSPACE,
        P::Tab => keys::TAB,
        P::Space => keys::SPACE,
        P::Minus => keys::MINUS,
        P::Equal => keys::EQUAL,
        P::BracketLeft => keys::BRACKET_LEFT,
        P::BracketRight => keys::BRACKET_RIGHT,
        P::Backslash => keys::BACKSLASH,
        P::Semicolon => keys::SEMICOLON,
        P::Quote => keys::QUOTE,
        P::Backquote => keys::BACKQUOTE,
        P::Comma => keys::COMMA,
        P::Period => keys::PERIOD,
        P::Slash => keys::SLASH,
        P::CapsLock => keys::CAPS_LOCK,
        P::PrintScreen => keys::PRINT_SCREEN,
        P::ScrollLock => keys::SCROLL_LOCK,
        P::Pause => keys::PAUSE,
        P::Insert => keys::INSERT,
        P::Home => keys::HOME,
        P::PageUp => keys::PAGE_UP,
        P::Delete => keys::DELETE,
        P::End => keys::END,
        P::PageDown => keys::PAGE_DOWN,
        P::ArrowRight => keys::RIGHT,
        P::ArrowLeft => keys::LEFT,
        P::ArrowDown => keys::DOWN,
        P::ArrowUp => keys::UP,
        P::NumLock | P::NumpadClear => keys::NUM_LOCK,
        P::NumpadDivide => keys::KEYPAD_DIVIDE,
        P::NumpadMultiply | P::NumpadStar => keys::KEYPAD_MULTIPLY,
        P::NumpadSubtract => keys::KEYPAD_SUBTRACT,
        P::NumpadAdd => keys::KEYPAD_ADD,
        P::NumpadEnter => keys::KEYPAD_ENTER,
        P::NumpadDecimal => keys::KEYPAD_DECIMAL,
        P::NumpadEqual => keys::KEYPAD_EQUAL,
        P::IntlBackslash => keys::INTL_BACKSLASH,
        P::IntlRo => keys::INTL_RO,
        P::IntlYen => keys::INTL_YEN,
        P::KanaMode => keys::KANA_MODE,
        P::Convert => keys::CONVERT,
        P::NonConvert => keys::NON_CONVERT,
        P::Lang1 => keys::LANG1,
        P::Lang2 => keys::LANG2,
        P::ContextMenu => keys::CONTEXT_MENU,
        P::ControlLeft => keys::CONTROL_LEFT,
        P::ShiftLeft => keys::SHIFT_LEFT,
        P::AltLeft => keys::ALT_LEFT,
        P::SuperLeft => keys::META_LEFT,
        P::ControlRight => keys::CONTROL_RIGHT,
        P::ShiftRight => keys::SHIFT_RIGHT,
        P::AltRight => keys::ALT_RIGHT,
        P::SuperRight => keys::META_RIGHT,
        // Media and volume keys stay with this machine, which they control.
        _ => return None,
    })
}

/// For a keypad key, the navigation key its local meaning `logical` names,
/// such as Left for keypad 4 with Num Lock off; `None` for other keys and
/// for keypad keys that mean a character.
pub fn navigation(key: Physical, logical: &Key) -> Option<KeyCode> {
    use Physical as P;
    let keypad = matches!(
        key,
        P::Numpad0
            | P::Numpad1
            | P::Numpad2
            | P::Numpad3
            | P::Numpad4
            | P::Numpad5
            | P::Numpad6
            | P::Numpad7
            | P::Numpad8
            | P::Numpad9
            | P::NumpadDecimal
    );
    let Key::Named(named) = logical else {
        return None;
    };
    if !keypad {
        return None;
    }
    Some(match named {
        NamedKey::ArrowLeft => keys::LEFT,
        NamedKey::ArrowRight => keys::RIGHT,
        NamedKey::ArrowUp => keys::UP,
        NamedKey::ArrowDown => keys::DOWN,
        NamedKey::Home => keys::HOME,
        NamedKey::End => keys::END,
        NamedKey::PageUp => keys::PAGE_UP,
        NamedKey::PageDown => keys::PAGE_DOWN,
        NamedKey::Insert => keys::INSERT,
        NamedKey::Delete => keys::DELETE,
        _ => return None,
    })
}

/// Whether `key` is a modifier, whose press the OS reports again when a
/// window gains focus with it held, so a Control held while clicking into
/// the viewer reaches the app.
pub fn is_modifier(key: Physical) -> bool {
    use Physical as P;
    matches!(
        key,
        P::ControlLeft
            | P::ControlRight
            | P::ShiftLeft
            | P::ShiftRight
            | P::AltLeft
            | P::AltRight
            | P::SuperLeft
            | P::SuperRight
    )
}

/// The protocol's bit for a mouse button, or `None` for buttons beyond the
/// five it has.
pub fn button(button: MouseButton) -> Option<Buttons> {
    Some(match button {
        MouseButton::Left => Buttons::LEFT,
        MouseButton::Right => Buttons::RIGHT,
        MouseButton::Middle => Buttons::MIDDLE,
        MouseButton::Back => Buttons::BACK,
        MouseButton::Forward => Buttons::FORWARD,
        MouseButton::Other(_) => return None,
    })
}

/// A wheel delta as the protocol has it: winit's positive values reveal
/// content to the left and above, the protocol's scroll right and down, so
/// both axes flip. Precise (pixel) deltas stay pixels.
pub fn wheel(delta: MouseScrollDelta) -> Option<(f32, f32, WheelUnit)> {
    let (x, y, unit) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (x, y, WheelUnit::Lines),
        MouseScrollDelta::PixelDelta(position) => {
            (position.x as f32, position.y as f32, WheelUnit::Pixels)
        }
    };
    let (x, y) = (-x, -y);
    (x.is_finite() && y.is_finite() && (x != 0.0 || y != 0.0)).then_some((x, y, unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The viewer and `devlanes gui key` send the same code for the same key:
    /// winit names keys by their US-layout position, as the tool's key names
    /// do, and each key has a code of its own.
    #[test]
    fn physical_keys_send_the_codes_the_tool_sends_for_their_names() {
        use Physical as P;
        let pairs = [
            (P::KeyA, "a"),
            (P::KeyQ, "q"),
            (P::KeyZ, "z"),
            (P::Digit1, "1"),
            (P::Digit0, "0"),
            (P::F1, "f1"),
            (P::F12, "f12"),
            (P::F13, "f13"),
            (P::F24, "f24"),
            (P::Enter, "enter"),
            (P::Escape, "esc"),
            (P::Backquote, "backquote"),
            (P::BracketLeft, "bracketleft"),
            (P::Quote, "quote"),
            (P::ArrowUp, "up"),
            (P::PageDown, "pagedown"),
            (P::Delete, "delete"),
            (P::ContextMenu, "menu"),
            (P::ControlLeft, "ctrl"),
            (P::ShiftLeft, "shift"),
            (P::AltLeft, "alt"),
            (P::SuperLeft, "cmd"),
        ];
        for (key, name) in pairs {
            let tool = devlanes_gui_tool::parse_chord(name).expect(name).keys;
            assert_eq!(key_code(key).map(|code| vec![code]), Some(tool), "{name}");
        }
        let all = [
            P::KeyA,
            P::KeyZ,
            P::Digit1,
            P::Digit0,
            P::Numpad1,
            P::Numpad0,
            P::F1,
            P::F13,
            P::Enter,
            P::NumpadEnter,
            P::ControlLeft,
            P::ControlRight,
            P::IntlBackslash,
            P::Backslash,
            P::NumLock,
            P::Pause,
        ];
        let mut codes: Vec<KeyCode> = all.iter().filter_map(|key| key_code(*key)).collect();
        assert_eq!(codes.len(), all.len());
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), all.len(), "two keys share a code");
    }
}
