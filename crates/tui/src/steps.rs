//! The step block of `daemon stop` and `daemon restart`: the devlanes logo
//! on the left and, beside it, a title and one row per step of the
//! [`Steps`] model, each with its glyph, text, detail and time.
//!
//! The CLI prints it inline in the shell ([`render`], redrawn in place by
//! [`Live`]) next to where the [`exit_summary`](crate::exit_summary) prints,
//! with the same indent and colors; the TUI's "daemon unavailable" screen
//! draws it into its own area with [`draw`].

use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

use crossterm::{
    Command,
    cursor::MoveToPreviousLine,
    terminal::{Clear, ClearType},
};
use devlanes_client::restart::{Row, RowState, Steps};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

use crate::{
    ansi, banner, doctor,
    exit_summary::INDENT,
    text,
    theme::{Glyph, Glyphs, Theme, Tone},
};

/// Columns between the logo and the text beside it.
const GAP: u16 = 2;
/// Columns the text needs beside the logo; a narrower area drops the logo.
const MIN_TEXT: u16 = 20;

/// How the logo bars are colored: as usual, grey while the daemon is in
/// transition, red while it is down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogoState {
    Online,
    Busy,
    Down,
}

/// The style of logo bar `index` (0 = top) in `state`; also used for the
/// TUI header's brand word. Without colors only `Busy` shows, dimmed.
pub fn logo_style(theme: &Theme, state: LogoState, index: usize) -> Style {
    match state {
        LogoState::Online => banner::bar(theme, index == 0),
        LogoState::Busy => theme.rule(),
        LogoState::Down => theme.tone(Tone::Failure),
    }
}

/// Rows the block needs for `steps`: at least the logo's 3, else 1 title row
/// plus one per step row.
pub fn height(steps: &Steps) -> u16 {
    rows(steps).max(banner::logo_size().1)
}

/// The title row plus one per step row.
fn rows(steps: &Steps) -> u16 {
    u16::try_from(steps.rows().len())
        .unwrap_or(u16::MAX)
        .saturating_add(1)
}

/// Draws the logo, `title` and the step rows into `area` of `buf`. `now`
/// gives running rows their live time; `spin` is the spinner frame. Rows
/// that do not fit `area` are left out; long text ends in an ellipsis.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    logo: LogoState,
    title: Line<'static>,
    steps: &Steps,
    now: Instant,
    spin: u64,
) {
    let area = area.intersection(buf.area);
    let ascii = theme.glyphs == Glyphs::Ascii;
    // ASCII has no bars: the word stands in for them, as in the exit summary.
    let (logo_width, _) = if ascii {
        banner::Size::Plain.dimensions()
    } else {
        banner::logo_size()
    };
    let mut left = 0;
    if area.width >= logo_width + GAP + MIN_TEXT {
        if ascii {
            banner::word(buf, area, |x| word_style(theme, logo, x));
        } else {
            banner::logo(buf, area, |index| logo_style(theme, logo, index));
        }
        left = logo_width + GAP;
    }
    let width = area.width - left;
    let columns = usize::from(width);
    let title = Line::from(clip(title.spans, columns)).style(title.style);
    let rows = steps
        .rows()
        .iter()
        .map(|row| row_line(theme, row, now, spin, columns));
    for (y, line) in (area.y..area.bottom()).zip(std::iter::once(title).chain(rows)) {
        buf.set_line(area.x + left, y, &line, width);
    }
}

/// The block as text with SGR colors, for a terminal `width` columns wide
/// (no surrounding blank lines). The last column stays empty: a line that
/// fills it wraps early on some terminals, which would throw [`Live`]'s
/// count of lines to erase off.
pub fn render(
    theme: &Theme,
    width: u16,
    logo: LogoState,
    title: Line<'static>,
    steps: &Steps,
    now: Instant,
    spin: u64,
) -> String {
    let rows = if theme.glyphs == Glyphs::Ascii {
        rows(steps)
    } else {
        height(steps)
    };
    let columns = width.saturating_sub(1);
    let mut buf = Buffer::empty(Rect::new(0, 0, columns, rows));
    let area = Rect::new(INDENT, 0, columns.saturating_sub(INDENT), rows);
    draw(&mut buf, area, theme, logo, title, steps, now, spin);
    ansi::text(&buf)
}

