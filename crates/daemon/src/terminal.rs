//! The screen model all of a unit's output goes through, and what it decides
//! about lines. A line goes to the unit's log once the program has written
//! past it, and to the viewers' scrollback once it leaves the top of the
//! screen. Both come from the same grid, so output drawn over itself (a
//! console host repainting its window after a resize, a status line
//! rewritten in place) is logged once, and a viewer's scrollback, its screen
//! and the log join without lines missing or repeated.

use std::{
    collections::VecDeque,
    hash::{DefaultHasher, Hash, Hasher},
    num::NonZeroU16,
};

use devlanes_core::TerminalSize;
use vt100::capture::{BasicFormattedCaptureRange, BasicFormattedCaptureState, RowContents};

/// Lines kept for viewers: a new viewer's backlog ([`VIEW_BACKLOG`]) plus
/// room for one that falls behind; older ones come from the log.
const HISTORY_LINES: usize = 2_000;
/// Bytes kept for viewers, whichever bound comes first. Every unit keeps
/// this much, attached or not.
const HISTORY_BYTES: usize = 512 * 1024;
/// Style kept in effect across lines: vt100 writes changes, not resets,
/// between colors, so a program that never resets would grow it without end.
const STYLE_BYTES: usize = 256;

/// A line that left the screen, as drawn (text with SGR colors, starting
/// from none and ending in a reset), and the log sequence it was stored at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    pub styled: String,
    pub seq: u64,
}

/// Collects the rows that leave the top of the main screen as one capture.
#[derive(Default)]
pub(crate) struct Rows {
    text: String,
    state: BasicFormattedCaptureState,
    /// The last row ended its line: the capture writes a line break before
    /// the next row.
    ended: bool,
}

impl vt100::Callbacks for Rows {
    /// Keeps a row of the main screen that scrolled off, joined to the one
    /// before when that one wrapped.
    fn on_scroll(&mut self, contents: RowContents<'_>, alternate_screen: bool) {
        if !alternate_screen {
            // Writing to a `String` cannot fail.
            let _ = contents.write_formatted_basic(&mut self.text, &mut self.state);
            // The state keeps whether the row wrapped to itself; writing the
            // row again from a copy shows the line break it owes.
            let mut probe = self.state.clone();
            let mut written = String::new();
            let _ = contents.write_formatted_basic(&mut written, &mut probe);
            self.ended = written.starts_with('\n');
        }
    }
}

/// A line on the grid that the log has: where it went and its text's hash.
struct Logged {
    hash: u64,
    seq: u64,
}

/// A unit's terminal: the grid at the unit's size, the lines that left it
/// and the bookkeeping that keeps the log free of repeats.
pub(crate) struct Terminal {
    parser: vt100::Parser<Rows>,
    /// Lines that left the screen, oldest first.
    history: VecDeque<Line>,
    history_bytes: usize,
    /// Index of `history`'s first line among every line that left the screen.
    start: u64,
    /// History index from which a whole-screen redraw replays; a clear of
    /// the scrollback moves it to the end.
    replay_from: u64,
    /// The rows of a line whose start left the screen and whose end did not.
    open: String,
    /// Style in effect where `open` starts.
    open_style: String,
    /// The capture already ended `open`; the line break it writes before its
    /// next row belongs to that line.
    ended: bool,
    /// The grid's lines from the top that the log has.
    logged: VecDeque<Logged>,
    next_seq: u64,
    /// Lines for the log since [`Self::take_log`].
    log: Vec<String>,
    /// Whether an erase of the whole screen ends what it showed, as on a Unix
    /// terminal. A console host erases rows to repaint its window and passes
    /// on only a clear of the scrollback (`cls`), so there only that does.
    erase_ends: bool,
    /// The start of a sequence [`next_boundary`] finds, at the end of the
    /// last chunk.
    seam: Vec<u8>,
}

/// The switches to the alternate screen.
const ALTERNATE: [&[u8]; 3] = [b"\x1b[?1049h", b"\x1b[?1047h", b"\x1b[?47h"];
/// Longest sequence [`next_boundary`] finds: `ESC [ ? 1049 h`.
const BOUNDARY_MAX: usize = 8;

impl Terminal {
    /// An empty grid of `size` whose first logged line gets sequence `first_seq`.
    pub(crate) fn new(size: TerminalSize, first_seq: u64, erase_ends: bool) -> Self {
        Self {
            parser: parser(size, Rows::default()),
            history: VecDeque::new(),
            history_bytes: 0,
            start: 0,
            replay_from: 0,
            open: String::new(),
            open_style: String::new(),
            ended: false,
            logged: VecDeque::new(),
            next_seq: first_seq,
            log: Vec::new(),
            erase_ends,
            seam: Vec::new(),
        }
    }

    /// The grid as the program drew it.
    pub(crate) fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// The grid's size.
    pub(crate) fn size(&self) -> TerminalSize {
        let (rows, columns) = self.parser.screen().size();
        TerminalSize {
            columns: columns.get(),
            rows: rows.get(),
        }
    }

