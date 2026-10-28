//! The screen model of a unit whose output renders its terminal (see
//! [`devlanes_core::Unit::renders_screen`]), such as a Windows pseudo console.
//!
//! Such output places text with absolute cursor moves for the terminal's
//! size, so replaying it at another size, as [`crate::screen::Screen`] does
//! for other units, scatters it. A redraw (see [`Rendered::frame`]) replays
//! the lines that left the grid plus the grid's own rows, joined where a row
//! wrapped, at the viewer's size instead.

use bytes::Bytes;
use devlanes_core::TerminalSize;

use crate::terminal::{Terminal, cells};

/// The grid of a rendered terminal plus the lines that left it.
pub(crate) struct Rendered {
    terminal: Terminal,
}

impl Rendered {
    /// An empty grid of `size` whose first logged line gets `first_seq`.
    pub(crate) fn new(size: TerminalSize, first_seq: u64) -> Self {
        Self {
            terminal: Terminal::new(size, first_seq, false),
        }
    }

    /// The terminal behind the model.
    pub(crate) fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    /// The terminal behind the model, to settle it and take its log.
    pub(crate) fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    /// Feeds output. A size marker (`ESC[8;<rows>;<columns>t`, see
    /// [`devlanes_core::Unit::renders_screen`]) resizes the grid where it
    /// stands, since what follows it is drawn at that size.
    pub(crate) fn process(&mut self, mut bytes: &[u8]) {
        while let Some((start, end, size)) = size_marker(bytes) {
            self.terminal.process(&bytes[..start]);
            self.resize(size);
            bytes = &bytes[end..];
        }
        self.terminal.process(bytes);
    }

    /// Resizes the grid the way the console host resizes its buffer, so its
    /// repaint that follows lands on the same rows (see [`Terminal::reflow`]).
    fn resize(&mut self, size: TerminalSize) {
        self.terminal.reflow(size);
    }

    /// The model as the console host drew it.
    pub(crate) fn screen(&self) -> &vt100::Screen {
        self.terminal.screen()
    }

    /// A whole redraw for a viewer of `size` (the grid's size when `None`).
    pub(crate) fn snapshot(&self, size: Option<TerminalSize>) -> Bytes {
        self.frame(size).render(None).0
    }

    /// What a redraw for a viewer of `size` (the grid's size when `None`)
    /// needs: a copy, so that the replay can run after the screen lock is
    /// released.
    pub(crate) fn frame(&self, size: Option<TerminalSize>) -> Frame {
        let screen = self.terminal.screen();
        let size = size.unwrap_or_else(|| self.terminal.size());
        let drawn = screen
            .alternate_screen()
            .then(|| screen.contents_formatted());
        let mut lines = Vec::new();
        if drawn.is_none() {
            // Each line takes a row at least, so this many fill the screen.
            let history: Vec<&str> = self
                .terminal
                .replayed_history()
                .map(|line| line.styled.as_str())
                .collect();
            for line in &history[history.len().saturating_sub(usize::from(size.rows))..] {
                lines.extend_from_slice(line.as_bytes());
                lines.extend_from_slice(b"\r\n");
            }
            lines.extend(self.terminal.grid_lines(size));
        }
        Frame {
            drawn,
            lines,
            size,
            hide_cursor: screen.hide_cursor(),
            input_modes: screen.input_mode_formatted(),
        }
    }
}

/// A rendered screen copied for one redraw (see [`Rendered::frame`]).
pub(crate) struct Frame {
    /// The alternate screen as the program drew it; `None` on the main screen.
    drawn: Option<String>,
    /// The history and the grid as text lines, replayed at `size`.
    lines: Vec<u8>,
    size: TerminalSize,
    hide_cursor: bool,
    input_modes: String,
}

impl Frame {
    /// The bytes that bring a viewer showing `shown` (what the last redraw
    /// left; a whole screen is drawn when `None` or of another size) to this
    /// frame, and the screen they leave it showing. Only the rows that differ
    /// are drawn again, so a viewer sees no flicker. CPU work; blocking.
    pub(crate) fn render(self, shown: Option<&vt100::Screen>) -> (Bytes, Option<vt100::Screen>) {
        let mut bytes = Vec::new();
        let screen = match self.drawn {
            Some(drawn) => {
                bytes.extend_from_slice(b"\x1b[H\x1b[2J");
                bytes.extend_from_slice(drawn.as_bytes());
                None
            }
            None => {
                let mut replay =
                    vt100::Parser::new(cells(self.size.rows), cells(self.size.columns), 0);
                replay.process(&self.lines);
                let screen = replay.screen();
                match shown.filter(|shown| shown.size() == screen.size()) {
                    Some(shown) => bytes.extend_from_slice(screen.contents_diff(shown).as_bytes()),
                    None => {
                        bytes.extend_from_slice(b"\x1b[H\x1b[2J");
                        bytes.extend_from_slice(screen.contents_formatted().as_bytes());
                    }
                }
                // The sequence that hid the cursor is not part of the lines.
                bytes.extend_from_slice(if self.hide_cursor {
                    b"\x1b[?25l"
                } else {
                    b"\x1b[?25h"
                });
                Some(screen.clone())
            }
        };
        bytes.extend_from_slice(self.input_modes.as_bytes());
        (Bytes::from(bytes), screen)
    }
}