/// Redraws the block in place on a terminal: `update` erases what the
/// previous call printed (cursor up + clear) and prints the new block;
/// `finish` prints it a last time and leaves it on screen. Writes to the
/// given writer so tests can capture it.
pub struct Live<W: Write> {
    out: W,
    theme: Theme,
    width: u16,
    /// Lines the last update printed, which the next one erases.
    printed: u16,
}

impl<W: Write> Live<W> {
    /// A block that prints to `out`, a terminal `width` columns wide.
    pub fn new(out: W, theme: Theme, width: u16) -> Self {
        Self {
            out,
            theme,
            width,
            printed: 0,
        }
    }

    /// Replaces the block printed last with this one.
    pub fn update(
        &mut self,
        logo: LogoState,
        title: Line<'static>,
        steps: &Steps,
        now: Instant,
        spin: u64,
    ) -> io::Result<()> {
        let block = render(&self.theme, self.width, logo, title, steps, now, spin);
        let mut bytes = String::new();
        if self.printed > 0 {
            let _ = MoveToPreviousLine(self.printed).write_ansi(&mut bytes);
            let _ = Clear(ClearType::FromCursorDown).write_ansi(&mut bytes);
        }
        bytes.push_str(&block);
        // One write per frame, so the terminal never shows the block half erased.
        self.out.write_all(bytes.as_bytes())?;
        self.out.flush()?;
        self.printed = u16::try_from(block.matches('\n').count()).unwrap_or(u16::MAX);
        Ok(())
    }

    /// Replaces the block a last time and leaves it on screen: a later
    /// [`Live::update`] prints below it.
    pub fn finish(
        &mut self,
        logo: LogoState,
        title: Line<'static>,
        steps: &Steps,
        now: Instant,
        spin: u64,
    ) -> io::Result<()> {
        self.update(logo, title, steps, now, spin)?;
        self.printed = 0;
        Ok(())
    }
}

/// One plain line per finished row, for output that is not a terminal
/// (`✓ stopped 26 processes in 1 lane 5.8s`), ASCII glyphs when the theme
/// says so. A row still running has no time yet.
pub fn plain_line(theme: &Theme, row: &Row) -> String {
    let glyph = match row.state {
        RowState::Running => theme.glyph(Glyph::Running),
        RowState::Done => theme.glyph(Glyph::Done),
        RowState::Failed => theme.glyph(Glyph::Failed),
    };
    let mut line = format!("{glyph} {}", text::clean(&row.text));
    if let Some(detail) = detail(row) {
        line.push_str(", ");
        line.push_str(&detail);
    }
    if let Some(elapsed) = row.elapsed {
        line.push(' ');
        line.push_str(&time(elapsed));
    }
    line
}

/// A step's time: one decimal under 10 s (`5.8s`), else `12s`, `2m 3s` or
/// `1h 5m` like the daemon's uptime.
pub fn time(elapsed: Duration) -> String {
    let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
    let tenths = ms / 100;
    if tenths < 100 {
        format!("{}.{}s", tenths / 10, tenths % 10)
    } else {
        doctor::uptime(ms)
    }
}

/// The word that stands in for the logo: in the title gradient while the
/// daemon is up, else in the bars' color.
fn word_style(theme: &Theme, state: LogoState, x: usize) -> Style {
    match state {
        LogoState::Online => banner::gradient(theme, x as f64 / 7.0),
        LogoState::Busy | LogoState::Down => {
            logo_style(theme, state, 0).add_modifier(Modifier::BOLD)
        }
    }
}

/// The row's detail, cleaned, when it says something.
fn detail(row: &Row) -> Option<String> {
    row.detail
        .as_deref()
        .map(text::clean)
        .filter(|detail| !detail.trim().is_empty())
}