    /// Feeds output. What a clear of the screen removes, or a switch to the
    /// alternate screen hides, reaches the log first; everything else waits
    /// for [`Self::settle`].
    pub(crate) fn process(&mut self, mut bytes: &[u8]) {
        // A sequence split across two chunks: its start is already in the parser.
        let seam = std::mem::take(&mut self.seam);
        if !seam.is_empty() {
            let mut joined = seam.clone();
            joined.extend_from_slice(&bytes[..bytes.len().min(BOUNDARY_MAX)]);
            if let Some((0, end)) = next_boundary(&joined) {
                let rest = end - seam.len();
                self.boundary(&joined[..end], &bytes[..rest]);
                bytes = &bytes[rest..];
            }
        }
        while let Some((start, end)) = next_boundary(bytes) {
            self.parser.process(&bytes[..start]);
            self.boundary(&bytes[start..end], &bytes[start..end]);
            bytes = &bytes[end..];
        }
        self.parser.process(bytes);
        self.seam = bytes[bytes.len() - boundary_prefix(bytes)..].to_vec();
    }

    /// Feeds `fed`, the part of `sequence` (see [`next_boundary`]) the parser
    /// has not seen. A switch to the alternate screen logs what the main
    /// screen shows first: a unit that exits before switching back would
    /// otherwise never log it. An erase that ends the screen does the same,
    /// then starts a new one.
    fn boundary(&mut self, sequence: &[u8], fed: &[u8]) {
        let main = !self.parser.screen().alternate_screen();
        if main && ALTERNATE.contains(&sequence) {
            self.settle_through(Through::LastText);
            self.parser.process(fed);
            return;
        }
        let ends = main && self.ends_screen(sequence);
        if ends {
            self.settle_through(Through::LastText);
        }
        self.parser.process(fed);
        if ends {
            self.take_scrolled();
            // A new screen starts: its lines are new even where they repeat
            // the old ones, and a line cut by the clear is over.
            self.logged.clear();
            if !self.ended {
                let open = std::mem::take(&mut self.open);
                apply_style(&mut self.open_style, &open);
            }
            self.replay_from = self.end();
        }
    }

    /// Logs the lines the program wrote past and moves the rows that left
    /// the screen into the history. Call it after each batch of output.
    pub(crate) fn settle(&mut self) {
        self.settle_through(Through::Cursor);
    }

    /// [`Self::settle`], and the cursor's line as well: output paused there,
    /// so a prompt waiting for input shows in the log.
    pub(crate) fn settle_paused(&mut self) {
        self.settle_through(Through::CursorLine);
    }

    /// Logs everything the screen still shows, for the end of output.
    pub(crate) fn finish(&mut self) {
        self.settle_through(Through::LastText);
    }

    /// Lines for the log since the last call, in order.
    pub(crate) fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    /// Resizes the grid the way a console host resizes its buffer, and the
    /// way terminals that re-wrap do, so nothing written is cut off. On the
    /// main screen the grid's lines are re-wrapped at the new width and keep
    /// their place from the top; rows that no longer fit push the top ones
    /// out of the grid, and a taller grid gets its new rows at the bottom.
    /// Nothing comes back from the history: a console host keeps no rows
    /// above its window. A program on the alternate screen redraws itself,
    /// so that grid is only cut or extended.
    pub(crate) fn reflow(&mut self, size: TerminalSize) {
        if self.parser.screen().alternate_screen() {
            // A taller grid would take rows back from the scrollback, and with
            // none kept vt100 adds blank ones at the top instead.
            self.parser.screen_mut().clear_scrollback();
            self.parser.set_size(cells(size.rows), cells(size.columns));
        } else {
            let replay = self.reflowed(size);
            let rows = std::mem::take(self.parser.callbacks_mut());
            self.parser = parser(size, rows);
            self.parser.process(&replay);
        }
        self.take_scrolled();
        self.rebase();
    }

    /// The bytes that draw the grid's lines, joined where a row wrapped, into
    /// an empty grid of `size`, with the cursor on the same character and the
    /// program's attributes and modes. Blank lines at the bottom, below the
    /// cursor, are left out, so they push nothing out of the grid.
    fn reflowed(&self, size: TerminalSize) -> Vec<u8> {
        let screen = self.parser.screen();
        let grid = self.size();
        let (cursor_row, cursor_column) = screen.cursor_position();
        // Each line as its first row and its width in cells.
        let mut lines: Vec<(u16, usize)> = Vec::new();
        let mut first = 0;
        for row in 0..grid.rows {
            if !screen.row_wrapped(row) || row + 1 == grid.rows {
                let width = usize::from(row - first) * usize::from(grid.columns)
                    + usize::from(written_end(screen, row));
                lines.push((first, width));
                first = row + 1;
            }
        }
        let cursor_line = lines
            .iter()
            .rposition(|&(first, _)| first <= cursor_row)
            .unwrap_or(0);
        let last = lines
            .iter()
            .rposition(|&(_, width)| width > 0)
            .map_or(cursor_line, |last| last.max(cursor_line));
        let mut text = String::new();
        // Writing to a `String` cannot fail.
        let _ = screen.write_contents_formatted_basic(
            &mut text,
            BasicFormattedCaptureRange::Full(&mut BasicFormattedCaptureState::default()),
        );
        let text = trim_blank_ends(&text);
        let columns = usize::from(size.columns.max(1));
        let rows_of = |width: usize| width.div_ceil(columns).max(1);
        let mut bytes = Vec::new();
        for (i, line) in text.split('\n').take(last + 1).enumerate() {
            if i > 0 {
                bytes.extend_from_slice(b"\r\n");
            }
            bytes.extend_from_slice(line.as_bytes());
        }
        let fed: usize = lines[..=last]
            .iter()
            .map(|&(_, width)| rows_of(width))
            .sum();
        let scrolled = fed.saturating_sub(usize::from(size.rows));
        let (first, _) = lines[cursor_line];
        let offset = usize::from(cursor_row - first) * usize::from(grid.columns)
            + usize::from(cursor_column);
        let above: usize = lines[..cursor_line]
            .iter()
            .map(|&(_, width)| rows_of(width))
            .sum();
        let row = (above + offset / columns)
            .saturating_sub(scrolled)
            .min(usize::from(size.rows.max(1) - 1));
        bytes.extend_from_slice(format!("\x1b[{};{}H", row + 1, offset % columns + 1).as_bytes());
        bytes.extend_from_slice(screen.attributes_formatted().as_bytes());
        bytes.extend_from_slice(screen.input_mode_formatted().as_bytes());
        if screen.hide_cursor() {
            bytes.extend_from_slice(b"\x1b[?25l");
        }
        bytes
    }

