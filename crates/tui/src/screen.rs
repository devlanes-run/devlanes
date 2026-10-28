//! Client-side terminal emulation for the one visible unit.
//!
//! A unit's pane is one surface: the lines that left the unit's screen (the
//! daemon's [`AttachView`] lines, then older ones read from its log) above
//! the screen the daemon redraws. A scroll moves over both alike, so there is
//! no point where the pane switches to another view of the output. A
//! container's Docker logs keep vt100's own scrollback instead.
//!
//! tui-term 0.3 depends on vt100 0.16 while devlanes uses 0.18, so its
//! built-in `Screen` impl does not apply. [`ScreenView`] implements tui-term's
//! traits on newtypes instead.

use std::{collections::VecDeque, num::NonZeroU16};

use devlanes_proto::{AttachView, LogLine, Page};
use ratatui::{
    buffer::Cell as BufferCell,
    style::{Color, Modifier, Style},
};

/// Lines kept for a container's Docker logs; older ones stay with Docker.
pub const SCROLLBACK_LINES: usize = 2000;
/// Lines a unit's pane keeps above its screen; past this, the oldest go and
/// are read from the log again when scrolled to.
const HISTORY_LINES: usize = 50_000;

/// The attached stream's screen plus the user's scroll position.
pub struct TerminalScreen {
    parser: vt100::Parser,
    /// The pane's `(rows, columns)`.
    pane: (u16, u16),
    /// The lines above the screen, for a stream that keeps its own
    /// scrollback; `None` for one whose scrollback is the parser's own.
    history: Option<History>,
    /// Rows scrolled back from the bottom, with `history`.
    offset: usize,
}

/// The lines above a unit's screen, oldest first.
#[derive(Default)]
struct History {
    lines: VecDeque<HistoryLine>,
    /// Rows each line takes at `width`.
    rows: VecDeque<usize>,
    width: u16,
    total: usize,
    /// The log's name for the unit, for cursors.
    log_unit: String,
    /// Cursor for the log's lines older than the first; `None` when there
    /// are none, or before the first view.
    before: Option<String>,
    loading: bool,
    /// The next view starts over: the stream was reopened.
    restart: bool,
    /// Index the next view's lines should start at; `None` before the first.
    next: Option<u64>,
}

/// A line above the screen: as drawn, and where the log has it.
struct HistoryLine {
    styled: String,
    seq: u64,
}

impl TerminalScreen {
    /// A screen for a stream of raw terminal bytes with its own scrollback;
    /// zero sizes are clamped to one cell.
    pub fn new(rows: u16, columns: u16) -> Self {
        Self {
            parser: vt100::Parser::new(non_zero(rows), non_zero(columns), SCROLLBACK_LINES),
            pane: (rows, columns),
            history: None,
            offset: 0,
        }
    }

    /// A screen for a stream of [`AttachView`]s, which keeps the lines that
    /// left the unit's screen above it.
    pub fn with_history(rows: u16, columns: u16) -> Self {
        Self {
            parser: vt100::Parser::new(non_zero(rows), non_zero(columns), 0),
            pane: (rows, columns),
            history: Some(History {
                width: columns,
                ..History::default()
            }),
            offset: 0,
        }
    }

    /// Whether the screen keeps the lines above it (a unit's stream).
    pub fn has_history(&self) -> bool {
        self.history.is_some()
    }

    /// Whether the current stream drew this screen with views.
    pub fn viewed(&self) -> bool {
        self.history
            .as_ref()
            .is_some_and(|history| history.next.is_some() && !history.restart)
    }

    /// Feeds raw output from the attach stream.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// The stream was reopened: the screen stays until the new stream's first
    /// view replaces it and the lines above it.
    pub fn restart(&mut self) {
        if let Some(history) = &mut self.history {
            history.restart = true;
        }
    }

