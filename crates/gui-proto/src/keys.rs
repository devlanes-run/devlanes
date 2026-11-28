//! The physical keys clients send, as USB HID usages (HID Usage Tables 1.5,
//! keyboard page 0x07 and consumer page 0x0C). A key is named by what it
//! shows on a US layout, as HID codes are: [`A`] is the key labelled A there,
//! wherever another layout puts its letter. The viewer maps its OS's key
//! codes to these and `devlanes gui key` maps key names to them, so both send
//! the same codes for the same key.

use crate::KeyCode;

/// A key on the keyboard page.
const fn key(usage: u16) -> KeyCode {
    KeyCode::keyboard(usage)
}

/// A key on the consumer page.
const fn consumer(usage: u16) -> KeyCode {
    KeyCode::new(KeyCode::CONSUMER_PAGE, usage)
}

/// The letter key `a` to `z` (either case), or `None` for anything else.
pub const fn letter(letter: u8) -> Option<KeyCode> {
    match letter.to_ascii_lowercase() {
        lower @ b'a'..=b'z' => Some(key(0x04 + (lower - b'a') as u16)),
        _ => None,
    }
}

/// The digit key `0` to `9` of the main block, or `None` for anything else.
pub const fn digit(digit: u8) -> Option<KeyCode> {
    match digit {
        b'0' => Some(key(0x27)),
        b'1'..=b'9' => Some(key(0x1e + (digit - b'1') as u16)),
        _ => None,
    }
}

/// The function key F1 to F24, or `None` for any other number.
pub const fn function(number: u8) -> Option<KeyCode> {
    match number {
        1..=12 => Some(key(0x3a + (number - 1) as u16)),
        13..=24 => Some(key(0x68 + (number - 13) as u16)),
        _ => None,
    }
}

/// The digit key `0` to `9` of the numeric keypad, or `None` for anything else.
pub const fn keypad_digit(digit: u8) -> Option<KeyCode> {
    match digit {
        b'0' => Some(key(0x62)),
        b'1'..=b'9' => Some(key(0x59 + (digit - b'1') as u16)),
        _ => None,
    }
}