    /// Lines that left the screen from index `from` on (from the oldest kept
    /// when it is older), with the index of the first one returned.
    pub(crate) fn history_since(&self, from: u64) -> (u64, impl Iterator<Item = &Line>) {
        let from = from.clamp(self.start, self.end());
        let skip = usize::try_from(from - self.start).unwrap_or(usize::MAX);
        (from, self.history.iter().skip(skip))
    }

    /// Index after the last line that left the screen.
    pub(crate) fn end(&self) -> u64 {
        self.start + self.history.len() as u64
    }

    /// The lines a whole-screen redraw replays above the grid: those since
    /// the last clear of the scrollback.
    pub(crate) fn replayed_history(&self) -> impl Iterator<Item = &Line> {
        self.history_since(self.replay_from).1
    }

    /// The grid's lines as text that continues the history, for a viewer of
    /// `size`. Blank rows under the cursor are left out: a console host's
    /// repaint after a resize leaves them, and replayed they would push lines
    /// off the top. A viewer as wide as the grid wraps the cursor's line as
    /// the grid did, so the cursor keeps its column there; for any other
    /// viewer the cursor moves on from the row's last cell as far as it is
    /// from it in the grid, which keeps a prompt's trailing space (the console
    /// host sends a cursor move for it, not a cell).
    pub(crate) fn grid_lines(&self, size: TerminalSize) -> Vec<u8> {
        let screen = self.parser.screen();
        let text = self.grid_capture();
        let lines = text.trim_end_matches('\n');
        // Each blank row at the bottom adds one line break.
        let blank = text.len() - lines.len();
        let grid = self.size();
        let (row, column) = screen.cursor_position();
        let below = usize::from(grid.rows.saturating_sub(row + 1));
        let mut bytes = crlf(lines);
        bytes.extend_from_slice(&b"\r\n".repeat(blank.saturating_sub(below)));
        bytes.extend_from_slice(b"\x1b[m");
        if blank >= below {
            if size.columns == grid.columns {
                bytes.extend_from_slice(format!("\x1b[{}G", column + 1).as_bytes());
            } else {
                let gap = column.saturating_sub(written_end(screen, row));
                if gap > 0 {
                    bytes.extend_from_slice(format!("\x1b[{gap}C").as_bytes());
                }
            }
        }
        bytes
    }

    /// What a viewer that keeps its own scrollback needs for a redraw at
    /// `size`: the lines that left the screen from index `since` on (the
    /// newest [`VIEW_BACKLOG`] for a new viewer), and the grid.
    pub(crate) fn view(&self, size: TerminalSize, since: Option<u64>) -> View {
        let since = since.unwrap_or_else(|| self.end().saturating_sub(VIEW_BACKLOG));
        let (from, lines) = self.history_since(since);
        let lines: Vec<Line> = lines.cloned().collect();
        let before_seq = lines
            .first()
            .map(|line| line.seq)
            .or_else(|| self.logged.front().map(|logged| logged.seq))
            .unwrap_or(self.next_seq);
        let screen = self.parser.screen();
        let grid = if screen.alternate_screen() {
            Grid::Drawn {
                contents: screen.contents_formatted(),
                size: self.size(),
            }
        } else {
            Grid::Lines(self.grid_lines(size))
        };
        View {
            from,
            lines,
            before_seq,
            grid,
            size,
            hide_cursor: screen.hide_cursor(),
            input_modes: screen.input_mode_formatted(),
        }
    }

    /// The grid's capture, starting with the rows of `open` that already
    /// left it, in the style they started in. Lines end in `\n`.
    fn grid_capture(&self) -> String {
        let (starts, capture) = self.capture();
        self.joined(starts, capture)
    }

    /// Logs the grid's lines through `through`, after moving the rows that
    /// left it into the history.
    fn settle_through(&mut self, through: Through) {
        let capture = self.take_scrolled();
        if self.parser.screen().alternate_screen() {
            return;
        }
        let lines = styled_lines(&capture);
        let cursor = self.cursor_line();
        let end = match through {
            Through::Cursor => cursor,
            Through::CursorLine => {
                cursor + usize::from(!is_blank(&lines[cursor.min(lines.len() - 1)]))
            }
            Through::LastText => lines
                .iter()
                .rposition(|line| !is_blank(line))
                .map_or(0, |last| last + 1),
        };
        for (index, line) in lines.into_iter().enumerate().take(end) {
            self.log_grid_line(index, line);
        }
    }