    /// Applies a redraw: the lines that left the unit's screen go above it,
    /// and the screen takes the view's size and rows. A pane scrolled back
    /// keeps showing the same rows.
    pub fn apply_view(&mut self, view: &AttachView) {
        let before = self.total_rows();
        // A raw stream never gets views; this keeps it from turning into one.
        let Some(history) = self.history.as_mut() else {
            return;
        };
        let mut lines = view.lines.as_slice();
        // The first view, or one after lines the daemon no longer kept, starts
        // the lines over; those above it come from the log.
        let restart = std::mem::take(&mut history.restart);
        let fell_behind = history.next.is_some_and(|next| view.from > next);
        if restart || fell_behind || history.next.is_none() {
            *history = History {
                width: history.width,
                log_unit: view.log_unit.clone(),
                before: Some(format!("before:{}#{}", view.log_unit, view.before_seq)),
                ..History::default()
            };
            if fell_behind {
                self.offset = 0;
            }
        } else if let Some(next) = history.next {
            // Lines the pane already has.
            let seen = usize::try_from(next.saturating_sub(view.from)).unwrap_or(usize::MAX);
            lines = &lines[seen.min(lines.len())..];
        }
        history.next = Some(view.from + view.lines.len() as u64);
        for line in lines {
            history.push_back(line.styled.clone(), line.seq);
        }
        let size = (non_zero(view.rows), non_zero(view.columns));
        let (rows, columns) = self.parser.screen().size();
        if view.whole || (rows, columns) != size {
            self.parser = vt100::Parser::new(size.0, size.1, 0);
        }
        self.parser.process(view.screen.as_bytes());
        // Rows came or went below what a scrolled pane shows (a shorter
        // screen loses its bottom rows), so the offset follows both ways.
        if self.offset > 0 {
            self.offset = (self.offset + self.total_rows()).saturating_sub(before);
        }
    }

    /// The cursor for the page of log lines before the oldest kept, when one
    /// is due: the pane shows the top of what is kept, and none is loading.
    pub fn wants_older(&mut self) -> Option<String> {
        let (rows, _) = self.pane;
        let top = self.top();
        let history = self.history.as_mut()?;
        if history.loading || top > usize::from(rows) {
            return None;
        }
        let cursor = history.before.clone()?;
        history.loading = true;
        Some(cursor)
    }

    /// Puts a page of log lines older than the oldest kept above it. The pane
    /// keeps showing the same rows.
    pub fn prepend(&mut self, page: Page<LogLine>) {
        let Some(history) = &mut self.history else {
            return;
        };
        history.loading = false;
        history.before = if page.truncated {
            page.next_cursor
        } else {
            None
        };
        for line in page.items.into_iter().rev() {
            let styled = line.styled.unwrap_or(line.text);
            history.push_front(styled, line.seq);
        }
    }

    /// A page of older lines could not be read; the next scroll asks again.
    pub fn older_failed(&mut self) {
        if let Some(history) = &mut self.history {
            history.loading = false;
        }
    }

    /// Resizes the pane. A raw stream's grid follows; a unit's screen waits
    /// for the daemon's redraw at the new size.
    pub fn resize(&mut self, rows: u16, columns: u16) {
        self.pane = (rows, columns);
        match &mut self.history {
            Some(history) => history.rewrap(columns),
            None => self
                .parser
                .screen_mut()
                .set_size(non_zero(rows), non_zero(columns)),
        }
    }

    /// The pane's `(rows, columns)`.
    pub fn size(&self) -> (u16, u16) {
        self.pane
    }

    /// The vt100 screen the stream draws, for input modes.
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Rows scrolled back from the live view.
    pub fn scroll_offset(&self) -> usize {
        match self.history {
            Some(_) => self.offset,
            None => self.parser.screen().scrollback(),
        }
    }

    /// Scrolls by `delta` rows (positive is older), no further than the
    /// oldest row kept.
    pub fn scroll_by(&mut self, delta: isize) {
        match self.history {
            Some(_) => {
                let rows = usize::from(self.pane.0);
                let most = self.total_rows().saturating_sub(rows);
                self.offset = self.offset.saturating_add_signed(delta).min(most);
            }
            None => {
                let wanted = self.scroll_offset().saturating_add_signed(delta);
                self.parser.screen_mut().set_scrollback(wanted);
            }
        }
    }

    /// Returns to the live view.
    pub fn scroll_to_bottom(&mut self) {
        self.offset = 0;
        self.parser.screen_mut().set_scrollback(0);
    }

    /// Plain text of the pane between two cells, inclusive of the start.
    pub fn text_between(&self, start: (u16, u16), end: (u16, u16)) -> String {
        let (first, last) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let view = self.view(self.pane.0, false);
        let screen = view
            .as_ref()
            .map_or(self.parser.screen(), vt100::Parser::screen);
        screen.contents_between(first.0, first.1, last.0, last.1.saturating_add(1))
    }

    /// Every row of the surface: the history's, then the screen's (without
    /// its blank rows at the bottom when `tail`).
    fn total_rows(&self) -> usize {
        self.history
            .as_ref()
            .map_or(0, |history| history.total + self.screen_rows(false))
    }