/// One step row within `width` columns: glyph, text, detail, then the time,
/// which is kept whole when the text has to be cut.
fn row_line(theme: &Theme, row: &Row, now: Instant, spin: u64, width: usize) -> Line<'static> {
    let (glyph, glyph_style) = match row.state {
        RowState::Running => (theme.spinner(spin), theme.tone(Tone::Busy)),
        RowState::Done => (theme.glyph(Glyph::Done), theme.tone(Tone::Ok)),
        RowState::Failed => (theme.glyph(Glyph::Failed), theme.tone(Tone::Failure)),
    };
    let elapsed = row
        .elapsed
        .unwrap_or_else(|| now.saturating_duration_since(row.started));
    let time = time(elapsed);
    let mut spans = vec![
        Span::styled(glyph, glyph_style),
        Span::raw(" "),
        Span::raw(text::clean(&row.text)),
    ];
    if let Some(detail) = detail(row) {
        // A failure's detail is its reason, which must not read as secondary.
        let style = match row.state {
            RowState::Failed => theme.tone(Tone::Failure),
            RowState::Running | RowState::Done => theme.quiet(),
        };
        spans.push(Span::styled(", ", theme.quiet()));
        spans.push(Span::styled(detail, style));
    }
    let mut spans = clip(spans, width.saturating_sub(text::width(&time) + 1));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(time, theme.quiet()));
    Line::from(spans)
}