    /// Logs grid line `index` unless the log has it as it reads now. A line
    /// the program wrote over is logged again with its new text; one it
    /// erased is not, since nothing new was written.
    fn log_grid_line(&mut self, index: usize, line: String) {
        let hash = hash(&line);
        match self.logged.get(index) {
            Some(logged) if logged.hash == hash || is_blank(&line) => {}
            Some(_) => {
                let seq = self.write(line);
                self.logged[index] = Logged { hash, seq };
            }
            None => {
                // Lines are logged top down, so the ones above this one are in.
                let seq = self.write(line);
                self.logged.push_back(Logged { hash, seq });
            }
        }
    }

    /// Moves the lines whose last row left the screen into the history,
    /// logging those the log does not have as they read, and returns the
    /// grid's capture (see [`Self::grid_capture`]).
    fn take_scrolled(&mut self) -> String {
        let text = std::mem::take(&mut self.parser.callbacks_mut().text);
        let mut text = text.as_str();
        if self.ended && !text.is_empty() {
            text = text.strip_prefix('\n').unwrap_or(text);
            self.ended = false;
        }
        let mut pieces = text.split('\n');
        if let Some(first) = pieces.next() {
            self.open.push_str(first);
        }
        for piece in pieces {
            let raw = std::mem::replace(&mut self.open, piece.to_owned());
            self.scrolled(raw);
        }
        // The capture ends a line only when the next row comes; the grid's
        // capture shows whether its first row starts a line or continues `open`.
        let (starts, capture) = self.capture();
        if starts && !self.ended {
            let raw = std::mem::take(&mut self.open);
            self.scrolled(raw);
            self.ended = true;
        }
        self.joined(starts, capture)
    }

    /// The grid captured in continuation of the rows that left it: whether
    /// its first row starts a line, and the capture without that line break.
    fn capture(&self) -> (bool, String) {
        let rows = self.parser.callbacks();
        let mut state = rows.state.clone();
        let mut capture = String::new();
        // Writing to a `String` cannot fail.
        let _ = self.parser.screen().write_contents_formatted_basic(
            &mut capture,
            BasicFormattedCaptureRange::Full(&mut state),
        );
        if rows.ended && capture.starts_with('\n') {
            capture.remove(0);
        }
        (rows.ended, capture)
    }

    /// `capture` after the rows of `open` that already left the grid, when
    /// its first row continues them.
    fn joined(&self, starts: bool, capture: String) -> String {
        if starts {
            trim_blank_ends(&capture)
        } else {
            trim_blank_ends(&format!("{}{}{capture}", self.open_style, self.open))
        }
    }

    /// A line whose last row left the screen: into the history, and into
    /// the log unless the log has it as it reads.
    fn scrolled(&mut self, raw: String) {
        let line = trim_blank_ends(&styled(&self.open_style, &raw));
        apply_style(&mut self.open_style, &raw);
        let hash = hash(&line);
        let seq = match self.logged.pop_front() {
            Some(logged) if logged.hash == hash || is_blank(&line) => logged.seq,
            _ => self.write(line.clone()),
        };
        self.history_bytes += line.len();
        self.history.push_back(Line { styled: line, seq });
        while self.history.len() > HISTORY_LINES || self.history_bytes > HISTORY_BYTES {
            let Some(old) = self.history.pop_front() else {
                break;
            };
            self.history_bytes -= old.styled.len();
            self.start += 1;
        }
    }

    /// Queues `line` for the log; returns the sequence it gets there.
    fn write(&mut self, line: String) -> u64 {
        let seq = self.next_seq;
        self.next_seq += devlanes_logs::line_records(&line);
        self.log.push(line);
        seq
    }

    /// Keeps the logged lines that are still on the grid, matched to their
    /// text now: a resize cut or re-wrapped them without anything new.
    fn rebase(&mut self) {
        if self.parser.screen().alternate_screen() {
            return;
        }
        let lines = styled_lines(&self.grid_capture());
        let lines = &lines[..lines.len().min(self.logged.len())];
        self.logged.truncate(lines.len());
        for (logged, line) in self.logged.iter_mut().zip(lines) {
            logged.hash = hash(line);
        }
    }

    /// Index of the cursor's line among the grid's lines.
    fn cursor_line(&self) -> usize {
        let screen = self.parser.screen();
        let (row, _) = screen.cursor_position();
        (0..row).filter(|&row| !screen.row_wrapped(row)).count()
    }

    /// Whether `sequence`, found by [`next_boundary`], ends what the
    /// screen showed.
    fn ends_screen(&self, sequence: &[u8]) -> bool {
        if !self.erase_ends {
            return sequence == b"\x1b[3J";
        }
        match sequence {
            b"\x1bc" | b"\x1b[2J" => true,
            b"\x1b[J" | b"\x1b[0J" => self.parser.screen().cursor_position() == (0, 0),
            _ => false,
        }
    }
}

/// How far down the grid [`Terminal::settle_through`] logs.
enum Through {
    /// The lines above the cursor's.
    Cursor,
    /// Those and the cursor's line.
    CursorLine,
    /// Every line down to the last with text.
    LastText,
}

/// Lines a new viewer gets with its first redraw; it reads older ones from the log.
pub(crate) const VIEW_BACKLOG: u64 = 1_000;
/// Rows a viewer's screen grows to at most, for a grid whose lines wrap a lot.
const VIEW_ROWS: usize = 1_000;

