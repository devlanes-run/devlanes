//! The colors a terminal would give the TUI's named colors, for the browser.
//!
//! The TUI leaves the 16 ANSI colors and the default colors to the
//! terminal's own theme. Ratzilla turns the names into the dim VGA values
//! (blue is `#000080`), unreadable on the website's dark terminal, the
//! indexed ones into xterm's, and the default foreground into white, so the
//! demo picks its palette before drawing.

use ratatui::{
    buffer::Buffer,
    style::{Color, Modifier},
};

/// The website's terminal background (`--terminal`).
pub const BACKGROUND: Color = Color::Rgb(0x1a, 0x19, 0x17);
/// The website's terminal text (`--terminal-ink`).
pub const FOREGROUND: Color = Color::Rgb(0xe8, 0xe6, 0xdf);

/// The ANSI names in index order, so `Indexed(0..16)` (from the units'
/// screens, through vt100) matches the names the TUI's own text uses.
const ANSI: [Color; 16] = [
    Color::Black,
    Color::Red,
    Color::Green,
    Color::Yellow,
    Color::Blue,
    Color::Magenta,
    Color::Cyan,
    Color::Gray,
    Color::DarkGray,
    Color::LightRed,
    Color::LightGreen,
    Color::LightYellow,
    Color::LightBlue,
    Color::LightMagenta,
    Color::LightCyan,
    Color::White,
];

/// `color` as the demo's terminal shows it; true colors and the indexed
/// ones past the first 16 stay.
fn rgb(color: Color) -> Color {
    let color = match color {
        Color::Indexed(index) => ANSI.get(usize::from(index)).copied().unwrap_or(color),
        other => other,
    };
    let (r, g, b) = match color {
        Color::Black => (0x48, 0x4f, 0x58),
        Color::Red => (0xff, 0x7b, 0x72),
        Color::Green => (0x3f, 0xb9, 0x50),
        Color::Yellow => (0xd2, 0x99, 0x22),
        Color::Blue => (0x58, 0xa6, 0xff),
        Color::Magenta => (0xbc, 0x8c, 0xff),
        Color::Cyan => (0x39, 0xc5, 0xcf),
        Color::Gray => (0xb1, 0xba, 0xc4),
        Color::DarkGray => (0x6e, 0x76, 0x81),
        Color::LightRed => (0xff, 0xa1, 0x98),
        Color::LightGreen => (0x56, 0xd3, 0x64),
        Color::LightYellow => (0xe3, 0xb3, 0x41),
        Color::LightBlue => (0x79, 0xc0, 0xff),
        Color::LightMagenta => (0xd2, 0xa8, 0xff),
        Color::LightCyan => (0x56, 0xd4, 0xdd),
        Color::White => (0xff, 0xff, 0xff),
        other => return other,
    };
    Color::Rgb(r, g, b)
}

/// Gives every cell of `buffer` the demo's palette. The default foreground
/// becomes the site's terminal text; the default background stays default
/// (the page's terminal shows through), except on reverse video, whose
/// swapped colors need a real one.
pub fn apply(buffer: &mut Buffer) {
    for cell in &mut buffer.content {
        cell.fg = match rgb(cell.fg) {
            Color::Reset => FOREGROUND,
            color => color,
        };
        cell.bg = rgb(cell.bg);
        if cell.modifier.contains(Modifier::REVERSED) && cell.bg == Color::Reset {
            cell.bg = BACKGROUND;
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{layout::Rect, style::Style};

    use super::*;

    #[test]
    fn names_and_low_indexes_get_the_palette_and_true_colors_stay() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 5, 1));
        buffer.set_string(0, 0, "a", Style::new().fg(Color::Blue));
        buffer.set_string(1, 0, "b", Style::new().fg(Color::Indexed(4)));
        buffer.set_string(2, 0, "c", Style::new().fg(Color::Indexed(208)));
        buffer.set_string(3, 0, "d", Style::new().fg(Color::Rgb(1, 2, 3)));
        buffer.set_string(4, 0, "e", Style::new());
        apply(&mut buffer);
        let blue = Color::Rgb(0x58, 0xa6, 0xff);
        assert_eq!(buffer.content[0].fg, blue);
        assert_eq!(buffer.content[1].fg, blue);
        assert_eq!(buffer.content[2].fg, Color::Indexed(208));
        assert_eq!(buffer.content[3].fg, Color::Rgb(1, 2, 3));
        assert_eq!(buffer.content[4].fg, FOREGROUND);
        assert_eq!(buffer.content[4].bg, Color::Reset);
    }

    #[test]
    fn reverse_video_gets_real_colors_to_swap() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer.set_string(0, 0, "x", Style::new().add_modifier(Modifier::REVERSED));
        apply(&mut buffer);
        assert_eq!(buffer.content[0].fg, FOREGROUND);
        assert_eq!(buffer.content[0].bg, BACKGROUND);
    }
}
