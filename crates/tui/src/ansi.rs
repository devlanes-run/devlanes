//! A ratatui [`Buffer`] as text with SGR colors, for what devlanes prints
//! inline in the shell instead of on the TUI's screen: the exit summary and
//! the step block of `daemon stop` and `daemon restart`.

use crossterm::{
    Command,
    style::{Attribute, SetAttribute, SetForegroundColor},
};
use ratatui::{
    backend::IntoCrossterm,
    buffer::{Buffer, Cell},
    style::{Color, Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

/// `buf` as text, one line per row with trailing blanks cut, its colors and
/// bold or dim as SGR sequences. A row that switched colors ends with a reset.
pub(crate) fn text(buf: &Buffer) -> String {
    let plain = Cell::EMPTY.style();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let row: Vec<&Cell> = (0..buf.area.width)
            .map(|x| &buf[(buf.area.x + x, buf.area.y + y)])
            .collect();
        let end = row
            .iter()
            .rposition(|cell| cell.symbol() != " ")
            .map_or(0, |last| last + 1);
        let mut current = plain;
        // A wide character covers the cells after it, which hold a blank.
        let mut covered = 0;
        for cell in &row[..end] {
            if covered > 0 {
                covered -= 1;
                continue;
            }
            let style = cell.style();
            if style != current {
                sgr(&mut out, style);
                current = style;
            }
            out.push_str(cell.symbol());
            covered = cell.symbol().width().saturating_sub(1);
        }
        if current != plain {
            let _ = SetAttribute(Attribute::Reset).write_ansi(&mut out);
        }
        out.push('\n');
    }
    out
}

/// Writes the sequences that switch to `style` from whatever came before:
/// a reset, then its color and modifiers. Inline output uses only these.
fn sgr(out: &mut String, style: Style) {
    let _ = SetAttribute(Attribute::Reset).write_ansi(out);
    if let Some(color) = style.fg.filter(|color| *color != Color::Reset) {
        let _ = SetForegroundColor(color.into_crossterm()).write_ansi(out);
    }
    for (modifier, attribute) in [
        (Modifier::BOLD, Attribute::Bold),
        (Modifier::DIM, Attribute::Dim),
    ] {
        if style.add_modifier.contains(modifier) {
            let _ = SetAttribute(attribute).write_ansi(out);
        }
    }
}