/// One redraw for a viewer that keeps its own scrollback (see
/// [`Terminal::view`]): a copy, so the replay runs after the screen lock is
/// released.
pub(crate) struct View {
    /// Index of the first of `lines` among the lines that left the screen.
    pub from: u64,
    /// The lines that left the screen since the viewer's last redraw.
    pub lines: Vec<Line>,
    /// The log holds what is above `lines` (or the screen) below this sequence.
    pub before_seq: u64,
    grid: Grid,
    size: TerminalSize,
    hide_cursor: bool,
    input_modes: String,
}

/// The grid as a viewer redraws it.
enum Grid {
    /// The alternate screen as the program drew it, at the grid's size.
    Drawn {
        contents: String,
        size: TerminalSize,
    },
    /// The main screen's lines (see [`Terminal::grid_lines`]).
    Lines(Vec<u8>),
}

/// A viewer's screen after a redraw (see [`View::render`]).
pub(crate) struct Drawn {
    /// The screen's size: the viewer's width, and the rows its lines take.
    pub size: TerminalSize,
    /// The bytes that draw it over the screen the last redraw left, or over
    /// any screen when `whole`.
    pub bytes: String,
    pub whole: bool,
    pub screen: vt100::Screen,
}

impl View {
    /// Draws the grid for a viewer that shows `shown`. The screen is as wide
    /// as the viewer and exactly as tall as the grid's lines need at that
    /// width, down to the cursor's: a narrow viewer gets every line the grid
    /// holds, so none falls between its scrollback and its screen, and the
    /// lines above fill the rest of its pane. A program on the alternate
    /// screen gets the viewer's whole height. Only the rows that differ are
    /// drawn while the size stays. CPU work; blocking.
    pub(crate) fn render(&self, shown: Option<&vt100::Screen>) -> Drawn {
        let (rows, replay) = match &self.grid {
            Grid::Drawn { contents, size } => (size.rows.max(self.size.rows), contents.as_bytes()),
            Grid::Lines(bytes) => {
                let needed = rows_needed(bytes, self.size.columns).clamp(1, VIEW_ROWS);
                (u16::try_from(needed).unwrap_or(u16::MAX), bytes.as_slice())
            }
        };
        let mut parser = vt100::Parser::new(cells(rows), cells(self.size.columns), 0);
        parser.process(replay);
        let screen = parser.screen();
        let (mut bytes, whole) = match shown.filter(|shown| shown.size() == screen.size()) {
            Some(shown) => (screen.contents_diff(shown), false),
            None => (
                format!("\x1b[H\x1b[2J{}", screen.contents_formatted()),
                true,
            ),
        };
        // The sequence that hid the cursor is not part of the lines.
        bytes.push_str(if self.hide_cursor {
            "\x1b[?25l"
        } else {
            "\x1b[?25h"
        });
        bytes.push_str(&self.input_modes);
        Drawn {
            size: TerminalSize {
                columns: self.size.columns,
                rows,
            },
            bytes,
            whole,
            screen: screen.clone(),
        }
    }
}

/// Rows the grid lines in `bytes` take in a terminal `columns` wide.
fn rows_needed(bytes: &[u8], columns: u16) -> usize {
    let text = String::from_utf8_lossy(bytes);
    text.split("\r\n")
        .map(|line| rows_of(&visible_text(line), columns))
        .sum()
}

/// Rows `text` takes in a terminal `columns` wide, wrapped as vt100 does.
pub(crate) fn rows_of(text: &str, columns: u16) -> usize {
    use unicode_width::UnicodeWidthChar;
    let columns = usize::from(columns.max(1));
    let mut rows = 1;
    let mut column = 0;
    for c in text.chars() {
        let width = c.width().unwrap_or(0).min(2);
        if width == 0 {
            continue;
        }
        if column + width > columns {
            rows += 1;
            column = 0;
        }
        column += width;
    }
    rows
}

/// `line` without its control sequences (`ESC [ … final`).
fn visible_text(line: &str) -> String {
    let mut text = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
        } else if !c.is_control() {
            text.push(c);
        }
    }
    text
}

/// A parser of `size` that keeps no scrollback, so every row leaving the
/// top reaches `rows`.
fn parser(size: TerminalSize, rows: Rows) -> vt100::Parser<Rows> {
    vt100::Parser::new_with_callbacks(cells(size.rows), cells(size.columns), 0, rows)
}

/// Where the first sequence in `bytes` that ends or hides what the main
/// screen shows starts and ends: a terminal reset (`ESC c`), an erase of
/// the display (`ESC [ n J`) or a switch to the alternate screen
/// ([`ALTERNATE`]).
fn next_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(at) = bytes[from..].iter().position(|&byte| byte == 0x1b) {
        let start = from + at;
        let rest = &bytes[start..];
        if rest.starts_with(b"\x1bc") {
            return Some((start, start + 2));
        }
        if let Some(params) = rest.strip_prefix(b"\x1b[") {
            let digits = params
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if params.get(digits) == Some(&b'J') {
                return Some((start, start + 2 + digits + 1));
            }
        }
        if let Some(switch) = ALTERNATE.iter().find(|switch| rest.starts_with(switch)) {
            return Some((start, start + switch.len()));
        }
        from = start + 1;
    }
    None
}

/// Length of the start of a sequence [`next_boundary`] finds that `bytes`
/// ends with.
fn boundary_prefix(bytes: &[u8]) -> usize {
    let tail = &bytes[bytes.len().saturating_sub(BOUNDARY_MAX - 1)..];
    let Some(at) = tail.iter().rposition(|&byte| byte == 0x1b) else {
        return 0;
    };
    let rest = &tail[at..];
    let erase = rest
        .strip_prefix(b"\x1b[")
        .is_some_and(|digits| digits.iter().all(u8::is_ascii_digit));
    let started =
        rest == b"\x1b" || erase || ALTERNATE.iter().any(|switch| switch.starts_with(rest));
    if started { tail.len() - at } else { 0 }
}

