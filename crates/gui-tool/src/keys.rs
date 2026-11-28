//! Key names and chords as `devlanes gui key` takes them, mapped to the
//! protocol's USB HID key codes.
//!
//! A chord is names joined by `+`, such as `enter`, `ctrl+s` or
//! `ctrl+shift+t`. Names are case-insensitive. Every name before the last
//! must be a modifier; the last may be any key, a modifier included (`shift`
//! alone). The keys go down in the order written and up in reverse. A name
//! means a position on a US layout, as HID codes do: `a` is the key labelled
//! A there, wherever the target's layout puts its letter.

use devlanes_gui_proto::{KeyCode, keys};

/// A modifier by its names, as the left-hand key.
const MODIFIERS: &[(&[&str], KeyCode)] = &[
    (&["ctrl", "control"], keys::CONTROL_LEFT),
    (&["shift"], keys::SHIFT_LEFT),
    (&["alt", "option"], keys::ALT_LEFT),
    (&["meta", "super", "cmd", "win"], keys::META_LEFT),
];

/// Every other named key. Letters, digits and `f1` to `f24` are computed in
/// [`usage`].
const NAMED: &[(&[&str], KeyCode)] = &[
    (&["enter", "return"], keys::ENTER),
    (&["escape", "esc"], keys::ESCAPE),
    (&["backspace"], keys::BACKSPACE),
    (&["tab"], keys::TAB),
    (&["space"], keys::SPACE),
    (&["minus"], keys::MINUS),
    (&["equal"], keys::EQUAL),
    (&["bracketleft"], keys::BRACKET_LEFT),
    (&["bracketright"], keys::BRACKET_RIGHT),
    (&["backslash"], keys::BACKSLASH),
    (&["semicolon"], keys::SEMICOLON),
    (&["quote"], keys::QUOTE),
    (&["backquote"], keys::BACKQUOTE),
    (&["comma"], keys::COMMA),
    (&["period"], keys::PERIOD),
    (&["slash"], keys::SLASH),
    (&["capslock"], keys::CAPS_LOCK),
    (&["printscreen"], keys::PRINT_SCREEN),
    (&["scrolllock"], keys::SCROLL_LOCK),
    (&["pause"], keys::PAUSE),
    (&["insert"], keys::INSERT),
    (&["home"], keys::HOME),
    (&["pageup"], keys::PAGE_UP),
    (&["delete"], keys::DELETE),
    (&["end"], keys::END),
    (&["pagedown"], keys::PAGE_DOWN),
    (&["right"], keys::RIGHT),
    (&["left"], keys::LEFT),
    (&["down"], keys::DOWN),
    (&["up"], keys::UP),
    (&["menu"], keys::CONTEXT_MENU),
];

/// One chord: the keys in the order they go down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    /// The chord as written, lowercased, for output.
    pub name: String,
    /// The keys, modifiers first.
    pub keys: Vec<KeyCode>,
}

/// Why a chord is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// An empty name, as in `ctrl+` or `ctrl++`.
    #[error("{chord:?} has an empty key name")]
    Empty {
        /// The chord as written.
        chord: String,
    },
    /// A name that is no key.
    #[error("unknown key {name:?} in {chord:?}")]
    Unknown {
        /// The chord as written.
        chord: String,
        /// The name that is no key.
        name: String,
    },
    /// A key before the last that is not a modifier, or a key named twice.
    #[error("{chord:?} is not a chord: only modifiers can come before the last key, once each")]
    NotAChord {
        /// The chord as written.
        chord: String,
    },
}

/// Parses one chord, such as `ctrl+s`.
pub fn parse_chord(chord: &str) -> Result<Chord, KeyError> {
    let lower = chord.trim().to_ascii_lowercase();
    let names: Vec<&str> = lower.split('+').collect();
    let mut keys = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        if name.is_empty() {
            return Err(KeyError::Empty {
                chord: chord.to_owned(),
            });
        }
        let unknown = || KeyError::Unknown {
            chord: chord.to_owned(),
            name: (*name).to_owned(),
        };
        let (code, modifier) = usage(name).ok_or_else(unknown)?;
        let last = index + 1 == names.len();
        if (!last && !modifier) || keys.contains(&code) {
            return Err(KeyError::NotAChord {
                chord: chord.to_owned(),
            });
        }
        keys.push(code);
    }
    Ok(Chord { name: lower, keys })
}

