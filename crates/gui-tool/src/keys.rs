//! Key names and chords as `devlanes gui key` takes them, mapped to the
//! protocol's USB HID key codes.
//!
//! A chord is names joined by `+`, such as `enter`, `ctrl+s` or
//! `ctrl+shift+t`. Names are case-insensitive. Every name before the last
//! must be a modifier; the last may be any key, a modifier included (`shift`
//! alone). The keys go down in the order written and up in reverse. A name
//! means a position on a US layout, as HID codes do: `a` is the key labelled
//! A there, wherever the target's layout puts its letter.

use devlanes_gui_proto::KeyCode;

/// A modifier by its names, as the left-hand key.
const MODIFIERS: &[(&[&str], u16)] = &[
    (&["ctrl", "control"], 0xe0),
    (&["shift"], 0xe1),
    (&["alt", "option"], 0xe2),
    (&["meta", "super", "cmd", "win"], 0xe3),
];

/// Every other named key with its HID usage on the keyboard page. Letters,
/// digits and `f1` to `f24` are computed in [`usage`].
const NAMED: &[(&[&str], u16)] = &[
    (&["enter", "return"], 0x28),
    (&["escape", "esc"], 0x29),
    (&["backspace"], 0x2a),
    (&["tab"], 0x2b),
    (&["space"], 0x2c),
    (&["minus"], 0x2d),
    (&["equal"], 0x2e),
    (&["bracketleft"], 0x2f),
    (&["bracketright"], 0x30),
    (&["backslash"], 0x31),
    (&["semicolon"], 0x33),
    (&["quote"], 0x34),
    (&["backquote"], 0x35),
    (&["comma"], 0x36),
    (&["period"], 0x37),
    (&["slash"], 0x38),
    (&["capslock"], 0x39),
    (&["printscreen"], 0x46),
    (&["scrolllock"], 0x47),
    (&["pause"], 0x48),
    (&["insert"], 0x49),
    (&["home"], 0x4a),
    (&["pageup"], 0x4b),
    (&["delete"], 0x4c),
    (&["end"], 0x4d),
    (&["pagedown"], 0x4e),
    (&["right"], 0x4f),
    (&["left"], 0x50),
    (&["down"], 0x51),
    (&["up"], 0x52),
    (&["menu"], 0x65),
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
    let find = |table: &[(&[&str], u16)]| {
        table
            .iter()
            .find(|(names, _)| names.contains(&name))
            .map(|(_, usage)| *usage)
    };
    if let Some(usage) = find(MODIFIERS) {
        return Some((KeyCode::keyboard(usage), true));
    }
    let usage = match name.as_bytes() {
        [letter @ b'a'..=b'z'] => 0x04 + u16::from(letter - b'a'),
        [b'0'] => 0x27,
        [digit @ b'1'..=b'9'] => 0x1e + u16::from(digit - b'1'),
        [b'f', rest @ ..] if !rest.is_empty() && rest[0] != b'0' => {
            let number: u16 = std::str::from_utf8(rest).ok()?.parse().ok()?;
            match number {
                1..=12 => 0x3a + number - 1,
                13..=24 => 0x68 + number - 13,
                _ => return None,
            }
        }
        _ => find(NAMED)?,
    };
    Some((KeyCode::keyboard(usage), false))
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
            ("enter", &[0x28]),
            ("esc", &[0x29]),
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
            ("up", &[0x52]),
        ];
        for (chord, usages) in cases {
            let parsed = parse_chord(chord).unwrap_or_else(|error| panic!("{chord}: {error}"));
            let expected: Vec<KeyCode> = usages.iter().map(|usage| key(*usage)).collect();
            assert_eq!(parsed.keys, expected, "{chord}");
        }
    }

    /// Unknown names, keys out of place and doubled keys are refused.
    #[test]
    fn bad_chords_are_refused() {
        for (chord, unknown) in [
            ("ctrl+", true),
            ("", true),
            ("f0", true),
            ("f25", true),
            ("f01", true),
            ("hyper+a", true),
            ("plus", true),
            ("s+ctrl", false),
            ("a+b", false),
            ("ctrl+ctrl+s", false),
            ("shift+shift", false),
        ] {
            let error = parse_chord(chord).expect_err(chord);
            assert_eq!(
                matches!(error, KeyError::Unknown { .. }),
                unknown,
                "{chord}: {error}"
            );
        }
    }
}