/// A capture split into lines, each starting in the style the one before
/// left in effect and ending in a reset, so it reads the same alone.
fn styled_lines(capture: &str) -> Vec<String> {
    let mut style = String::new();
    capture
        .split('\n')
        .map(|raw| {
            let line = styled(&style, raw);
            apply_style(&mut style, raw);
            line
        })
        .collect()
}

/// `raw` drawn from no style: `style` first, then a reset if it has any.
fn styled(style: &str, raw: &str) -> String {
    // A line that starts by resetting owes nothing to the style before it.
    let (style, raw) = match raw
        .strip_prefix("\x1b[m")
        .or_else(|| raw.strip_prefix("\x1b[0m"))
    {
        Some(rest) => ("", rest),
        None => (style, raw),
    };
    if style.is_empty() && !raw.contains('\x1b') {
        return raw.to_owned();
    }
    format!("{style}{raw}\x1b[m")
}

/// `style` after the SGR sequences in `text`: a reset clears it, the
/// others add to it.
fn apply_style(style: &mut String, text: &str) {
    let mut rest = text;
    while let Some(start) = rest.find("\x1b[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('m') else {
            break;
        };
        let params = &after[..end];
        if params
            .split(';')
            .all(|param| param.is_empty() || param == "0")
        {
            style.clear();
        } else {
            style.push_str(&rest[start..start + 2 + end + 1]);
        }
        rest = &after[end + 1..];
    }
    if style.len() > STYLE_BYTES
        && let Some(cut) = style[style.len() - STYLE_BYTES..].find('\x1b')
    {
        style.drain(..style.len() - STYLE_BYTES + cut);
    }
}

/// Whether `line` shows nothing.
fn is_blank(line: &str) -> bool {
    devlanes_logs::strip_sgr(line).trim_end().is_empty()
}

/// Hash of `line`'s text; colors alone changing is not new output.
fn hash(line: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    devlanes_logs::strip_sgr(line).trim_end().hash(&mut hasher);
    hasher.finish()
}

/// The columns of `row` up to its last cell that shows something: text, or
/// a space with a background, inverse or underline (see [`trim_blank_ends`]).
pub(crate) fn written_end(screen: &vt100::Screen, row: u16) -> u16 {
    let columns = screen.size().1.get();
    (0..columns)
        .rev()
        .find(|&column| {
            screen.cell(row, column).is_some_and(|cell| {
                !cell.contents().trim().is_empty()
                    || cell.bgcolor() != vt100::Color::Default
                    || cell.inverse()
                    || cell.underline()
            })
        })
        .map_or(0, |column| column + 1)
}

/// `capture` without the spaces its lines end with that show nothing. A
/// console host draws the rest of a row as spaces in the color the text
/// ended in; counted as text, they make a line as wide as the console, and
/// it wraps into an empty-looking row in any narrower viewer. A space keeps
/// its place when a background, inverse or underline makes it visible.
/// Color sequences stay, so the style carried into the next line is the same.
fn trim_blank_ends(capture: &str) -> String {
    let mut out = String::with_capacity(capture.len());
    let mut shows = Shows::default();
    for (index, line) in capture.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        // What follows the last visible character, held until another comes.
        let mut tail = String::new();
        let mut rest = line;
        while let Some(c) = rest.chars().next() {
            if let Some(sequence) = sgr_at(rest) {
                shows.apply(sequence);
                tail.push_str(sequence);
                rest = &rest[sequence.len()..];
                continue;
            }
            if c == ' ' && !shows.any() {
                tail.push(c);
            } else {
                out.push_str(&tail);
                tail.clear();
                out.push(c);
            }
            rest = &rest[c.len_utf8()..];
        }
        // The trailing spaces go; the sequences among them stay.
        let mut sequences = tail.as_str();
        while let Some(start) = sequences.find('\x1b') {
            let sequence = sgr_at(&sequences[start..]).unwrap_or(&sequences[start..]);
            out.push_str(sequence);
            sequences = &sequences[start + sequence.len()..];
        }
    }
    out
}

/// The SGR sequence (`ESC [ … m`) `text` starts with.
fn sgr_at(text: &str) -> Option<&str> {
    let params = text.strip_prefix("\x1b[")?;
    let end = params.find(|c: char| !(c.is_ascii_digit() || c == ';' || c == ':'))?;
    (params.as_bytes()[end] == b'm').then(|| &text[..2 + end + 1])
}

/// The attributes in effect that make a space visible.
#[derive(Default)]
struct Shows {
    background: bool,
    inverse: bool,
    underline: bool,
}

impl Shows {
    /// Whether a space drawn now shows.
    fn any(&self) -> bool {
        self.background || self.inverse || self.underline
    }