/// Enter (Return).
pub const ENTER: KeyCode = key(0x28);
/// Escape.
pub const ESCAPE: KeyCode = key(0x29);
/// Backspace (Delete on a Mac keyboard).
pub const BACKSPACE: KeyCode = key(0x2a);
/// Tab.
pub const TAB: KeyCode = key(0x2b);
/// The space bar.
pub const SPACE: KeyCode = key(0x2c);
/// `-` and `_`.
pub const MINUS: KeyCode = key(0x2d);
/// `=` and `+`.
pub const EQUAL: KeyCode = key(0x2e);
/// `[` and `{`.
pub const BRACKET_LEFT: KeyCode = key(0x2f);
/// `]` and `}`.
pub const BRACKET_RIGHT: KeyCode = key(0x30);
/// `\` and `|`.
pub const BACKSLASH: KeyCode = key(0x31);
/// The key next to Enter on ISO layouts (`#` and `~` on a UK one).
pub const NON_US_HASH: KeyCode = key(0x32);
/// `;` and `:`.
pub const SEMICOLON: KeyCode = key(0x33);
/// `'` and `"`.
pub const QUOTE: KeyCode = key(0x34);
/// `` ` `` and `~`.
pub const BACKQUOTE: KeyCode = key(0x35);
/// `,` and `<`.
pub const COMMA: KeyCode = key(0x36);
/// `.` and `>`.
pub const PERIOD: KeyCode = key(0x37);
/// `/` and `?`.
pub const SLASH: KeyCode = key(0x38);
/// Caps Lock.
pub const CAPS_LOCK: KeyCode = key(0x39);
/// Print Screen.
pub const PRINT_SCREEN: KeyCode = key(0x46);
/// Scroll Lock.
pub const SCROLL_LOCK: KeyCode = key(0x47);
/// Pause.
pub const PAUSE: KeyCode = key(0x48);
/// Insert.
pub const INSERT: KeyCode = key(0x49);
/// Home.
pub const HOME: KeyCode = key(0x4a);
/// Page Up.
pub const PAGE_UP: KeyCode = key(0x4b);
/// Delete (forward delete).
pub const DELETE: KeyCode = key(0x4c);
/// End.
pub const END: KeyCode = key(0x4d);
/// Page Down.
pub const PAGE_DOWN: KeyCode = key(0x4e);
/// Arrow right.
pub const RIGHT: KeyCode = key(0x4f);
/// Arrow left.
pub const LEFT: KeyCode = key(0x50);
/// Arrow down.
pub const DOWN: KeyCode = key(0x51);
/// Arrow up.
pub const UP: KeyCode = key(0x52);
/// Num Lock (Clear on a Mac keyboard).
pub const NUM_LOCK: KeyCode = key(0x53);
/// Keypad `/`.
pub const KEYPAD_DIVIDE: KeyCode = key(0x54);
/// Keypad `*`.
pub const KEYPAD_MULTIPLY: KeyCode = key(0x55);
/// Keypad `-`.
pub const KEYPAD_SUBTRACT: KeyCode = key(0x56);
/// Keypad `+`.
pub const KEYPAD_ADD: KeyCode = key(0x57);
/// Keypad Enter.
pub const KEYPAD_ENTER: KeyCode = key(0x58);
/// Keypad `.`.
pub const KEYPAD_DECIMAL: KeyCode = key(0x63);
/// The key next to left Shift on ISO layouts (`\` and `|` on a UK one).
pub const INTL_BACKSLASH: KeyCode = key(0x64);
/// The context menu key.
pub const CONTEXT_MENU: KeyCode = key(0x65);
/// Keypad `=`.
pub const KEYPAD_EQUAL: KeyCode = key(0x67);
/// The Ro key of Japanese layouts.
pub const INTL_RO: KeyCode = key(0x87);
/// Katakana/Hiragana of Japanese layouts.
pub const KANA_MODE: KeyCode = key(0x88);
/// The Yen key of Japanese layouts.
pub const INTL_YEN: KeyCode = key(0x89);
/// Henkan (convert) of Japanese layouts.
pub const CONVERT: KeyCode = key(0x8a);
/// Muhenkan (non-convert) of Japanese layouts.
pub const NON_CONVERT: KeyCode = key(0x8b);
/// Han/Yeong of Korean layouts.
pub const LANG1: KeyCode = key(0x90);
/// Hanja of Korean layouts.
pub const LANG2: KeyCode = key(0x91);
/// Left Control.
pub const CONTROL_LEFT: KeyCode = key(0xe0);
/// Left Shift.
pub const SHIFT_LEFT: KeyCode = key(0xe1);
/// Left Alt (Option).
pub const ALT_LEFT: KeyCode = key(0xe2);
/// Left Meta (Windows, Command).
pub const META_LEFT: KeyCode = key(0xe3);
/// Right Control.
pub const CONTROL_RIGHT: KeyCode = key(0xe4);
/// Right Shift.
pub const SHIFT_RIGHT: KeyCode = key(0xe5);
/// Right Alt (Option, AltGr).
pub const ALT_RIGHT: KeyCode = key(0xe6);
/// Right Meta (Windows, Command).
pub const META_RIGHT: KeyCode = key(0xe7);

/// Next track.
pub const MEDIA_NEXT: KeyCode = consumer(0xb5);
/// Previous track.
pub const MEDIA_PREVIOUS: KeyCode = consumer(0xb6);
/// Stop.
pub const MEDIA_STOP: KeyCode = consumer(0xb7);
/// Play or pause.
pub const MEDIA_PLAY_PAUSE: KeyCode = consumer(0xcd);
/// Mute.
pub const VOLUME_MUTE: KeyCode = consumer(0xe2);
/// Volume up.
pub const VOLUME_UP: KeyCode = consumer(0xe9);
/// Volume down.
pub const VOLUME_DOWN: KeyCode = consumer(0xea);

#[cfg(test)]
mod tests {
    use super::*;

    /// The computed ranges land on the usages of the HID Usage Tables.
    #[test]
    fn computed_keys_match_the_usage_tables() {
        assert_eq!(letter(b'a'), Some(KeyCode::keyboard(0x04)));
        assert_eq!(letter(b'Z'), Some(KeyCode::keyboard(0x1d)));
        assert_eq!(letter(b'1'), None);
        assert_eq!(digit(b'1'), Some(KeyCode::keyboard(0x1e)));
        assert_eq!(digit(b'9'), Some(KeyCode::keyboard(0x26)));
        assert_eq!(digit(b'0'), Some(KeyCode::keyboard(0x27)));
        assert_eq!(function(1), Some(KeyCode::keyboard(0x3a)));
        assert_eq!(function(12), Some(KeyCode::keyboard(0x45)));
        assert_eq!(function(13), Some(KeyCode::keyboard(0x68)));
        assert_eq!(function(24), Some(KeyCode::keyboard(0x73)));
        assert_eq!(function(0), None);
        assert_eq!(function(25), None);
        assert_eq!(keypad_digit(b'1'), Some(KeyCode::keyboard(0x59)));
        assert_eq!(keypad_digit(b'0'), Some(KeyCode::keyboard(0x62)));
    }
}
