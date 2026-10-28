//! A unit's screen model and the recent output it was built from.
//!
//! A terminal grid keeps every line wrapped at the width it was printed at, so
//! after a resize, or for a viewer of another size, the grid alone shows long
//! lines cut at the old width and leaves extra rows empty. Programs on the
//! main screen (dev servers, loggers, pipe units) get their snapshot by
//! replaying the recent output at the viewer's size instead, the way a fresh
//! terminal of that size would have drawn it. Programs on the alternate screen
//! (editors, pagers) redraw themselves after a resize, so their grid is sent.

use std::collections::VecDeque;

use bytes::Bytes;
use devlanes_core::TerminalSize;

use crate::terminal::{Terminal, cells};

/// Recent output kept for replay: enough for several screens of colored log
/// lines at any width.
const HISTORY: usize = 128 * 1024;
/// How far into trimmed history a line start is looked for.
const LINE_SEARCH: usize = 16 * 1024;

/// The screen model plus the output it was fed, for replay at other sizes.
pub(crate) struct Screen {
    terminal: Terminal,
    /// The last [`HISTORY`] bytes fed to `terminal`, starting at a line start
    /// once trimmed.
    history: VecDeque<u8>,
}

impl Screen {
    /// An empty screen of `size` whose first logged line gets `first_seq`.
    pub(crate) fn new(size: TerminalSize, first_seq: u64) -> Self {
        Self {
            terminal: Terminal::new(size, first_seq, true),
            history: VecDeque::new(),
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

    /// Feeds output to the model and the replay history.
    pub(crate) fn process(&mut self, bytes: &[u8]) {
        self.terminal.process(bytes);
        self.history.extend(bytes);
        if self.history.len() > HISTORY {
            self.history.drain(..self.history.len() - HISTORY);
            // A replay starting mid-line would show a fragment as a whole line.
            // The search is bounded: output redrawn with `\r` alone has no line
            // ends, and scanning all of it for every chunk would add up.
            if let Some(end) = self
                .history
                .iter()
                .take(LINE_SEARCH)
                .position(|&byte| byte == b'\n')
            {
                self.history.drain(..=end);
            }
        }
    }

    /// Resizes the grid, as the PTY was, re-wrapping what it shows.
    pub(crate) fn resize(&mut self, size: TerminalSize) {
        self.terminal.reflow(size);
    }

    /// The model as the program drew it.
    pub(crate) fn screen(&self) -> &vt100::Screen {
        self.terminal.screen()
    }

    /// Clear, home and the screen as a viewer of `size` should see it (the
    /// grid's size when `None`), plus the program's input modes.
    pub(crate) fn snapshot(&self, size: Option<TerminalSize>) -> Bytes {
        let screen = self.terminal.screen();
        let mut bytes = b"\x1b[H\x1b[2J".to_vec();
        if screen.alternate_screen() {
            bytes.extend_from_slice(screen.contents_formatted().as_bytes());
        } else {
            let size = size.unwrap_or_else(|| self.terminal.size());
            let mut replay = vt100::Parser::new(cells(size.rows), cells(size.columns), 0);
            let (front, back) = self.history.as_slices();
            replay.process(front);
            replay.process(back);
            bytes.extend_from_slice(replay.screen().contents_formatted().as_bytes());
            // The sequence that hid the cursor may be older than the history.
            bytes.extend_from_slice(if screen.hide_cursor() {
                b"\x1b[?25l"
            } else {
                b"\x1b[?25h"
            });
        }
        bytes.extend_from_slice(screen.input_mode_formatted().as_bytes());
        Bytes::from(bytes)
    }
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

    /// Lines printed at 120 columns and shown after a resize to 200x30 sit one
    /// per row and fill the rows that were empty, as a fresh 200-column
    /// terminal would show them.
    #[test]
    fn a_resize_shows_lines_rewrapped_at_the_new_size() {
        let mut screen = Screen::new(TerminalSize::default(), 1);
        for i in 0..60 {
            screen.process(format!("{i:03} {}\r\n", "x".repeat(176)).as_bytes());
        }
        let size = TerminalSize {
            columns: 200,
            rows: 30,
        };
        screen.resize(size);
        let shown = draw(&screen.snapshot(None), 200, 30);
        let rows: Vec<String> = shown.screen().rows(0, 200).collect();
        // The last row holds the cursor after the final newline.
        assert_eq!(rows[29], "");
        for (row, i) in rows[..29].iter().zip(31..) {
            assert_eq!(row, &format!("{i:03} {}", "x".repeat(176)));
        }
    }

    /// An observer of another size gets the output at its own size.
    #[test]
    fn a_snapshot_uses_the_viewer_size() {
        let mut screen = Screen::new(TerminalSize::default(), 1);
        screen.process(format!("{}\r\nnext\r\n", "y".repeat(150)).as_bytes());
        let size = TerminalSize {
            columns: 160,
            rows: 10,
        };
        let shown = draw(&screen.snapshot(Some(size)), 160, 10);
        assert_eq!(
            shown.screen().contents(),
            format!("{}\nnext", "y".repeat(150))
        );
    }

    /// The alternate screen is sent as the program drew it.
    #[test]
    fn the_alternate_screen_is_sent_as_drawn() {
        let mut screen = Screen::new(
            TerminalSize {
                columns: 40,
                rows: 5,
            },
            1,
        );
        screen.process(b"before\r\n\x1b[?1049h\x1b[Hmenu");
        let shown = draw(&screen.snapshot(None), 40, 5);
        assert_eq!(shown.screen().contents(), "menu");
    }

    /// Trimmed history starts at a line, never in the middle of one.
    #[test]
    fn history_is_trimmed_at_a_line_start() {
        let mut screen = Screen::new(TerminalSize::default(), 1);
        let line = format!("{}\r\n", "z".repeat(998));
        for _ in 0..(HISTORY / line.len() + 10) {
            screen.process(line.as_bytes());
        }
        assert!(screen.history.len() <= HISTORY);
        assert_eq!(screen.history.front(), Some(&b'z'));
        assert_eq!(screen.history.len() % line.len(), 0);
    }
}