    /// Follows the SGR `sequence` (`ESC [ … m`).
    fn apply(&mut self, sequence: &str) {
        let mut params = sequence[2..sequence.len() - 1].split(';');
        while let Some(param) = params.next() {
            let value = |text: &str| {
                text.split(':')
                    .next()
                    .unwrap_or("")
                    .parse::<u16>()
                    .unwrap_or(0)
            };
            match value(param) {
                0 => *self = Self::default(),
                4 => self.underline = true,
                24 => self.underline = false,
                7 => self.inverse = true,
                27 => self.inverse = false,
                40..=47 | 100..=107 => self.background = true,
                49 => self.background = false,
                color @ (38 | 48) => {
                    self.background |= color == 48;
                    // `38;5;n` or `38;2;r;g;b` take the next parameters; the
                    // `38:5:n` form keeps them inside this one.
                    if !param.contains(':') {
                        let extra = match params.next().map(value) {
                            Some(5) => 1,
                            Some(2) => 3,
                            _ => 0,
                        };
                        for _ in 0..extra {
                            params.next();
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// `text` with `\r\n` line ends; captured rows end lines with `\n` alone.
pub(crate) fn crlf(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len());
    for byte in text.bytes() {
        if byte == b'\n' {
            bytes.push(b'\r');
        }
        bytes.push(byte);
    }
    bytes
}

/// vt100 rejects zero-sized screens; a client reporting 0 rows still gets one.
pub(crate) fn cells(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value).unwrap_or(NonZeroU16::MIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: TerminalSize = TerminalSize {
        columns: 40,
        rows: 4,
    };

    /// A Unix terminal of [`SIZE`] logging from sequence 1.
    fn unix() -> Terminal {
        Terminal::new(SIZE, 1, true)
    }

    /// Feeds `bytes` and settles, as the pump does per batch.
    fn feed(terminal: &mut Terminal, bytes: &[u8]) -> Vec<String> {
        terminal.process(bytes);
        terminal.settle();
        terminal.take_log()
    }

    /// The history's plain text, oldest first.
    fn history(terminal: &Terminal) -> Vec<String> {
        terminal
            .history_since(0)
            .1
            .map(|line| devlanes_logs::strip_sgr(&line.styled))
            .collect()
    }

    #[test]
    /// A line goes to the log once the program wrote past it, and to the
    /// history with the same sequence once it leaves the screen; nothing is
    /// logged twice.
    fn lines_are_logged_once_and_keep_their_sequence() {
        let mut terminal = unix();
        assert_eq!(feed(&mut terminal, b"one\r\ntw"), ["one"]);
        assert_eq!(
            feed(&mut terminal, b"o\r\nthree\r\nfour\r\nfive\r\n"),
            ["two", "three", "four", "five"]
        );
        assert_eq!(history(&terminal), ["one", "two"]);
        let seqs: Vec<u64> = terminal.history_since(0).1.map(|line| line.seq).collect();
        assert_eq!(seqs, [1, 2]);
        assert!(feed(&mut terminal, b"").is_empty());
    }

    #[test]
    /// A prompt without a line end reaches the log when output pauses on it,
    /// and only once more if it changes.
    fn a_paused_prompt_is_logged() {
        let mut terminal = unix();
        assert!(feed(&mut terminal, b"continue? ").is_empty());
        terminal.settle_paused();
        // A trailing space shows nothing; the cursor still waits after it.
        assert_eq!(terminal.take_log(), ["continue?"]);
        assert!(feed(&mut terminal, b"\r\n").is_empty());
        terminal.settle_paused();
        assert!(terminal.take_log().is_empty());
    }

    #[test]
    /// A console host's repaint draws the same lines over themselves: the
    /// log gets nothing new, and neither does the history.
    fn a_repaint_logs_nothing() {
        let mut terminal = Terminal::new(SIZE, 1, false);
        feed(&mut terminal, b"one\r\ntwo\r\nthree\r\n");
        let repaint =
            b"\x1b[?25l\x1b[2J\x1b[Hone\x1b[K\r\ntwo\x1b[K\r\nthree\x1b[K\r\n\x1b[K\x1b[4;1H";
        assert!(feed(&mut terminal, repaint).is_empty());
        assert!(history(&terminal).is_empty());
        assert_eq!(feed(&mut terminal, b"four\r\nfive\r\n"), ["four", "five"]);
        assert_eq!(history(&terminal), ["one", "two"]);
    }

    #[test]
    /// A line the program rewrites in place is logged again with its new
    /// text, once; one it erases is not.
    fn a_rewritten_line_is_logged_again() {
        let mut terminal = unix();
        feed(&mut terminal, b"step 1/3\r\nnext\r\n");
        assert_eq!(
            feed(&mut terminal, b"\x1b[2A\rstep 3/3\x1b[K\x1b[2B\r"),
            ["step 3/3"]
        );
        assert!(feed(&mut terminal, b"\x1b[2A\x1b[2K\x1b[2B\r").is_empty());
        feed(&mut terminal, b"a\r\nb\r\nc\r\n");
        assert_eq!(history(&terminal)[..2], ["", "next"]);
    }

    #[test]
    /// A Unix clear logs what the screen showed, even a line without its end,
    /// and the same lines printed again afterwards are new output.
    fn a_clear_ends_the_screen() {
        let mut terminal = unix();
        terminal.process(b"ready\r\nwaiting\x1b[H\x1b[2Jready\r\n");
        terminal.settle();
        assert_eq!(terminal.take_log(), ["ready", "waiting", "ready"]);
        assert!(history(&terminal).is_empty(), "{:?}", history(&terminal));
    }

    #[test]
    /// A console host clears rows to repaint them; only its clear of the
    /// scrollback (`cls`) ends the screen.
    fn a_console_host_ends_the_screen_with_a_scrollback_clear() {
        let mut terminal = Terminal::new(SIZE, 1, false);
        feed(&mut terminal, b"ready\r\n");
        assert!(feed(&mut terminal, b"\x1b[2J\x1b[Hready\r\n").is_empty());
        assert_eq!(
            feed(&mut terminal, b"\x1b[H\x1b[K\x1b[3J\x1b[Hready\r\n"),
            ["ready"]
        );
    }

    #[test]
    /// A line longer than the screen leaves it as one history line, logged
    /// once, and its colors read the same alone.
    fn a_long_colored_line_leaves_whole() {
        let mut terminal = unix();
        let long = format!("\x1b[32m{}\x1b[m\r\nend\r\n", "x".repeat(200));
        assert_eq!(feed(&mut terminal, long.as_bytes()).len(), 2);
        feed(&mut terminal, b"a\r\nb\r\nc\r\n");
        let first = terminal
            .history_since(0)
            .1
            .next()
            .map(|line| line.styled.clone());
        let first = first.unwrap_or_default();
        assert!(
            first.starts_with("\x1b[32m") && first.ends_with("\x1b[m"),
            "{first:?}"
        );
        assert_eq!(devlanes_logs::strip_sgr(&first), "x".repeat(200));
        assert_eq!(history(&terminal)[1], "end");
    }

    #[test]
    /// A colored line continued on the next line keeps its color there.
    fn colors_carry_into_the_next_line() {
        let mut terminal = unix();
        let log = feed(
            &mut terminal,
            b"\x1b[31mred\r\nstill red\x1b[m\r\nplain\r\n",
        );
        assert_eq!(log[1], "\x1b[31mstill red\x1b[m");
        assert_eq!(log[2], "plain");
    }

    #[test]
    /// A narrower grid re-wraps its lines and pushes the top ones out; that
    /// is not new output, and nothing written is cut off.
    fn a_resize_logs_nothing() {
        let mut terminal = unix();
        feed(&mut terminal, b"a long line of output\r\nmore\r\n");
        terminal.reflow(TerminalSize {
            columns: 10,
            rows: 2,
        });
        terminal.settle();
        assert!(terminal.take_log().is_empty());
        assert_eq!(feed(&mut terminal, b"next\r\n"), ["next"]);
        assert_eq!(history(&terminal), ["a long line of output", "more"]);
    }

    #[test]
    /// Output written on the alternate screen stays out of the log.
    fn the_alternate_screen_is_not_logged() {
        let mut terminal = unix();
        assert!(feed(&mut terminal, b"\x1b[?1049hmenu\r\nitem\r\n").is_empty());
        assert_eq!(feed(&mut terminal, b"\x1b[?1049ldone\r\n"), ["done"]);
    }

    #[test]
    /// Lines printed in the same batch as a switch to the alternate screen
    /// are logged once, even when the unit ends before switching back, and
    /// also when the switch is split across two chunks.
    fn lines_before_the_alternate_screen_are_logged() {
        let mut terminal = unix();
        terminal.process(b"before\r\n\x1b[?1049hmenu\r\n");
        terminal.settle();
        terminal.finish();
        assert_eq!(terminal.take_log(), ["before"]);
        let mut terminal = unix();
        terminal.process(b"one\r\ntwo\x1b[?10");
        terminal.process(b"49hmenu");
        terminal.settle();
        terminal.finish();
        assert_eq!(terminal.take_log(), ["one", "two"]);
        // Back on the main screen, those lines are not new.
        terminal.process(b"\x1b[?1049l\r\nafter\r\n");
        terminal.settle();
        assert_eq!(terminal.take_log(), ["after"]);
    }

    #[test]
    /// A console host draws the rest of a colored line's row as spaces in
    /// that color. They are not part of the line: a narrower viewer gets one
    /// row per line, not an empty-looking second one, and the scrollback and
    /// the log hold the line without them. Spaces with a background stay.
    fn spaces_a_console_pads_a_row_with_are_not_text() {
        let mut terminal = unix();
        let pad = " ".repeat(25);
        let output = format!(
            "\x1b[36mready\x1b[K\x1b[39m\r\n\x1b[32mlisten {pad}\x1b[39m\r\n\x1b[44m   \x1b[m bar\r\n"
        );
        let log = feed(&mut terminal, output.as_bytes());
        let texts: Vec<String> = log
            .iter()
            .map(|line| devlanes_logs::strip_sgr(line))
            .collect();
        assert_eq!(texts, ["ready", "listen", "    bar"]);
        let narrow = TerminalSize {
            columns: 20,
            rows: 4,
        };
        let drawn = terminal.view(narrow, None).render(None);
        assert_eq!(drawn.size.rows, 4, "one row per line, then the cursor's");
        let rows: Vec<String> = drawn.screen.rows(0, 20).collect();
        assert_eq!(rows[..3], ["ready", "listen", "    bar"]);
        assert_eq!(
            drawn.screen.cell(2, 0).map(vt100::Cell::bgcolor),
            Some(vt100::Color::Idx(4))
        );
        feed(&mut terminal, b"a\r\nb\r\nc\r\n");
        assert_eq!(history(&terminal), ["ready", "listen", "    bar"]);
        assert!(
            terminal
                .history_since(0)
                .1
                .all(|line| !devlanes_logs::strip_sgr(&line.styled).ends_with(' '))
        );
    }

    #[test]
    /// The end of output logs what is left on the screen.
    fn finish_logs_the_rest() {
        let mut terminal = unix();
        feed(&mut terminal, b"one\r\nlast words");
        terminal.finish();
        assert_eq!(terminal.take_log(), ["last words"]);
    }
}
