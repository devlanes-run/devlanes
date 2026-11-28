//! Text another machine controls, made safe to show.
//!
//! A box (or anyone who wrote a ticket) chooses the reason bytes of a QUIC
//! close and the relay URL in a `PeerAddr`, and noq writes a close reason
//! with `String::from_utf8_lossy`. The daemon shows link failures in a
//! terminal, so such text never reaches a message or a log line as it came.

/// Longest cleaned text, in bytes: enough for a useful reason, too short to
/// fill a terminal with someone else's text.
const MAX_LEN: usize = 200;

/// `text` with control characters and Unicode format characters (category
/// Cf, which holds the bidirectional overrides and zero-width characters)
/// removed, every run of whitespace turned into one space, and cut to about
/// [`MAX_LEN`] bytes on a character boundary, with `...` marking the cut.
/// ANSI escape sequences lose their ESC, so what is left is plain text.
pub(crate) fn peer_text(text: &str) -> String {
    let words = text
        .split(char::is_whitespace)
        .map(|word| {
            word.chars()
                .filter(|c| !c.is_control() && !is_format(*c))
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if words.len() <= MAX_LEN {
        return words;
    }
    let mut end = MAX_LEN;
    while !words.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &words[..end])
}

/// Whether `c` is in Unicode's general category Cf (format), which the
/// standard library does not expose. The list follows Unicode 16.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a box puts in a close reason cannot reach a terminal as escape
    /// sequences, reordered text or a wall of text, and plain text passes
    /// unchanged.
    #[test]
    fn peer_text_keeps_only_plain_bounded_text() {
        let long = "é".repeat(300);
        let cut = format!("{}...", "é".repeat(MAX_LEN / 2));
        let cases: [(&str, &str); 6] = [
            (
                "closed by peer: 7: shutting down",
                "closed by peer: 7: shutting down",
            ),
            ("\u{1b}[31mred\u{1b}[0m text", "[31mred[0m text"),
            ("pay \u{202E}gnp.exe", "pay gnp.exe"),
            ("zero\u{200B}width\u{FEFF}", "zerowidth"),
            ("many\n\n  lines\r\n\tand\u{7}bell", "many lines andbell"),
            (&long, &cut),
        ];
        for (raw, shown) in cases {
            assert_eq!(peer_text(raw), shown, "{raw:?}");
        }
    }
}