/// The key code of `name` and whether it is a modifier.
fn usage(name: &str) -> Option<(KeyCode, bool)> {
    let find = |table: &[(&[&str], KeyCode)]| {
        table
            .iter()
            .find(|(names, _)| names.contains(&name))
            .map(|(_, code)| *code)
    };
    if let Some(code) = find(MODIFIERS) {
        return Some((code, true));
    }
    let code = match name.as_bytes() {
        [letter @ b'a'..=b'z'] => keys::letter(*letter)?,
        [digit @ b'0'..=b'9'] => keys::digit(*digit)?,
        [b'f', rest @ ..] if !rest.is_empty() && rest[0] != b'0' => {
            keys::function(std::str::from_utf8(rest).ok()?.parse().ok()?)?
        }
        _ => find(NAMED)?,
    };
    Some((code, false))
}

/// Every key name, for the error that lists them: modifiers, then the
/// letters, digits and function keys as ranges, then the named keys.
pub fn known_names() -> Vec<String> {
    MODIFIERS
        .iter()
        .flat_map(|(names, _)| names.iter())
        .map(|name| (*name).to_owned())
        .chain(["a-z".to_owned(), "0-9".to_owned(), "f1-f24".to_owned()])
        .chain(
            NAMED
                .iter()
                .flat_map(|(names, _)| names.iter())
                .map(|name| (*name).to_owned()),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keyboard-page code of `usage`.
    fn key(usage: u16) -> KeyCode {
        KeyCode::keyboard(usage)
    }

    /// Chords map to the HID usages of the USB HID Usage Tables (keyboard
    /// page 0x07), in press order, whatever the case.
    #[test]
    fn chords_map_to_hid_usages() {
        let cases: &[(&str, &[u16])] = &[
            ("a", &[0x04]),
            ("Z", &[0x1d]),
            ("1", &[0x1e]),
            ("0", &[0x27]),
            ("f1", &[0x3a]),
            ("f12", &[0x45]),
            ("f13", &[0x68]),
            ("f24", &[0x73]),
            ("ctrl+s", &[0xe0, 0x16]),
            ("Ctrl+Shift+T", &[0xe0, 0xe1, 0x17]),
            ("cmd+q", &[0xe3, 0x14]),
            ("alt+f4", &[0xe2, 0x3d]),
            ("shift", &[0xe1]),
            ("ctrl+shift", &[0xe0, 0xe1]),
        ];
        for (chord, usages) in cases {
            let parsed = parse_chord(chord).unwrap_or_else(|error| panic!("{chord}: {error}"));
            let expected: Vec<KeyCode> = usages.iter().map(|usage| key(*usage)).collect();
            assert_eq!(parsed.keys, expected, "{chord}");
        }
    }

    /// Empty and unknown names, keys out of place and doubled keys are refused.
    #[test]
    fn bad_chords_are_refused() {
        for (chord, kind) in [
            ("ctrl+", "empty"),
            ("ctrl++", "empty"),
            ("", "empty"),
            ("f0", "unknown"),
            ("f25", "unknown"),
            ("f01", "unknown"),
            ("hyper+a", "unknown"),
            ("plus", "unknown"),
            ("s+ctrl", "chord"),
            ("a+b", "chord"),
            ("ctrl+ctrl+s", "chord"),
            ("shift+shift", "chord"),
        ] {
            let error = parse_chord(chord).expect_err(chord);
            let got = match &error {
                KeyError::Empty { .. } => "empty",
                KeyError::Unknown { .. } => "unknown",
                KeyError::NotAChord { .. } => "chord",
            };
            assert_eq!(got, kind, "{chord}: {error}");
        }
    }
}