/// Fits `spans` into `max` columns; the span where they are cut ends in `…`
/// in its own style.
fn clip(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    if spans.iter().map(Span::width).sum::<usize>() <= max {
        return spans;
    }
    let mut out = Vec::new();
    let mut left = max;
    for span in spans {
        let width = span.width();
        // Strictly less: the cut needs one column left for the ellipsis.
        if width < left {
            left -= width;
            out.push(span);
            continue;
        }
        if left > 0 {
            let mut kept = String::new();
            let mut used = 0;
            for c in span.content.chars() {
                let w = c.width().unwrap_or(0);
                if used + w > left - 1 {
                    break;
                }
                used += w;
                kept.push(c);
            }
            kept.push('…');
            out.push(Span::styled(kept, span.style));
        }
        break;
    }
    out
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;
    use crate::theme::Tier;

    fn theme(tier: Tier, glyphs: Glyphs) -> Theme {
        Theme { tier, glyphs }
    }

    /// `ms` milliseconds after `begun`.
    fn at(begun: Instant, ms: u64) -> Instant {
        begun + Duration::from_millis(ms)
    }

    /// "devlanes is restarting" and the version after `separator`.
    fn title(separator: &str) -> Line<'static> {
        Line::from(vec![
            Span::styled(
                "devlanes is restarting",
                Style::new().add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{separator}0.6.1"), Theme::default().quiet()),
        ])
    }

    /// A restart three seconds into stopping its processes.
    fn stopping(begun: Instant) -> Steps {
        let mut steps = Steps::default();
        steps.set(
            "units",
            "stopping 26 processes in 1 lane".into(),
            RowState::Running,
            Some("4 left".into()),
            begun,
        );
        steps
    }

    /// A restart that went through: processes stopped, state saved, new
    /// daemon up.
    fn restarted(begun: Instant) -> Steps {
        let mut steps = stopping(begun);
        let mut step = |key: &str, running: &str, done: &str, detail: Option<&str>, from, to| {
            steps.set(
                key,
                running.into(),
                RowState::Running,
                None,
                at(begun, from),
            );
            steps.set(
                key,
                done.into(),
                RowState::Done,
                detail.map(Into::into),
                at(begun, to),
            );
        };
        step(
            "units",
            "stopping 26 processes in 1 lane",
            "stopped 26 processes in 1 lane",
            None,
            0,
            5800,
        );
        step("state", "saving state", "saved state", None, 5800, 5900);
        step(
            "start",
            "starting the new daemon",
            "started devlanes 0.6.1",
            Some("pid 4242"),
            5900,
            6000,
        );
        steps
    }

    /// The block without its SGR sequences.
    fn text(
        theme: &Theme,
        width: u16,
        title: Line<'static>,
        steps: &Steps,
        now: Instant,
    ) -> Vec<String> {
        let rendered = render(theme, width, LogoState::Busy, title, steps, now, 2);
        let plain = strip_ansi_escapes::strip_str(&rendered);
        plain.lines().map(str::to_owned).collect()
    }

    #[test]
    fn a_restart_in_progress_shows_the_running_row_with_its_live_time() {
        let begun = Instant::now();
        let rows = text(
            &theme(Tier::Ansi, Glyphs::Unicode),
            80,
            title(" · "),
            &stopping(begun),
            at(begun, 3000),
        );
        assert_eq!(
            rows,
            [
                "  ▀▀▀▀▀    devlanes is restarting · 0.6.1",
                "   ▀▀▀▀▀   ⠹ stopping 26 processes in 1 lane, 4 left 3.0s",
                "    ▀▀▀▀▀",
            ]
        );
    }

    #[test]
    fn a_finished_restart_grows_below_the_logo() {
        let begun = Instant::now();
        let rows = text(
            &theme(Tier::Ansi, Glyphs::Unicode),
            80,
            title(" · "),
            &restarted(begun),
            at(begun, 9000),
        );
        assert_eq!(
            rows,
            [
                "  ▀▀▀▀▀    devlanes is restarting · 0.6.1",
                "   ▀▀▀▀▀   ✓ stopped 26 processes in 1 lane 5.8s",
                "    ▀▀▀▀▀  ✓ saved state 0.1s",
                "           ✓ started devlanes 0.6.1, pid 4242 0.1s",
            ]
        );
    }

    #[test]
    fn ascii_puts_the_word_in_place_of_the_bars() {
        let begun = Instant::now();
        let ascii = theme(Tier::Plain, Glyphs::Ascii);
        assert_eq!(
            text(&ascii, 80, title(" "), &stopping(begun), at(begun, 12_000)),
            [
                "  devlanes  devlanes is restarting 0.6.1",
                "            | stopping 26 processes in 1 lane, 4 left 12s",
            ]
        );
        let mut steps = restarted(begun);
        steps.set(
            "lane",
            "starting acme@0".into(),
            RowState::Running,
            None,
            at(begun, 6000),
        );
        steps.fail("FRAMEWORK_FAILED", at(begun, 7000));
        assert_eq!(
            text(&ascii, 80, title(" "), &steps, at(begun, 9000)),
            [
                "  devlanes  devlanes is restarting 0.6.1",
                "            + stopped 26 processes in 1 lane 5.8s",
                "            + saved state 0.1s",
                "            + started devlanes 0.6.1, pid 4242 0.1s",
                "            x starting acme@0, FRAMEWORK_FAILED 1.0s",
            ]
        );
    }

    #[test]
    fn long_rows_end_in_an_ellipsis_and_keep_their_time() {
        let begun = Instant::now();
        let rows = text(
            &theme(Tier::Ansi, Glyphs::Unicode),
            40,
            title(" · "),
            &stopping(begun),
            at(begun, 3000),
        );
        assert_eq!(rows[0], "  ▀▀▀▀▀    devlanes is restarting · 0.…");
        assert_eq!(rows[1], "   ▀▀▀▀▀   ⠹ stopping 26 processe… 3.0s");
        for row in &rows {
            assert!(crate::text::width(row) < 40, "{row:?}");
        }
    }

    #[test]
    fn the_block_is_as_tall_as_the_logo_or_its_rows() {
        let begun = Instant::now();
        assert_eq!(height(&Steps::default()), 3);
        assert_eq!(height(&stopping(begun)), 3);
        assert_eq!(height(&restarted(begun)), 4);
    }

    #[test]
    fn colors_end_with_a_reset() {
        let begun = Instant::now();
        let rendered = render(
            &theme(Tier::Ansi, Glyphs::Unicode),
            80,
            LogoState::Down,
            title(" · "),
            &restarted(begun),
            at(begun, 9000),
            0,
        );
        assert!(rendered.contains('\x1b'), "{rendered:?}");
        for line in rendered.lines().filter(|line| line.contains('\x1b')) {
            assert!(line.ends_with("\x1b[0m"), "{line:?}");
        }
    }

    #[test]
    fn the_logo_takes_the_daemon_state_color() {
        let ansi = theme(Tier::Ansi, Glyphs::Unicode);
        assert_eq!(
            logo_style(&ansi, LogoState::Online, 0).fg,
            Some(Color::Yellow)
        );
        assert_eq!(
            logo_style(&ansi, LogoState::Online, 2).fg,
            Some(Color::Gray)
        );
        assert_eq!(
            logo_style(&ansi, LogoState::Busy, 0).fg,
            Some(Color::DarkGray)
        );
        assert_eq!(logo_style(&ansi, LogoState::Down, 1).fg, Some(Color::Red));
        let plain = theme(Tier::Plain, Glyphs::Unicode);
        assert_eq!(logo_style(&plain, LogoState::Down, 0), Style::new());
    }

    #[test]
    fn draw_stays_inside_its_area() {
        let begun = Instant::now();
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 8));
        let area = Rect::new(4, 2, 50, 4);
        draw(
            &mut buf,
            area,
            &theme(Tier::Ansi, Glyphs::Unicode),
            LogoState::Online,
            title(" · "),
            &restarted(begun),
            at(begun, 9000),
            0,
        );
        let row = |y: u16| -> String { (0..60).map(|x| buf[(x, y)].symbol()).collect() };
        assert_eq!(row(1).trim(), "");
        assert_eq!(row(6).trim(), "");
        assert!(row(2).starts_with("    ▀▀▀▀▀    devlanes"), "{:?}", row(2));
        assert!(row(5).starts_with("             ✓ started"), "{:?}", row(5));
        assert_eq!(buf[(4, 2)].fg, Color::Yellow);
    }

    #[test]
    fn live_erases_the_previous_block_and_leaves_the_last() {
        let begun = Instant::now();
        let theme = theme(Tier::Ansi, Glyphs::Unicode);
        let mut out = Vec::new();
        let mut live = Live::new(&mut out, theme, 80);
        let steps = stopping(begun);
        live.update(LogoState::Busy, title(" · "), &steps, at(begun, 100), 0)
            .expect("first frame");
        live.update(LogoState::Busy, title(" · "), &steps, at(begun, 200), 1)
            .expect("second frame");
        let done = restarted(begun);
        live.finish(LogoState::Online, title(" · "), &done, at(begun, 9000), 2)
            .expect("last frame");
        let written = String::from_utf8(out).expect("UTF-8");
        let erase = "\x1b[3F\x1b[J";
        let frames: Vec<&str> = written.split(erase).collect();
        assert_eq!(frames.len(), 3, "{written:?}");
        assert!(!frames[0].contains("\x1b[J"), "{written:?}");
        let last = render(
            &theme,
            80,
            LogoState::Online,
            title(" · "),
            &done,
            at(begun, 9000),
            2,
        );
        assert_eq!(frames[2], last);
    }

    #[test]
    fn finish_leaves_the_block_for_what_comes_after() {
        let begun = Instant::now();
        let mut out = Vec::new();
        let mut live = Live::new(&mut out, theme(Tier::Plain, Glyphs::Ascii), 80);
        let steps = stopping(begun);
        live.finish(LogoState::Busy, title(" "), &steps, begun, 0)
            .expect("last frame");
        live.update(LogoState::Busy, title(" "), &steps, begun, 0)
            .expect("next block");
        let written = String::from_utf8(out).expect("UTF-8");
        assert!(!written.contains("\x1b[J"), "{written:?}");
    }

    #[test]
    fn plain_lines_have_the_glyph_text_and_time() {
        let begun = Instant::now();
        let steps = restarted(begun);
        let unicode = theme(Tier::Ansi, Glyphs::Unicode);
        assert_eq!(
            plain_line(&unicode, &steps.rows()[0]),
            "✓ stopped 26 processes in 1 lane 5.8s"
        );
        let ascii = theme(Tier::Plain, Glyphs::Ascii);
        assert_eq!(
            plain_line(&ascii, &steps.rows()[2]),
            "+ started devlanes 0.6.1, pid 4242 0.1s"
        );
        let mut failed = stopping(begun);
        failed.fail("timed out", at(begun, 1000));
        assert_eq!(
            plain_line(&ascii, &failed.rows()[0]),
            "x stopping 26 processes in 1 lane, timed out 1.0s"
        );
        assert_eq!(
            plain_line(&ascii, &stopping(begun).rows()[0]),
            "o stopping 26 processes in 1 lane, 4 left"
        );
    }

    #[test]
    fn times_read_like_the_rest_of_devlanes() {
        let ms = Duration::from_millis;
        assert_eq!(time(ms(0)), "0.0s");
        assert_eq!(time(ms(5_849)), "5.8s");
        assert_eq!(time(ms(9_999)), "9.9s");
        assert_eq!(time(ms(12_400)), "12s");
        assert_eq!(time(ms(123_000)), "2m 3s");
    }
}