    /// The surface's row at the top of the pane.
    fn top(&self) -> usize {
        let rows = usize::from(self.pane.0);
        self.total_rows()
            .saturating_sub(rows)
            .saturating_sub(self.offset)
    }

    /// The screen's rows the surface holds: all, or with `tail` down to the
    /// last one with text, so a stopped unit's last output sits right under
    /// whatever is above it.
    fn screen_rows(&self, tail: bool) -> usize {
        let screen = self.parser.screen();
        let rows = screen.size().0.get();
        if !tail {
            return usize::from(rows);
        }
        let columns = screen.size().1.get();
        screen
            .rows(0, columns)
            .enumerate()
            .filter(|(_, row)| !row.trim_end().is_empty())
            .last()
            .map_or(0, |(row, _)| row + 1)
    }

    /// What `rows` rows of the pane show of a unit's surface: the rows at its
    /// scroll position, drawn as a terminal of that size, with the cursor
    /// where the program has it when the bottom shows. `None` for a raw
    /// stream, whose screen is drawn as it is.
    pub fn view(&self, rows: u16, tail: bool) -> Option<vt100::Parser> {
        let history = self.history.as_ref()?;
        let columns = self.pane.1;
        let height = usize::from(rows);
        let screen = self.parser.screen();
        let screen_rows = self.screen_rows(tail);
        let total = history.total + screen_rows;
        let top = total
            .saturating_sub(height)
            .saturating_sub(self.offset.min(total.saturating_sub(height)));
        let mut drawn = String::new();
        let mut row = 0;
        // History rows from `top` on.
        if top < history.total {
            let (mut index, mut skip) = history.locate(top);
            while row < height && index < history.lines.len() {
                let line = &history.lines[index];
                let line_rows = history.rows[index];
                let mut parser = vt100::Parser::new(non_zero_rows(line_rows), non_zero(columns), 0);
                parser.process(line.styled.as_bytes());
                for formatted in parser.screen().rows_formatted(0, columns).skip(skip) {
                    if row == height {
                        break;
                    }
                    drawn.push_str(&format!("\x1b[{};1H\x1b[m{formatted}", row + 1));
                    row += 1;
                }
                skip = 0;
                index += 1;
            }
        }
        // Then the screen's rows.
        let first = top.saturating_sub(history.total);
        for formatted in screen
            .rows_formatted(0, columns)
            .take(screen_rows)
            .skip(first)
        {
            if row == height {
                break;
            }
            drawn.push_str(&format!("\x1b[{};1H\x1b[m{formatted}", row + 1));
            row += 1;
        }
        drawn.push_str("\x1b[m");
        // The program's cursor, when its row is in the pane.
        let (cursor_row, cursor_column) = screen.cursor_position();
        let at = (history.total + usize::from(cursor_row)).checked_sub(top);
        match at.filter(|&at| at < height && !tail && !screen.hide_cursor()) {
            Some(at) => drawn.push_str(&format!("\x1b[{};{}H\x1b[?25h", at + 1, cursor_column + 1)),
            None => drawn.push_str("\x1b[?25l"),
        }
        let mut view = vt100::Parser::new(non_zero(rows), non_zero(columns), 0);
        view.process(drawn.as_bytes());
        Some(view)
    }
}

impl History {
    /// Adds a line below the others.
    fn push_back(&mut self, styled: String, seq: u64) {
        let rows = rows_of(&plain(&styled), self.width);
        self.total += rows;
        self.rows.push_back(rows);
        self.lines.push_back(HistoryLine { styled, seq });
        while self.lines.len() > HISTORY_LINES {
            self.lines.pop_front();
            self.total -= self.rows.pop_front().unwrap_or(0);
            if let Some(first) = self.lines.front() {
                self.before = Some(format!("before:{}#{}", self.log_unit, first.seq));
            }
        }
    }

    /// Adds a line above the others.
    fn push_front(&mut self, styled: String, seq: u64) {
        let rows = rows_of(&plain(&styled), self.width);
        self.total += rows;
        self.rows.push_front(rows);
        self.lines.push_front(HistoryLine { styled, seq });
    }

    /// Wraps the lines at `width` from now on.
    fn rewrap(&mut self, width: u16) {
        if width == self.width {
            return;
        }
        self.width = width;
        self.rows = self
            .lines
            .iter()
            .map(|line| rows_of(&plain(&line.styled), width))
            .collect();
        self.total = self.rows.iter().sum();
    }