/// The first `ESC[8;<rows>;<columns>t` in `bytes`: where it starts and ends,
/// and the size it names.
fn size_marker(bytes: &[u8]) -> Option<(usize, usize, TerminalSize)> {
    const START: &[u8] = b"\x1b[8;";
    let mut from = 0;
    while let Some(at) = bytes[from..].windows(START.len()).position(|w| w == START) {
        let start = from + at;
        let params = &bytes[start + START.len()..];
        let end = params
            .iter()
            .position(|&byte| !(byte.is_ascii_digit() || byte == b';'));
        if let Some(end) = end
            && params[end] == b't'
            && let Some((rows, columns)) = std::str::from_utf8(&params[..end])
                .ok()
                .and_then(|text| text.split_once(';'))
            && let (Ok(rows), Ok(columns)) = (rows.parse(), columns.parse())
        {
            let size = TerminalSize { columns, rows };
            return Some((start, start + START.len() + end + 1, size));
        }
        from = start + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bytes` drawn by a fresh terminal of `columns`x`rows`.
    fn draw(bytes: &[u8], columns: u16, rows: u16) -> vt100::Parser {
        let mut parser = vt100::Parser::new(cells(rows), cells(columns), 0);
        parser.process(bytes);
        parser
    }

    /// Line `i`: 50 columns, wider than the test grids.
    fn line(i: usize) -> String {
        format!("L{i:02} {}", "x".repeat(46))
    }

    /// Feeds `bytes` and settles, as the pump does per batch.
    fn feed(screen: &mut Rendered, bytes: &[u8]) {
        screen.process(bytes);
        screen.terminal_mut().settle();
    }

    /// A 40x8 grid fed lines 1 to 20 the way Windows' inbox console host sends
    /// them: the first screen as written, then, at the bottom, the first 40
    /// columns, a line break, and the rest written from the last column of the
    /// row above, so the terminal marks that row as wrapped.
    fn rendered() -> Rendered {
        let mut screen = Rendered::new(
            TerminalSize {
                columns: 40,
                rows: 8,
            },
            1,
        );
        for i in 1..=20 {
            let text = line(i);
            if i <= 3 {
                feed(&mut screen, format!("{text}\r\n").as_bytes());
            } else {
                let (head, _) = text.split_at(40);
                let rest = &text[39..];
                feed(
                    &mut screen,
                    format!("{head}\r\n\x1b[7;40H{rest}\r\n").as_bytes(),
                );
            }
        }
        screen
    }

    /// A redraw for a viewer of `size`, from scratch.
    fn redraw(screen: &Rendered, size: Option<TerminalSize>) -> Bytes {
        screen.snapshot(size)
    }

    /// The rows of a viewer of `columns`x`rows` after `bytes`, without
    /// trailing blanks.
    fn rows_after(bytes: &[u8], columns: u16, rows: u16) -> Vec<String> {
        draw(bytes, columns, rows)
            .screen()
            .rows(0, columns)
            .map(|row| row.trim_end().to_owned())
            .collect()
    }

    /// Whole lines at the viewer's size, not scattered by cursor moves meant
    /// for the console's size.
    #[test]
    fn a_rendered_screen_is_rewrapped_for_another_size() {
        let screen = rendered();
        let size = TerminalSize {
            columns: 60,
            rows: 10,
        };
        let shown = draw(&redraw(&screen, Some(size)), 60, 10);
        let rows: Vec<String> = shown.screen().rows(0, 60).collect();
        assert_eq!(rows[9], "");
        for (row, i) in rows[..9].iter().zip(12..) {
            assert_eq!(row, &line(i));
        }
    }

    /// After a resize the console host repaints its window from the top and
    /// leaves blank rows under the cursor; the redraw still fills the screen
    /// from the lines above, and the line cut by the window's top edge is
    /// joined to its start.
    #[test]
    fn a_repaint_after_a_resize_fills_the_screen() {
        let mut screen = rendered();
        let size = TerminalSize {
            columns: 60,
            rows: 8,
        };
        screen.resize(size);
        // The window kept its top row, the end of line 17.
        let mut repaint = format!("\x1b[?25l\x1b[H{}\x1b[K\r\n", &line(17)[40..]);
        for i in 18..=20 {
            repaint.push_str(&format!("{}\x1b[K\r\n", line(i)));
        }
        repaint.push_str("\x1b[K\r\n\x1b[K\r\n\x1b[K\r\n\x1b[K\x1b[5;1H\x1b[?25h");
        feed(&mut screen, repaint.as_bytes());
        let shown = draw(&redraw(&screen, None), 60, 8);
        let rows: Vec<String> = shown.screen().rows(0, 60).collect();
        assert_eq!(rows[7], "");
        for (row, i) in rows[..7].iter().zip(14..) {
            assert_eq!(row, &line(i));
        }
        assert_eq!(shown.screen().cursor_position(), (7, 0));
    }

    /// A viewer as wide as the grid keeps the cursor's column, so the cursor
    /// of a prompt stays after it; a viewer of another width keeps the space
    /// after the prompt.
    #[test]
    fn a_prompt_keeps_its_cursor() {
        let mut screen = Rendered::new(
            TerminalSize {
                columns: 40,
                rows: 4,
            },
            1,
        );
        // The console host sends a prompt's trailing space as a cursor move.
        feed(&mut screen, b"one\r\ntwo\r\n>\x1b[3;3H");
        let shown = draw(&redraw(&screen, None), 40, 4);
        assert_eq!(
            rows_after(&redraw(&screen, None), 40, 4),
            ["one", "two", ">", ""]
        );
        assert_eq!(shown.screen().cursor_position(), (2, 2));
        let size = TerminalSize {
            columns: 30,
            rows: 4,
        };
        let bytes = redraw(&screen, Some(size));
        assert_eq!(rows_after(&bytes, 30, 4), ["one", "two", ">", ""]);
        assert_eq!(draw(&bytes, 30, 4).screen().cursor_position(), (2, 2));
    }

    /// A redraw after another one changes only what differs, and leaves the
    /// viewer with the same screen as a whole redraw would.
    #[test]
    fn a_redraw_after_another_draws_the_difference() {
        let mut screen = rendered();
        let size = Some(TerminalSize {
            columns: 60,
            rows: 10,
        });
        let (first, shown) = screen.frame(size).render(None);
        feed(&mut screen, b"L21 new\r\n");
        let (diff, _) = screen.frame(size).render(shown.as_ref());
        let whole = redraw(&screen, size);
        assert!(diff.len() < whole.len() / 2, "{diff:?}");
        let mut viewer = first.to_vec();
        viewer.extend_from_slice(&diff);
        assert_eq!(rows_after(&viewer, 60, 10), rows_after(&whole, 60, 10));
        assert_eq!(rows_after(&viewer, 60, 10)[8], "L21 new");
    }

    /// A clear of the scrollback (`cls` in a pseudo console) drops the rows
    /// that left the grid before it from whole redraws, also when it arrives
    /// in two chunks; the console host's `ESC[2J` at startup does not.
    #[test]
    fn a_clear_of_the_scrollback_drops_the_history() {
        let size = TerminalSize {
            columns: 40,
            rows: 4,
        };
        let mut screen = Rendered::new(size, 1);
        feed(
            &mut screen,
            b"\x1b[?25l\x1b[2J\x1b[m\x1b[Hone\r\ntwo\r\nthree\r\nfour\r\nfive\r\n",
        );
        assert_eq!(rows_after(&redraw(&screen, Some(size)), 40, 4)[0], "three");
        let taller = TerminalSize {
            columns: 40,
            rows: 8,
        };
        assert_eq!(rows_after(&redraw(&screen, Some(taller)), 40, 8)[0], "one");
        feed(
            &mut screen,
            b"\x1b[H\x1b[K\r\n\x1b[K\r\n\x1b[K\r\n\x1b[K\x1b[H\x1b[",
        );
        feed(&mut screen, b"3Jsix\r\n");
        let rows = rows_after(&redraw(&screen, Some(taller)), 40, 8);
        assert_eq!(rows[..2], ["six", ""]);
        // What was cleared stays in the log, and the new line joins it.
        let log = screen.terminal_mut().take_log();
        assert_eq!(log, ["one", "two", "three", "four", "five", "six"]);
    }

    /// A taller grid gets its new rows at the bottom, as the console host's
    /// own buffer does, so its repaint lands on the same rows.
    #[test]
    fn a_taller_grid_grows_at_the_bottom() {
        let mut screen = rendered();
        let (row, _) = screen.screen().cursor_position();
        screen.resize(TerminalSize {
            columns: 40,
            rows: 12,
        });
        assert_eq!(screen.screen().cursor_position().0, row);
        // No blank rows between the history and the grid's rows.
        let rows = rows_after(&redraw(&screen, None), 40, 12);
        assert!(rows[..11].iter().all(|row| !row.is_empty()), "{rows:?}");
        assert_eq!(rows[11], "");
    }

    /// Line `i` of the reflow test: 180 columns, like a dev server's.
    fn wide(i: usize) -> String {
        format!("P{i:05} {}END", "abcdefghij".repeat(18)[..170].to_owned())
    }

    /// The console host's repaint after a resize, as the inbox one sends it:
    /// from the top, every line whole (the terminal wraps it), then the blank
    /// rows below, then the cursor.
    fn repaint(first: usize, last: usize, blank: usize, cursor_row: usize) -> String {
        let mut bytes = String::from("\x1b[?25l\x1b[H");
        for i in first..=last {
            bytes.push_str(&format!("{}\r\n", wide(i)));
        }
        bytes.push_str(&"\x1b[K\r\n".repeat(blank));
        bytes.push_str(&format!("\x1b[K\x1b[{cursor_row};1H\x1b[?25h"));
        bytes
    }

    /// Every service line once and in order, wrapped at `columns`, in the
    /// rows of a viewer of `columns`x`rows` after `bytes`.
    fn lines_in_order(bytes: &[u8], columns: u16, rows: u16) -> Vec<usize> {
        let shown = rows_after(bytes, columns, rows);
        let numbers: Vec<usize> = shown
            .iter()
            .filter(|row| row.starts_with('P'))
            .filter_map(|row| row.get(1..6)?.parse().ok())
            .collect();
        for pair in numbers.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "{shown:#?}");
        }
        numbers
    }

    /// Narrowing and widening the console again and again, as zooming a pane
    /// does: the console host re-wraps its lines and repaints from the top,
    /// and the lines it pushed out of its window stay in the history, so a
    /// redraw never skips or repeats one, and the log has each line once.
    #[test]
    fn resizes_rewrap_like_the_console_host() {
        let mut screen = Rendered::new(
            TerminalSize {
                columns: 200,
                rows: 45,
            },
            1,
        );
        for i in 1..=150 {
            feed(&mut screen, format!("{}\r\n", wide(i)).as_bytes());
        }
        let narrow = TerminalSize {
            columns: 152,
            rows: 45,
        };
        let wide_size = TerminalSize {
            columns: 200,
            rows: 45,
        };
        for _ in 0..3 {
            // The runner marks where each repaint at a new size begins.
            let marker = |size: TerminalSize| format!("\x1b[8;{};{}t", size.rows, size.columns);
            // 22 lines of two rows each, then the cursor's row.
            feed(
                &mut screen,
                format!("{}{}", marker(narrow), repaint(129, 150, 0, 45)).as_bytes(),
            );
            assert_eq!(screen.terminal().size(), narrow);
            let shown = lines_in_order(&redraw(&screen, None), 152, 45);
            assert_eq!(shown.last(), Some(&150));
            assert_eq!(shown.len(), 22);
            feed(&mut screen, marker(wide_size).as_bytes());
            feed(&mut screen, repaint(129, 150, 22, 23).as_bytes());
            let shown = lines_in_order(&redraw(&screen, None), 200, 45);
            assert_eq!(shown.last(), Some(&150));
            assert_eq!(shown.len(), 44, "the history fills the screen");
        }
        let logged: Vec<String> = screen.terminal_mut().take_log();
        let expected: Vec<String> = (1..=150).map(wide).collect();
        assert_eq!(logged, expected);
    }

    /// The alternate screen is sent as the program drew it.
    #[test]
    fn the_alternate_screen_is_sent_as_drawn() {
        let mut screen = Rendered::new(
            TerminalSize {
                columns: 40,
                rows: 5,
            },
            1,
        );
        feed(&mut screen, b"before\r\n\x1b[?1049h\x1b[Hmenu");
        let shown = draw(&redraw(&screen, None), 40, 5);
        assert_eq!(shown.screen().contents(), "menu");
    }
}
