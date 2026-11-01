//! Browser keys (`KeyboardEvent.key` and its modifier flags) as the key
//! events the TUI takes from crossterm in a terminal.

use devlanes_tui::keys::{KeyCode, KeyEvent, KeyModifiers};

/// Ctrl chords the demo passes to the TUI: copy and interrupt (`c`), the
/// page keys (`d`, `u`) and the end of typing (`]`). Other Ctrl chords,
/// Ctrl+Shift ones included, stay with the browser, so reload, find, new
/// tab and the inspector keep working on the page.
const CTRL_KEYS: &[char] = &['c', 'd', 'u', ']'];

/// The TUI's key event for a browser key, or `None` when the page should
/// keep it: modifier keys alone, Tab (which moves focus out of the demo,
/// so the keyboard never gets stuck in it), Alt chords other than typed
/// symbols, and Ctrl chords outside `CTRL_KEYS`.
pub fn key_event(key: &str, ctrl: bool, alt: bool, shift: bool) -> Option<KeyEvent> {
    if alt {
        // AltGr (Ctrl+Alt on Windows) and Option on macOS type symbols such
        // as `[` or `@` on many layouts; like the native TUI's `altgr`, a
        // printable symbol is text, and any other Alt chord is the page's.
        let c = single_char(key)?;
        let typed = !c.is_alphanumeric() && !c.is_control() && c != ' ';
        return typed.then(|| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let code = match key {
        "Enter" => KeyCode::Enter,
        "Escape" | "Esc" => KeyCode::Esc,
        "Backspace" => KeyCode::Backspace,
        "Delete" | "Del" => KeyCode::Delete,
        "Insert" => KeyCode::Insert,
        "ArrowUp" | "Up" => KeyCode::Up,
        "ArrowDown" | "Down" => KeyCode::Down,
        "ArrowLeft" | "Left" => KeyCode::Left,
        "ArrowRight" | "Right" => KeyCode::Right,
        "Home" => KeyCode::Home,
        "End" => KeyCode::End,
        "PageUp" => KeyCode::PageUp,
        "PageDown" => KeyCode::PageDown,
        "Spacebar" => KeyCode::Char(' '),
        _ => match function_key(key) {
            Some(number) => KeyCode::F(number),
            None => KeyCode::Char(single_char(key)?),
        },
    };
    let mut modifiers = KeyModifiers::NONE;
    if ctrl {
        let KeyCode::Char(c) = code else {
            return None;
        };
        if !CTRL_KEYS.contains(&c) {
            return None;
        }
        modifiers |= KeyModifiers::CONTROL;
    }
    // crossterm reports Shift with the shifted character itself (`M`, `?`),
    // and the TUI matches those characters, so the flag only rides along.
    if shift {
        modifiers |= KeyModifiers::SHIFT;
    }
    Some(KeyEvent::new(code, modifiers))
}

/// `F1` to `F12` as their number.
fn function_key(key: &str) -> Option<u8> {
    let number: u8 = key.strip_prefix('F')?.parse().ok()?;
    (1..=12).contains(&number).then_some(number)
}

/// The character of a key that types exactly one (`m`, `M`, `?`, ` `);
/// named keys such as `Shift`, `Tab` or `CapsLock` have none.
fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The event a key maps to, for assertions.
    fn event(code: KeyCode, modifiers: KeyModifiers) -> Option<KeyEvent> {
        Some(KeyEvent::new(code, modifiers))
    }

    #[test]
    fn letters_and_symbols_are_characters() {
        assert_eq!(
            key_event("j", false, false, false),
            event(KeyCode::Char('j'), KeyModifiers::NONE)
        );
        assert_eq!(
            key_event("M", false, false, true),
            event(KeyCode::Char('M'), KeyModifiers::SHIFT)
        );
        assert_eq!(
            key_event("?", false, false, true),
            event(KeyCode::Char('?'), KeyModifiers::SHIFT)
        );
        assert_eq!(
            key_event(" ", false, false, false),
            event(KeyCode::Char(' '), KeyModifiers::NONE)
        );
    }

    #[test]
    fn named_keys_map_to_their_codes() {
        for (key, code) in [
            ("Enter", KeyCode::Enter),
            ("Escape", KeyCode::Esc),
            ("ArrowDown", KeyCode::Down),
            ("PageUp", KeyCode::PageUp),
            ("F5", KeyCode::F(5)),
        ] {
            assert_eq!(
                key_event(key, false, false, false),
                event(code, KeyModifiers::NONE)
            );
        }
    }

    #[test]
    fn the_page_keeps_tab_modifiers_and_unknown_keys() {
        for key in ["Tab", "Shift", "Control", "Meta", "CapsLock", "F13", "Dead"] {
            assert_eq!(key_event(key, false, false, false), None, "{key}");
        }
        assert_eq!(key_event("Tab", false, false, true), None);
    }

    #[test]
    fn only_listed_ctrl_chords_reach_the_tui() {
        assert_eq!(
            key_event("c", true, false, false),
            event(KeyCode::Char('c'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            key_event("]", true, false, false),
            event(KeyCode::Char(']'), KeyModifiers::CONTROL)
        );
        assert_eq!(key_event("r", true, false, false), None);
        assert_eq!(key_event("C", true, false, true), None);
        assert_eq!(key_event("Enter", true, false, false), None);
    }

    #[test]
    fn altgr_and_option_symbols_are_text_and_other_alt_chords_are_not() {
        assert_eq!(
            key_event("[", true, true, false),
            event(KeyCode::Char('['), KeyModifiers::NONE)
        );
        assert_eq!(
            key_event("@", false, true, false),
            event(KeyCode::Char('@'), KeyModifiers::NONE)
        );
        assert_eq!(key_event("j", false, true, false), None);
        assert_eq!(key_event("ø", false, true, false), None);
        assert_eq!(key_event("ArrowLeft", false, true, false), None);
    }
}