    /// The line holding row `row` of the history, and that row's place in it.
    fn locate(&self, row: usize) -> (usize, usize) {
        // Panes mostly show the newest rows, so the search starts there.
        let mut below = self.total;
        for (index, rows) in self.rows.iter().enumerate().rev() {
            below -= rows;
            if below <= row {
                return (index, row - below);
            }
        }
        (0, 0)
    }
}

/// `styled` without its SGR sequences, the only ones stored lines keep.
fn plain(styled: &str) -> String {
    let mut text = String::with_capacity(styled.len());
    let mut rest = styled;
    while let Some(start) = rest.find("\x1b[") {
        text.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        rest = after.find('m').map_or("", |end| &after[end + 1..]);
    }
    text.push_str(rest);
    text
}

/// Rows `text` takes in a terminal `columns` wide, wrapped as vt100 does.
fn rows_of(text: &str, columns: u16) -> usize {
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

/// [`non_zero`] for a row count past `u16`.
fn non_zero_rows(rows: usize) -> NonZeroU16 {
    non_zero(u16::try_from(rows).unwrap_or(u16::MAX))
}

/// vt100 rejects zero dimensions; a hidden pane still keeps a 1x1 parser.
fn non_zero(value: u16) -> NonZeroU16 {
    NonZeroU16::new(value).unwrap_or(NonZeroU16::MIN)
}

/// One borrowed vt100 cell, so tui-term's `Cell` trait can be implemented locally.
pub struct CellView<'a>(&'a vt100::Cell);

/// Visible cells of a vt100 screen, gathered once per frame as references.
pub struct ScreenView<'a> {
    screen: &'a vt100::Screen,
    columns: u16,
    cells: Vec<Option<CellView<'a>>>,
}

impl<'a> ScreenView<'a> {
    /// Collects references to the visible cells; no cell contents are copied.
    pub fn new(screen: &'a vt100::Screen) -> Self {
        let (rows, columns) = screen.size();
        let (rows, columns) = (rows.get(), columns.get());
        let mut cells = Vec::with_capacity(usize::from(rows) * usize::from(columns));
        for row in 0..rows {
            for column in 0..columns {
                cells.push(screen.cell(row, column).map(CellView));
            }
        }
        Self {
            screen,
            columns,
            cells,
        }
    }
}

impl<'a> tui_term::widget::Screen for ScreenView<'a> {
    type C = CellView<'a>;

    /// Looks up a visible cell in the gathered grid.
    fn cell(&self, row: u16, col: u16) -> Option<&Self::C> {
        if col >= self.columns {
            return None;
        }
        let index = usize::from(row) * usize::from(self.columns) + usize::from(col);
        self.cells.get(index)?.as_ref()
    }

    /// Hidden while scrolled back, since the cursor belongs to the live screen.
    fn hide_cursor(&self) -> bool {
        self.screen.hide_cursor() || self.screen.scrollback() > 0
    }

    /// The cursor shifted by the scrollback offset, as tui-term expects.
    fn cursor_position(&self) -> (u16, u16) {
        let (row, col) = self.screen.cursor_position();
        let offset = u16::try_from(self.screen.scrollback()).unwrap_or(u16::MAX);
        (row.saturating_add(offset), col)
    }
}

impl tui_term::widget::Cell for CellView<'_> {
    /// Mirrors vt100's notion of an empty cell.
    fn has_contents(&self) -> bool {
        self.0.has_contents()
    }

    /// Copies symbol and style into the ratatui buffer.
    fn apply(&self, cell: &mut BufferCell) {
        apply_cell(self.0, cell);
    }
}

/// Writes one vt100 cell into a ratatui buffer cell.
pub fn apply_cell(source: &vt100::Cell, target: &mut BufferCell) {
    if source.has_contents() {
        target.set_symbol(source.contents());
    }
    let mut modifier = Modifier::empty();
    if source.bold() {
        modifier |= Modifier::BOLD;
    }
    if source.dim() {
        modifier |= Modifier::DIM;
    }
    if source.italic() {
        modifier |= Modifier::ITALIC;
    }
    if source.underline() {
        modifier |= Modifier::UNDERLINED;
    }
    if source.inverse() {
        modifier |= Modifier::REVERSED;
    }
    target.set_style(
        Style::reset()
            .fg(map_color(source.fgcolor()))
            .bg(map_color(source.bgcolor()))
            .add_modifier(modifier),
    );
}

/// vt100 colors map one to one onto ratatui colors.
fn map_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
