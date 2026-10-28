//! Rendering. Reads [`App`] and draws; the only state it writes is the map of
//! clickable chrome ([`App::hits`]), so hit tests match what was drawn.
//!
//! Every string from the daemon is cleaned and fitted to its widget before it
//! reaches the buffer (see [`crate::text`]), so long or hostile text cannot
//! spill into another widget or move the real terminal's cursor.

mod boxes;
mod connect;
pub(crate) mod daemon;
mod dialog;
mod list;
mod monitor;
mod pane;
mod unavailable;

use crossterm::event::{KeyCode, KeyModifiers};
use devlanes_proto::{LaneSummary, ORPHANED, UnitSummary};
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{
    App, AttachState, Click, Connection, DaemonState, Focus, MIN_HEIGHT, MIN_WIDTH, PaneMode,
    Target, View,
};
use crate::steps::LogoState;
use crate::text::{self, clean, clip};
use crate::theme::{Glyph, Theme, Tier, Tone};
use crate::tree::{self, Role, RowId};

pub use list::lane_summary;
pub use monitor::format_bytes;

/// Draws one frame.
pub fn render(app: &App, frame: &mut Frame) {
    app.hits.borrow_mut().clear();
    *app.dialog_area.borrow_mut() = None;
    let size = frame.area();
    let Some(layout) = app.areas_sized(size.width, size.height) else {
        render_too_small(frame, size);
        return;
    };
    render_header(app, frame, layout.header);
    // Nothing switches views: the old one draws again once the daemon is back.
    match app.view {
        _ if app.unavailable() => unavailable::render(app, frame, layout.body),
        View::Project => {
            if app.lanes.is_empty() && app.boxes.lanes.is_empty() {
                render_welcome(app, frame, layout.body);
            } else {
                if let Some(bar) = layout.lane_bar {
                    render_lane_bar(app, frame, bar);
                }
                if let (Some(boxed), Some(rows)) = (layout.list_box, layout.list) {
                    list::render(app, frame, boxed, rows);
                }
                pane::render(app, frame, layout.pane_box, layout.pane_inner);
            }
        }
        View::Monitor => monitor::render(app, frame, layout.body),
        View::Daemon => daemon::render(app, frame, layout.body),
        View::Boxes => boxes::render(app, frame, layout.body),
    }
    render_footer(app, frame, layout.footer);
    if let Some(dialog) = &app.dialog {
        dialog::render(app, dialog, frame, size);
    }
}

/// Draws a box with `left` in its top border and `right` at the end of it,
/// as the navigation.md mockups draw the list and the pane:
/// `┌ main @0 ───┐`. The border takes the focus color when `accent`.
pub(crate) fn draw_box(
    app: &App,
    frame: &mut Frame,
    area: Rect,
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    accent: bool,
) {
    use ratatui::{symbols::border, widgets::Block};
    let theme = &app.theme;
    let set = if theme.glyphs == crate::theme::Glyphs::Ascii {
        border::Set {
            top_left: "+",
            top_right: "+",
            bottom_left: "+",
            bottom_right: "+",
            vertical_left: "|",
            vertical_right: "|",
            horizontal_top: "-",
            horizontal_bottom: "-",
        }
    } else {
        border::PLAIN
    };
    let style = if accent { theme.focus() } else { theme.rule() };
    // Titles are drawn over the border's cells; without their own color
    // they would take the border's gray.
    let own = |spans: Vec<Span<'static>>| -> Vec<Span<'static>> {
        spans
            .into_iter()
            .map(|span| {
                let style = if span.style.fg.is_none() {
                    span.style.fg(ratatui::style::Color::Reset)
                } else {
                    span.style
                };
                Span::styled(span.content, style)
            })
            .collect()
    };
    let (left, right) = (own(left), own(right));
    // The right title drops before the left one is cut, and the left one
    // never runs into the corner.
    let room = usize::from(area.width).saturating_sub(4);
    let right_width = spans_width(&right);
    let fits_right = right_width > 0 && spans_width(&left) + right_width + 2 <= room;
    let left = fit(left, room);
    let mut block = Block::bordered()
        .border_set(set)
        .border_style(style)
        .title_top(left);
    // The right title drops before the left one is cut.
    if fits_right {
        block = block.title_top(Line::from(right).right_aligned());
    }
    frame.render_widget(block, area);
}

/// Records a clickable rectangle for the mouse.
fn hit(app: &App, rect: Rect, click: Click) {
    if rect.width > 0 && rect.height > 0 {
        app.hits.borrow_mut().push((rect, click));
    }
}

/// Cuts styled spans to `width` cells, ending in `…` where text was cut.
pub(crate) fn fit(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut out = Vec::new();
    let mut left = width;
    for span in spans {
        if left == 0 {
            break;
        }
        let content = span.content.to_string();
        let span_width = text::width(&content);
        if span_width <= left {
            left -= span_width;
            out.push(span);
        } else {
            out.push(Span::styled(clip(&content, left), span.style));
            break;
        }
    }
    Line::from(out)
}

/// Spans on the left and right of one row of `width` cells; the left side
/// is cut first.
pub(crate) fn spread(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Line<'static> {
    let right_width: usize = right.iter().map(|s| text::width(&s.content)).sum();
    if right_width + 1 >= width {
        return fit(left, width);
    }
    let left = fit(left, width - right_width - 1);
    let gap = width - left.width() - right_width;
    let mut spans = left.spans;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

/// Total width of spans.
pub(crate) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| text::width(&s.content)).sum()
}

/// Shown instead of the layout below the minimum size.
fn render_too_small(frame: &mut Frame, area: Rect) {
    let width = usize::from(area.width);
    let text = vec![
        Line::from(clip(
            &format!("terminal is {}x{}", area.width, area.height),
            width,
        )),
        Line::from(clip(
            &format!("devlanes needs {MIN_WIDTH}x{MIN_HEIGHT} or larger"),
            width,
        ))
        .style(Style::new().add_modifier(Modifier::DIM)),
    ];
    let top = area.height.saturating_sub(2) / 2;
    let area = Rect::new(area.x, area.y + top, area.width, area.height - top);
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), area);
}

/// The look of a unit: glyph, its tone, the state word, the word's tone and
/// the detail column.
pub(crate) struct Look {
    pub glyph: &'static str,
    pub tone: Tone,
    pub word: String,
    pub word_tone: Tone,
    pub detail: String,
}

/// `exit 3` for the detail column, or the CLI's words for a code that has
/// them (a Windows Ctrl-C exit).
fn exit_words(code: i32) -> String {
    devlanes_cli::exit::words(code.into()).map_or_else(|| format!("exit {code}"), str::to_owned)
}

/// How a unit's state is drawn, by its role (visual.md's glyph table).
pub(crate) fn unit_look(theme: &Theme, unit: &UnitSummary, spin: u64) -> Look {
    let task = matches!(tree::role(unit), Role::Task | Role::Setup | Role::Teardown);
    let exit = unit.exit_code.map(exit_words);
    let look = |glyph, tone, word: &str, word_tone, detail: Option<String>| Look {
        glyph,
        tone,
        word: word.into(),
        word_tone,
        detail: detail.unwrap_or_default(),
    };
    let spinner = theme.spinner(spin);
    match (unit.state.as_str(), task) {
        // An idle lane's units still serve; only the shape and word change.
        ("ready", false) if unit.shown_state() == "frozen" => look(
            theme.glyph(Glyph::Frozen),
            Tone::Frozen,
            "frozen",
            Tone::Quiet,
            None,
        ),
        ("ready", false) if unit.shown_state() == "throttled" => look(
            theme.glyph(Glyph::Throttled),
            Tone::Quiet,
            "throttled",
            Tone::Quiet,
            None,
        ),
        // A ready check passed (`●`), or the process merely runs (`◉`): the
        // shapes differ so 16 colors and color-blind eyes tell them apart.
        ("ready" | "running", false) => look(
            theme.glyph(if unit.shown_state() == "ready" {
                Glyph::Ready
            } else {
                Glyph::Running
            }),
            Tone::Ok,
            unit.shown_state(),
            Tone::Quiet,
            None,
        ),
        ("running", true) => look(spinner, Tone::Busy, "running", Tone::Busy, None),
        ("starting", _) => look(spinner, Tone::Busy, "starting", Tone::Busy, None),
        ("stopping", _) => look(spinner, Tone::Quiet, "stopping", Tone::Quiet, None),
        ("queued", _) => look(
            theme.glyph(Glyph::Queued),
            Tone::Quiet,
            "queued",
            Tone::Quiet,
            queued_detail(unit),
        ),
        ("failed", _) => look(
            theme.glyph(Glyph::Failed),
            Tone::Failure,
            "failed",
            Tone::Failure,
            exit,
        ),
        ("exited", true) if unit.exit_code == Some(0) => look(
            theme.glyph(Glyph::Done),
            Tone::Ok,
            "done",
            Tone::Quiet,
            unit.finished_at_ms.map(ago),
        ),
        ("exited", true) if unit.exit_code.is_some() => look(
            theme.glyph(Glyph::Failed),
            Tone::Failure,
            "failed",
            Tone::Failure,
            exit,
        ),
        ("exited", true) => look(
            theme.glyph(Glyph::Exited),
            Tone::Quiet,
            "ended",
            Tone::Quiet,
            None,
        ),
        ("exited", false) => look(
            theme.glyph(Glyph::Exited),
            Tone::Quiet,
            "exited",
            Tone::Quiet,
            exit,
        ),
        (ORPHANED, _) => look(
            theme.glyph(Glyph::Orphaned),
            Tone::Attention,
            "orphaned",
            Tone::Attention,
            unit.pid.map(|pid| format!("pid {pid}")),
        ),
        // devlanes runs nothing for it and never checks whether it answers.
        (devlanes_proto::EXTERNAL_KIND, _) => look(
            theme.glyph(Glyph::Stopped),
            Tone::Quiet,
            "external",
            Tone::Quiet,
            None,
        ),
        (_, true) => look(
            theme.glyph(Glyph::Stopped),
            Tone::Quiet,
            "not run",
            Tone::Quiet,
            None,
        ),
        (_, false) => look(
            theme.glyph(Glyph::Stopped),
            Tone::Quiet,
            "stopped",
            Tone::Quiet,
            None,
        ),
    }
}

/// What a queued unit waits for, from its `queued` reason: `waiting for
/// migrate-db` while its lane comes up, `3rd in line` in the start gate,
/// `held: memory` while the gate waits for memory. `None` without a reason,
/// as on a row this TUI marked `queued` before the daemon answered.
pub(crate) fn queued_detail(unit: &UnitSummary) -> Option<String> {
    let data = &unit.reason.as_ref().filter(|r| r.code == "queued")?.data;
    let text = |key: &str| data.get(key).and_then(serde_json::Value::as_str);
    if let Some(step) = text("waiting_for") {
        return Some(match step {
            "lane" => "waiting for the lane".into(),
            step => format!("waiting for {}", clean(step)),
        });
    }
    let mut parts = Vec::new();
    match data.get("position").and_then(serde_json::Value::as_u64) {
        Some(1) => parts.push("next in line".to_owned()),
        Some(position) => parts.push(format!("{} in line", ordinal(position))),
        None => {}
    }
    if matches!(text("hold"), Some("memory" | "memory_critical")) {
        parts.push("held: memory".into());
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// `2nd`, `3rd`, `11th`, `22nd`.
fn ordinal(n: u64) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A task that is not a unit now, from its last stored run in `task.list`.
pub(crate) fn task_look(theme: &Theme, info: Option<&crate::api::TaskInfo>, spin: u64) -> Look {
    let run = info.and_then(|i| i.last_run.as_ref());
    let ago = run
        .and_then(|r| r.finished_at_ms)
        .map(ago)
        .unwrap_or_default();
    let look = |glyph, tone, word: &str, word_tone, detail: String| Look {
        glyph,
        tone,
        word: word.into(),
        word_tone,
        detail,
    };
    match run.map(|r| r.state.as_str()) {
        Some("running") => look(
            theme.spinner(spin),
            Tone::Busy,
            "running",
            Tone::Busy,
            String::new(),
        ),
        Some("done") => look(theme.glyph(Glyph::Done), Tone::Ok, "done", Tone::Quiet, ago),
        Some("failed") => {
            let exit = run
                .and_then(|r| r.exit_code)
                .map(exit_words)
                .unwrap_or_default();
            look(
                theme.glyph(Glyph::Failed),
                Tone::Failure,
                "failed",
                Tone::Failure,
                exit,
            )
        }
        Some(_) => look(
            theme.glyph(Glyph::Exited),
            Tone::Quiet,
            "ended",
            Tone::Quiet,
            ago,
        ),
        None => look(
            theme.glyph(Glyph::Stopped),
            Tone::Quiet,
            "not run",
            Tone::Quiet,
            String::new(),
        ),
    }
}

/// `3m ago` for a Unix millisecond time.
pub(crate) fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let seconds = now.saturating_sub(ms) / 1000;
    match seconds {
        0..60 => "just now".into(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86_400 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

/// The state word with its detail, for panes and tables (`failed  exit 1`).
pub fn unit_state(unit: &UnitSummary) -> String {
    let look = unit_look(&Theme::default(), unit, 0);
    if look.detail.is_empty() || look.detail.starts_with(':') {
        look.word
    } else {
        format!("{}  {}", look.word, look.detail)
    }
}

/// The glyph and tone for a lane in tabs and chips.
pub(crate) fn lane_glyph(theme: &Theme, lane: &LaneSummary, spin: u64) -> (&'static str, Tone) {
    let missing = lane
        .reason
        .as_ref()
        .is_some_and(|reason| reason.code == "worktree_missing");
    let power = lane.power.as_ref().map(|power| power.level.as_str());
    match lane.state.as_str() {
        _ if missing => (theme.glyph(Glyph::Attention), Tone::Attention),
        "running" if power == Some("frozen") => (theme.glyph(Glyph::Frozen), Tone::Frozen),
        // Dim, as on its units' rows; the `z` shape tells it from a stopped lane.
        "running" if power == Some("throttled") => (theme.glyph(Glyph::Throttled), Tone::Quiet),
        "running" => (theme.glyph(Glyph::Ready), Tone::Ok),
        "starting" | "stopping" => (theme.spinner(spin), Tone::Busy),
        "failed" => (theme.glyph(Glyph::Failed), Tone::Failure),
        "no-config" => (theme.glyph(Glyph::Attention), Tone::Attention),
        _ => (theme.glyph(Glyph::Stopped), Tone::Quiet),
    }
}

/// How bad a lane state is, for a project tab's glyph: the worst lane wins.
fn severity(lane: &LaneSummary) -> u8 {
    match lane_glyph(&Theme::default(), lane, 0).1 {
        Tone::Failure => 4,
        Tone::Attention => 3,
        Tone::Busy => 2,
        Tone::Ok | Tone::Frozen => 1,
        // A throttled lane is dim like a stopped one, yet it runs.
        Tone::Quiet if lane.power.is_some() => 1,
        Tone::Quiet => 0,
    }
}

/// ` devlanes   1 acme ●   2 shop ·` on the left, the message, then
/// `Monitor  Daemon  Boxes` on the right, each label's first letter its key.
fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let width = usize::from(area.width);
    let tabs = project_tabs(app);
    let message = header_message(app);
    // What the left side needs in full: the brand, every tab, the shown
    // lane's chip, and the message up to 32 columns.
    let wanted = 9
        + tabs.iter().map(|(_, s)| spans_width(s) + 1).sum::<usize>()
        + 2
        + shown_chip_width(app)
        + message
            .as_ref()
            .map_or(0, |(spans, _)| spans_width(spans).min(32) + 4);
    // The view buttons, right-aligned: in full when they fit next to that,
    // else Monitor, then Daemon, then Boxes drop to their key letter.
    let buttons = (0..=3)
        .map(|letters| view_buttons(app, letters))
        .find(|buttons| wanted + buttons_width(buttons) <= width)
        .unwrap_or_else(|| view_buttons(app, 3));
    let buttons_width = buttons_width(&buttons);
    let mut x = area.right().saturating_sub(buttons_width as u16);
    let mut right: Vec<Span> = Vec::new();
    for (index, (spans, key)) in buttons.into_iter().enumerate() {
        if index > 0 {
            right.push(Span::raw("  "));
            x += 2;
        }
        let w = spans_width(&spans) as u16;
        hit(
            app,
            Rect::new(x, area.y, w, 1),
            Click::Key(key, KeyModifiers::SHIFT),
        );
        x += w;
        right.extend(spans);
    }
    right.push(Span::raw(" "));

    // Brand, letter by letter from yellow to orange while the daemon is
    // online, else in the logo's color for its state; then project tabs.
    let mut left = vec![Span::raw(" ")];
    let brand = "devlanes";
    let logo = app.daemon_state.logo();
    left.extend(brand.chars().enumerate().map(|(index, letter)| {
        let style = match logo {
            LogoState::Online => theme.brand(index, brand.len()),
            LogoState::Busy | LogoState::Down => {
                crate::steps::logo_style(theme, logo, 0).add_modifier(Modifier::BOLD)
            }
        };
        Span::styled(letter.to_string(), style)
    }));
    let mut used = 9;
    let projects = app.projects();
    let single = projects.len() <= 1;
    let current = app.project().map(str::to_owned);
    // The message takes what is left; tabs come first and scroll to keep the
    // shown project in view.
    // Each tab pads itself; one space separates them.
    let tab_widths: Vec<usize> = tabs.iter().map(|(_, s)| spans_width(s) + 1).collect();
    // Tabs that already fold (`›`) leave a message 24 columns, so a notice
    // such as why a box lane's key did nothing still shows.
    let folds = used + tab_widths.iter().sum::<usize>() + 2 > width.saturating_sub(buttons_width);
    let reserve = match &message {
        Some((spans, _)) if folds => spans_width(spans).min(24) + 4,
        _ => 0,
    };
    let room = width.saturating_sub(buttons_width + used + reserve);
    let active = current
        .as_ref()
        .and_then(|c| projects.iter().position(|p| p == c))
        .unwrap_or(0);
    let mut first = 0;
    while first < active && tab_widths[first..=active].iter().sum::<usize>() + 2 > room {
        first += 1;
    }
    if first > 0 {
        left.push(Span::styled(" ‹", theme.quiet()));
        used += 2;
    }
    for (index, (name, spans)) in tabs.into_iter().enumerate().skip(first) {
        let w = tab_widths[index];
        if used + w + 2 > width.saturating_sub(buttons_width + reserve) {
            left.push(Span::styled(" ›", theme.quiet()));
            used += 2;
            break;
        }
        left.push(Span::raw(" "));
        hit(
            app,
            Rect::new(area.x + used as u16 + 1, area.y, (w - 1) as u16, 1),
            Click::Project(name),
        );
        left.extend(spans);
        used += w;
    }
    if single && app.view == View::Project && app.project_lanes().len() > 1 {
        // A message (a start's progress, an error) keeps up to 32 columns;
        // lanes past that fold into `+N more`.
        let message_room = message
            .as_ref()
            .map_or(0, |(spans, _)| spans_width(spans).min(32) + 4);
        let budget = width.saturating_sub(buttons_width + 2 + message_room);
        let (chips, after) = lane_chips(app, area, used, budget);
        left.extend(chips);
        used = after;
    }
    // The message, right-aligned before the buttons.
    let free = width.saturating_sub(used + buttons_width + 4);
    let mut middle = Vec::new();
    if let Some((spans, click)) = message
        && free > 4
    {
        let line = fit(spans, free);
        let w = line.width();
        middle.push(Span::raw(" ".repeat(free - w + 2)));
        if let Some(click) = click {
            let x = area.x + (used + 2 + free - w) as u16;
            hit(app, Rect::new(x, area.y, w as u16, 1), click);
        }
        middle.extend(line.spans);
        middle.push(Span::raw("  "));
    } else {
        // At least one space after a folded tab strip's `›`; the buttons'
        // trailing pad gives way for it.
        middle.push(Span::raw(
            " ".repeat(width.saturating_sub(used + buttons_width).max(1)),
        ));
    }
    let mut spans = left;
    spans.extend(middle);
    spans.extend(right);
    frame.render_widget(Paragraph::new(fit(spans, width)), area);
}

/// The width of the view buttons: two spaces between them, one after the
/// last.
fn buttons_width(buttons: &[(Vec<Span<'static>>, KeyCode)]) -> usize {
    buttons.iter().map(|(s, _)| spans_width(s)).sum::<usize>()
        + 2 * buttons.len().saturating_sub(1)
        + 1
}

/// The view buttons `Monitor  Daemon  Boxes` with their badges; the first
/// `letters` of them, unless open, show only their key letter.
fn view_buttons(app: &App, letters: usize) -> Vec<(Vec<Span<'static>>, KeyCode)> {
    let theme = &app.theme;
    let badge = daemon_badge(app);
    let boxes_badge = boxes_badge(app);
    let mut buttons: Vec<(Vec<Span<'static>>, KeyCode)> = Vec::new();
    let views = [
        ('M', "onitor", View::Monitor),
        ('D', "aemon", View::Daemon),
        ('B', "oxes", View::Boxes),
    ];
    for (index, (key, rest, view)) in views.into_iter().enumerate() {
        let compact = index < letters;
        let active = app.view == view;
        let label_style = if active {
            theme
                .focus()
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            theme.quiet()
        };
        // The key letter keeps the key color, underlined with the rest of an
        // open view's label.
        let key_style = if active {
            theme.key().add_modifier(Modifier::UNDERLINED)
        } else {
            theme.key()
        };
        let mut spans = vec![Span::styled(key.to_string(), key_style)];
        if active || !compact {
            spans.push(Span::styled(rest.to_owned(), label_style));
        }
        // `Daemon · 1 issue`, `Daemon · update`: a count alone said nothing.
        let view_badge = match view {
            View::Daemon => badge.as_ref(),
            View::Boxes => boxes_badge.as_ref(),
            _ => None,
        };
        if let Some((text, tone)) = view_badge {
            spans.push(Span::styled(format!(" · {text}"), theme.tone(*tone)));
        }
        buttons.push((spans, KeyCode::Char(key)));
    }
    buttons
}

/// Each project's tab in the top bar, by project name. One project needs
/// no tabs: its name, then its lanes on the same row.
fn project_tabs(app: &App) -> Vec<(String, Vec<Span<'static>>)> {
    let theme = &app.theme;
    let projects = app.projects();
    let single = projects.len() <= 1;
    let current = app.project().map(str::to_owned);
    projects
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let lanes: Vec<&LaneSummary> = app.all_lanes().filter(|l| l.project == *name).collect();
            let worst = lanes.iter().max_by_key(|lane| severity(lane)).copied();
            let (glyph, tone) =
                worst.map_or(("", Tone::Quiet), |lane| lane_glyph(theme, lane, app.spin));
            let active = app.view == View::Project && current.as_deref() == Some(name.as_str());
            let number = if index < 9 && !single {
                format!("{} ", index + 1)
            } else {
                String::new()
            };
            let glyph = if needs_glyph(theme) && !glyph.is_empty() {
                format!("{glyph} ")
            } else {
                String::new()
            };
            // The active tab is one yellow block; the others' names take the
            // color of their worst lane.
            let spans = if single {
                vec![Span::styled(
                    format!(" {}{glyph} ", clean(name)),
                    Style::new().add_modifier(Modifier::BOLD),
                )]
            } else if active {
                vec![Span::styled(
                    format!(" {number}{} {glyph}", clean(name)),
                    theme.tab(),
                )]
            } else {
                vec![
                    Span::raw(" "),
                    Span::styled(number, theme.quiet()),
                    Span::styled(clean(name), state_style(theme, tone)),
                    Span::raw(format!(" {glyph}")),
                ]
            };
            (name.clone(), spans)
        })
        .collect()
}

/// Failing and warning doctor checks, plus a daemon that runs another
/// version or a newer release: the count and tone of the daemon button's `!`.
fn daemon_badge(app: &App) -> Option<(String, Tone)> {
    let mut fail = 0;
    let mut warn = 0;
    if let Some(doctor) = &app.daemon.doctor {
        for check in crate::doctor::checks(doctor) {
            match check.state {
                crate::doctor::CheckState::Fail => fail += 1,
                crate::doctor::CheckState::Warn => warn += 1,
                crate::doctor::CheckState::Ok => {}
            }
        }
    }
    let update = app.release_badge().is_some();
    let issues = fail + warn;
    let mut text = match issues {
        0 if !update => return None,
        0 => "update".to_owned(),
        1 => "1 issue".to_owned(),
        n => format!("{n} issues"),
    };
    if update && issues > 0 {
        text.push_str(", update");
    }
    let tone = if fail > 0 {
        Tone::Failure
    } else {
        Tone::Attention
    };
    Some((text, tone))
}

/// Boxes whose link is down or that have warnings: the count and tone of the
/// boxes button's badge. A box only remembered, with no link, is no issue.
fn boxes_badge(app: &App) -> Option<(String, Tone)> {
    let hosts = &app.boxes.hosts;
    let down = |state: &str| {
        matches!(
            state,
            "reconnecting" | "needs_auth" | "needs_pairing" | "failed"
        )
    };
    let issues = hosts
        .iter()
        .filter(|h| down(&h.state) || !h.warnings.is_empty())
        .count();
    let text = match issues {
        0 => return None,
        1 => "1 issue".to_owned(),
        n => format!("{n} issues"),
    };
    let tone = if hosts.iter().any(|h| h.state == "failed") {
        Tone::Failure
    } else {
        Tone::Attention
    };
    Some((text, tone))
}

/// The one message the top bar shows, the first that applies.
fn header_message(app: &App) -> Option<(Vec<Span<'static>>, Option<Click>)> {
    let theme = &app.theme;
    let plain = |text: String, tone: Tone| Some((vec![Span::styled(text, theme.tone(tone))], None));
    if let Some(words) = app.daemon_state.words() {
        // `daemon restarting · stopping 26 processes in 1 lane`: the step under way.
        let (after, tone) = match &app.daemon_state {
            DaemonState::Stopped { .. } => (Some("s starts it".to_owned()), Tone::Quiet),
            DaemonState::Unreachable => (Some("retrying".to_owned()), Tone::Failure),
            state => (
                state
                    .steps()
                    .and_then(|steps| steps.running())
                    .map(|row| clean(&row.text)),
                Tone::Busy,
            ),
        };
        let text = match after {
            Some(after) => format!("daemon {words} · {after}"),
            None => format!("daemon {words}"),
        };
        return plain(text, tone);
    }
    if app.daemon.collecting {
        return plain(
            "daemon cleaning lanes with missing worktrees…".into(),
            Tone::Busy,
        );
    }
    if app.connection == Connection::Connecting && app.start_line().is_none() {
        return plain("connecting to the daemon…".into(), Tone::Busy);
    }
    if let Some(notice) = &app.notice
        && notice.error
    {
        // The header cuts the text; the click shows all of it.
        let text = format!("{} {}", theme.glyph(Glyph::Failed), notice.text);
        return Some((
            vec![Span::styled(text, theme.tone(Tone::Failure))],
            Some(Click::Errors),
        ));
    }
    if let Some((line, ended)) = app.start_line() {
        let tone = match ended {
            None => Tone::Busy,
            Some(true) => Tone::Ok,
            Some(false) => Tone::Failure,
        };
        let click = app
            .start_run
            .as_ref()
            .and_then(|run| run.lanes.first())
            .map(|lane| Click::StartLane(lane.clone()));
        return Some((vec![Span::styled(line, theme.tone(tone))], click));
    }
    if let Some(notice) = &app.notice {
        return plain(notice.text.clone(), Tone::Busy);
    }
    // A start this TUI did not send: the first starting lane of the project.
    if let Some(lane) = app
        .project_lanes()
        .into_iter()
        .find(|l| l.state == "starting")
    {
        let counts = tree::counts(lane);
        let what = lane.branch.as_deref().unwrap_or(&lane.reference);
        // Only a running phase is named: one that ended, skipped included, says nothing now.
        let step = app
            .phases
            .get(&lane.reference)
            .and_then(|phases| phases.steps.iter().rev().find(|s| s.state == "started"))
            .filter(|step| step.phase != "services");
        let text = match step {
            Some(step) => format!(
                "starting {}: {}",
                clean(what),
                crate::start::phase_words(&step.phase, step.detail.as_deref())
            ),
            None => format!(
                "starting {}: {} of {} ready",
                clean(what),
                counts.up,
                counts.total
            ),
        };
        return Some((
            vec![Span::styled(text, theme.tone(Tone::Busy))],
            Some(Click::StartLane(lane.reference.clone())),
        ));
    }
    // Problems in the project, clickable.
    let mut failed = 0;
    let mut orphaned = 0;
    for lane in app.project_lanes() {
        let counts = tree::counts(lane);
        failed += counts.failed;
        orphaned += counts.orphaned;
    }
    let mut spans = Vec::new();
    if failed > 0 {
        spans.push(Span::styled(
            format!("{} {failed} failed", theme.glyph(Glyph::Failed)),
            theme.tone(Tone::Failure),
        ));
    }
    if orphaned > 0 {
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!("{} {orphaned} orphaned", theme.glyph(Glyph::Orphaned)),
            theme.tone(Tone::Attention),
        ));
    }
    (!spans.is_empty()).then_some((spans, Some(Click::FirstProblem)))
}

/// ` lanes  1 main  2 feat/checkout 5/9` and the switch hint, under the
/// project tabs when there are several projects.
fn render_lane_bar(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let width = usize::from(area.width);
    let hint = vec![
        Span::styled("1-9", theme.key()),
        Span::styled(" or ", theme.quiet()),
        Span::styled("[ ]", theme.key()),
        Span::styled(" switch lanes ", theme.quiet()),
    ];
    let budget = width.saturating_sub(spans_width(&hint) + 2);
    let mut spans = vec![Span::styled(" lanes ", theme.quiet())];
    spans.extend(lane_chips(app, area, 7, budget).0);
    frame.render_widget(Paragraph::new(spread(spans, hint, width)), area);
}

/// The columns the shown lane's chip takes in the top bar, with its padding
/// and a `‹` before it; 0 when the bar shows project tabs instead.
fn shown_chip_width(app: &App) -> usize {
    let lanes = app.project_lanes();
    if app.projects().len() > 1 || app.view != View::Project || lanes.len() < 2 {
        return 0;
    }
    let current = app.selected.as_ref().map(|s| s.lane.as_str());
    let active = lanes
        .iter()
        .position(|lane| current == Some(lane.reference.as_str()))
        .unwrap_or(0);
    spans_width(&lane_chip(app, lanes[active], &lanes, active, false)) + 4
}

/// The project's lane chips from column `used` of `area`, up to column
/// `budget`: `1 main  2 feat/checkout`, the shown lane highlighted, `+N more`
/// for those that do not fit. Like the project tabs, they scroll (`‹`) to
/// keep the shown lane in view. Registers each chip's click; returns the
/// spans and the column after them.
fn lane_chips(
    app: &App,
    area: Rect,
    mut used: usize,
    budget: usize,
) -> (Vec<Span<'static>>, usize) {
    let theme = &app.theme;
    let lanes = app.project_lanes();
    let current = app.selected.as_ref().map(|s| s.lane.as_str());
    let build = |named: bool| -> Vec<Vec<Span<'static>>> {
        lanes
            .iter()
            .enumerate()
            .map(|(index, lane)| lane_chip(app, lane, &lanes, index, named))
            .collect()
    };
    // Each chip pads itself with a space on both sides, so the highlight is
    // a block; that padding is also the gap between chips.
    let width_of = |chips: &[Vec<Span<'static>>]| -> Vec<usize> {
        chips.iter().map(|chip| spans_width(chip) + 2).collect()
    };
    // Beside box lanes, this machine's lanes keep their branch while every
    // chip fits with it.
    let mut chips = build(true);
    if width_of(&chips).iter().sum::<usize>() > budget.saturating_sub(used + 2) {
        chips = build(false);
    }
    let widths = width_of(&chips);
    let active = lanes
        .iter()
        .position(|lane| current == Some(lane.reference.as_str()))
        .unwrap_or(0);
    let mut first = 0;
    let room = budget.saturating_sub(used + 2);
    while first < active && widths[first..=active].iter().sum::<usize>() > room {
        first += 1;
    }
    let mut spans = Vec::new();
    if first > 0 {
        spans.push(Span::styled(" ‹", theme.quiet()));
        used += 2;
    }
    for (index, (lane, chip)) in lanes.iter().zip(chips).enumerate().skip(first) {
        let w = widths[index];
        let more = lanes.len() - index;
        // A chip with lanes after it leaves room for ` +N more`; those up to
        // the shown lane fit already, the scroll above saw to that.
        let reserve = if more > 1 && index > active {
            format!(" +{} more", more - 1).len()
        } else {
            0
        };
        // The shown lane always draws; a message beside the chips gives way.
        if used + w + reserve > budget && index != active {
            let text = format!(" +{more} more");
            hit(
                app,
                Rect::new(area.x + used as u16 + 1, area.y, (text.len() - 1) as u16, 1),
                Click::Key(KeyCode::Char(']'), KeyModifiers::NONE),
            );
            used += text.len();
            spans.push(Span::styled(text, theme.quiet()));
            break;
        }
        hit(
            app,
            Rect::new(area.x + used as u16, area.y, w as u16, 1),
            Click::Lane(lane.reference.clone()),
        );
        let mut chip_spans = vec![Span::raw(" ")];
        chip_spans.extend(chip);
        chip_spans.push(Span::raw(" "));
        if current == Some(lane.reference.as_str()) {
            chip_spans = chip_spans
                .into_iter()
                .map(|span| {
                    let style = theme.selected(span.style).add_modifier(Modifier::BOLD);
                    Span::styled(span.content, style)
                })
                .collect();
        }
        spans.extend(chip_spans);
        used += w;
    }
    (spans, used)
}

/// What a lane is called in the chips: an extra lane's name, else its
/// branch; `@N` only when another lane of the project has the same name.
pub(crate) fn lane_label(lane: &LaneSummary, lanes: &[&LaneSummary]) -> (String, Option<String>) {
    let label = |lane: &LaneSummary| {
        lane.name
            .clone()
            .or_else(|| lane.branch.clone())
            .map_or_else(|| "detached".to_owned(), |name| clean(&name))
    };
    let own = label(lane);
    let shared = lanes
        .iter()
        .any(|other| other.reference != lane.reference && label(other) == own);
    let number = lane
        .index
        .filter(|_| shared)
        .map(|index| format!("@{index}"));
    (own, number)
}

/// The color of a lane or project name for its state: green when running,
/// yellow while it starts or needs a look, red when it failed, dim when idle.
fn state_style(theme: &Theme, tone: Tone) -> Style {
    match tone {
        // Busy is default text elsewhere (a spinner shows motion); a name has
        // no spinner, so it takes the attention color.
        Tone::Busy => theme.tone(Tone::Attention),
        tone => theme.tone(tone),
    }
}

/// Without colors the state needs its glyph, since the name cannot carry it.
fn needs_glyph(theme: &Theme) -> bool {
    theme.tier == Tier::Plain
}

/// The cells a box's name gets in a chip: a tenth of the screen, between 6
/// and 16.
pub(crate) fn box_room(screen: u16) -> usize {
    (usize::from(screen) / 10).clamp(6, 16)
}

/// Box `name` in at most `room` cells: whole when it fits. Otherwise the
/// start that every name in `names` shares gives way, `…fork-a`, keeping as
/// many of its words as fit. The end is cut only when the part that tells
/// the boxes apart does not fit either.
pub(crate) fn short_box(name: &str, names: &[&str], room: usize) -> String {
    let name = clean(name);
    if text::width(&name) <= room {
        return name;
    }
    let chars: Vec<char> = name.chars().collect();
    let shared = names
        .iter()
        .filter(|other| clean(other) != name)
        .map(|other| {
            chars
                .iter()
                .zip(clean(other).chars())
                .take_while(|(a, b)| **a == *b)
                .count()
        })
        .min()
        .unwrap_or(0);
    if shared == 0 {
        return clip(&name, room);
    }
    let tail = |from: usize| chars[from..].iter().collect::<String>();
    // The longest tail from a word start inside the shared part that fits.
    let start = (0..=shared)
        .filter(|at| *at == shared || (*at > 0 && matches!(chars[at - 1], '-' | '_' | '.')))
        // The `…` before the tail takes one cell.
        .find(|at| text::width(&tail(*at)) < room);
    match start {
        Some(at) => format!("…{}", tail(at)),
        None => format!("…{}", clip(&tail(shared), room.saturating_sub(1))),
    }
}

/// What a lane is called in the chips of a tab that has box lanes: its number,
/// which this machine's lane and the boxes' share, then the box or, with
/// `named`, this machine's branch. `@0 main` and `@0 fork-a`. `host` is the
/// box's name as [`short_box`] cut it.
pub(crate) fn numbered_label(lane: &LaneSummary, host: Option<&str>, named: bool) -> String {
    let number = lane
        .index
        .map(|index| format!("@{index}"))
        .unwrap_or_default();
    match host {
        Some(host) => format!("{number} {host}"),
        None if named => {
            let branch = lane
                .name
                .as_deref()
                .or(lane.branch.as_deref())
                .unwrap_or("detached");
            format!("{number} {}", clip(&clean(branch), 24))
        }
        None => number,
    }
}

/// One chip: its key (1-9, then 0), the name colored by state, `@N` when the
/// name is shared, the ready count while starting. A lane this machine does
/// not mirror is dim, with a hollow dot, or in parentheses with ascii glyphs.
/// `named` keeps this machine's branch beside box lanes.
fn lane_chip(
    app: &App,
    lane: &LaneSummary,
    lanes: &[&LaneSummary],
    index: usize,
    named: bool,
) -> Vec<Span<'static>> {
    let theme = &app.theme;
    let hosts: Vec<&str> = lanes
        .iter()
        .filter_map(|l| app.lane_box(&l.reference).map(|b| b.host.as_str()))
        .collect();
    // Box lanes have no branch here and share numbers with this machine's
    // lanes, so a tab with any of them leads every chip with the number.
    let (label, number) = if hosts.is_empty() {
        lane_label(lane, lanes)
    } else {
        let host = app
            .lane_box(&lane.reference)
            .map(|l| short_box(&l.host, &hosts, box_room(app.size.0)));
        (numbered_label(lane, host.as_deref(), named), None)
    };
    let (glyph, tone) = lane_glyph(theme, lane, app.spin);
    let mut spans = Vec::new();
    if index < 10 {
        spans.push(Span::styled(
            format!("{} ", (index + 1) % 10),
            theme.quiet(),
        ));
    }
    if app.grey(&lane.reference).is_some() {
        // The marker says it without color; the selection's reverse video
        // takes the dimming away.
        let (open, close) = theme.unmirrored();
        spans.push(Span::styled(format!("{open}{label}{close}"), theme.quiet()));
    } else {
        spans.push(Span::styled(label, state_style(theme, tone)));
        if let Some(number) = number {
            spans.push(Span::styled(number, theme.quiet()));
        }
        if needs_glyph(theme) {
            spans.push(Span::raw(format!(" {glyph}")));
        }
    }
    if lane.state == "starting" {
        let counts = tree::counts(lane);
        spans.push(Span::styled(
            format!(" {}/{}", counts.up, counts.total),
            state_style(theme, Tone::Busy),
        ));
    }
    spans
}

/// No projects: what devlanes is and how to start.
fn render_welcome(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let width = usize::from(area.width.saturating_sub(2)).max(1);
    let mut lines = vec![Line::from("")];
    if !app.loaded {
        lines.push(Line::styled(" loading…", theme.quiet()));
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }
    let mut add = |text: &str, style: Style| {
        for line in text::wrap(text, width) {
            lines.push(Line::styled(format!(" {line}"), style));
        }
    };
    add(
        "No projects yet.",
        Style::new().add_modifier(Modifier::BOLD),
    );
    add("", Style::new());
    add(
        "devlanes runs the services, tasks and containers a project lists in devlanes.yaml, one lane per git worktree.",
        Style::new(),
    );
    add("", Style::new());
    add(
        "  1  open a terminal in a repository that has a devlanes.yaml",
        Style::new(),
    );
    add("  2  run devlanes start", Style::new());
    add("", Style::new());
    add(
        "The project appears here as a tab while it starts. This window can stay open.",
        theme.quiet(),
    );
    add(
        "Writing a new devlanes.yaml? devlanes config check validates it before the first start.",
        theme.quiet(),
    );
    frame.render_widget(Paragraph::new(lines), area);
}

/// A footer hint: the key as shown, its label, and the key a click presses.
type Hint = (&'static str, &'static str, KeyCode);

/// The daemon log's copy mode or text filter keys, wherever the log shows.
fn log_mode_hints(app: &App) -> Vec<Hint> {
    let ch = KeyCode::Char;
    if app.daemon.filter_editing && app.daemon.copy.is_none() {
        return vec![
            ("type", "to filter", KeyCode::Null),
            ("enter", "keep", KeyCode::Enter),
            ("esc", "clear", KeyCode::Esc),
        ];
    }
    vec![
        ("h j k l", "move", KeyCode::Null),
        ("v", "mark", ch('v')),
        ("y", "copy", ch('y')),
        ("Y", "copy whole entries", ch('Y')),
        ("esc", "cancel", KeyCode::Esc),
    ]
}

/// `G follow` while the log is paused, else `F pause`.
fn follow_hint(app: &App) -> Hint {
    if app.daemon.paused {
        ("G", "follow", KeyCode::Char('G'))
    } else {
        ("F", "pause", KeyCode::Char('F'))
    }
}

/// The keys the unavailable screen takes: starting the daemon where that
/// helps, then the log's.
fn unavailable_hints(app: &App) -> Vec<Hint> {
    let ch = KeyCode::Char;
    if app.daemon.copy.is_some() || app.daemon.filter_editing {
        return log_mode_hints(app);
    }
    let mut hints = Vec::new();
    if matches!(
        app.daemon_state,
        DaemonState::Stopped { .. } | DaemonState::Unreachable
    ) {
        hints.push(("s", "start daemon", ch('s')));
    }
    hints.extend([
        ("j k", "scroll", ch('k')),
        follow_hint(app),
        ("w", "level", ch('w')),
        ("/", "filter", ch('/')),
        ("c", "copy", ch('c')),
    ]);
    hints
}

/// The hints that apply to the selection and mode, most important first.
fn hints(app: &App) -> Vec<Hint> {
    let ch = KeyCode::Char;
    if app.unavailable() {
        return unavailable_hints(app);
    }
    match app.view {
        View::Monitor => {
            let swap = if app.monitor.memory_first {
                ("c", "cpu graph", ch('c'))
            } else {
                ("m", "memory graph", ch('m'))
            };
            return vec![
                ("j k", "move", ch('j')),
                ("enter", "open unit", KeyCode::Enter),
                ("←→", "sort", KeyCode::Right),
                ("i", "reverse", ch('i')),
                ("/", "filter", ch('/')),
                ("e", "flat", ch('e')),
                swap,
                ("s x r", "start stop restart", ch('r')),
                ("esc", "back", KeyCode::Esc),
            ];
        }
        View::Daemon if app.daemon.copy.is_some() || app.daemon.filter_editing => {
            return log_mode_hints(app);
        }
        View::Daemon if app.daemon.checks_copy.is_some() => {
            return vec![
                ("h j k l", "move", KeyCode::Null),
                ("v", "mark", ch('v')),
                ("y", "copy", ch('y')),
                ("Y", "copy whole checks", ch('Y')),
                ("esc", "cancel", KeyCode::Esc),
            ];
        }
        View::Daemon => {
            let follow = follow_hint(app);
            let fold = if app.daemon.filters.unfolded {
                ("d", "fold", ch('d'))
            } else {
                ("d", "unfold", ch('d'))
            };
            let tab = match app.daemon.focus {
                crate::app::DaemonBox::Log => ("tab", "to checks", KeyCode::Tab),
                crate::app::DaemonBox::Checks => ("tab", "to log", KeyCode::Tab),
            };
            return vec![
                ("j k", "scroll", ch('k')),
                follow,
                ("w", "level", ch('w')),
                ("/", "filter", ch('/')),
                ("T", "targets", ch('T')),
                ("c", "copy", ch('c')),
                ("i", "checks", ch('i')),
                fold,
                tab,
                ("R", "restart", ch('R')),
                ("S", "stop", ch('S')),
            ];
        }
        View::Boxes => {
            let mut hints = Vec::new();
            if let Some((host, skipped)) = app.boxes.selected_skipped() {
                let reference = crate::boxes::lane_reference(&skipped.lane, &host.host);
                if app.grey(&reference).is_some_and(|grey| grey.switchable()) {
                    hints.push(("m", "use this one", ch('m')));
                }
            } else if let Some(host) = app.boxes.selected_host() {
                // A reconnecting box waits for its next try, so Enter only
                // shows how it goes.
                let enter = match host.state.as_str() {
                    "connected" => "disconnect",
                    "needs_auth" => "sign in",
                    "needs_pairing" => "type code",
                    "connecting" | "reconnecting" => "show",
                    _ => "connect",
                };
                hints.push(("enter", enter, KeyCode::Enter));
                if !matches!(
                    host.state.as_str(),
                    "connected" | "disconnected" | "stopped"
                ) {
                    hints.push(("d", "disconnect", ch('d')));
                }
            }
            if app.boxes.selected_client().is_some() {
                hints.push(("x", "revoke", ch('x')));
            }
            hints.extend([("n", "new box", ch('n')), ("p", "pairing code", ch('p'))]);
            let clients = app.boxes.clients.list.as_ref();
            if clients.and_then(|list| list.allow_connections) == Some(false) {
                hints.push(("o", "options", ch('o')));
            }
            let skipped: usize = app.boxes.hosts.iter().map(|h| h.skipped_lanes.len()).sum();
            if app.boxes.hosts.len() + skipped + app.boxes.clients.count() > 1 {
                hints.push(("j k", "select", ch('j')));
            }
            hints.push(("esc", "back", KeyCode::Esc));
            return hints;
        }
        View::Project => {}
    }
    if app.filter.as_ref().is_some_and(|f| f.editing) {
        return vec![
            ("type", "to filter", KeyCode::Null),
            ("enter", "keep", KeyCode::Enter),
            ("esc", "clear", KeyCode::Esc),
        ];
    }
    if let Some(PaneMode::Copy(_)) = app.attached.as_ref().map(|a| &a.mode) {
        return vec![
            ("h j k l", "move", KeyCode::Null),
            ("v", "mark", ch('v')),
            ("y", "copy", ch('y')),
            ("esc", "cancel", KeyCode::Esc),
        ];
    }
    if app
        .attached
        .as_ref()
        .is_some_and(|a| a.screen.scroll_offset() > 0)
    {
        return vec![
            ("pgup pgdn", "scroll", KeyCode::PageUp),
            ("esc", "back to live", KeyCode::Esc),
            ("c", "select text", ch('c')),
        ];
    }
    let Some(lane) = app.selected_lane() else {
        return Vec::new();
    };
    let mut hints = lane_hints(app, lane);
    if let Some(grey) = app.grey(&lane.reference) {
        // Its hostnames lead to the lane in use, so there is nothing to open.
        hints.retain(|(key, ..)| *key != "o");
        // Only where `m` can switch, if need be after stopping this machine's lane.
        if grey.switchable() {
            hints.insert(0, ("m", "use this one", ch('m')));
        }
    }
    hints
}

/// The hints for the selected row of `lane` in the list.
fn lane_hints(app: &App, lane: &LaneSummary) -> Vec<Hint> {
    let ch = KeyCode::Char;
    let filter = ("/", "filter", ch('/'));
    let switch = ("[ ]", "switch lane", ch(']'));
    // A box lane is read-only: what helps here, and the footer's note says
    // where the rest happens.
    if app.box_lane(&lane.reference).is_some() {
        return match app.selected_id() {
            Some(RowId::Section(_) | RowId::Group(..)) => {
                vec![
                    ("enter", "fold", KeyCode::Enter),
                    filter,
                    ("B", "boxes", ch('B')),
                ]
            }
            Some(RowId::WholeLane) | None => vec![
                ("o", "open in browser", ch('o')),
                switch,
                filter,
                ("B", "boxes", ch('B')),
            ],
            _ => vec![
                ("o", "open in browser", ch('o')),
                ("z", "zoom", ch('z')),
                filter,
                ("B", "boxes", ch('B')),
            ],
        };
    }
    match app.selected_id() {
        Some(RowId::WholeLane) => {
            let missing = lane
                .reason
                .as_ref()
                .is_some_and(|r| r.code == "worktree_missing");
            if missing {
                return vec![("C", "clean lane", ch('C')), switch];
            }
            let live = tree::lane_is_live(lane);
            let mut hints = if live {
                let r = if tree::reloads_on_r(lane) {
                    "reload changes"
                } else {
                    "restart lane"
                };
                vec![("x", "stop lane", ch('x')), ("r", r, ch('r'))]
            } else {
                vec![("s", "start lane", ch('s')), ("S", "start all", ch('S'))]
            };
            hints.extend([
                ("p", "start with profiles", ch('p')),
                ("C", "clean lane", ch('C')),
                switch,
                filter,
            ]);
            hints
        }
        Some(RowId::Section(_) | RowId::Group(..)) => vec![
            ("enter", "fold", KeyCode::Enter),
            ("s", "start all", ch('s')),
            ("x", "stop all", ch('x')),
            ("r", "restart all", ch('r')),
            ("p", "profiles", ch('p')),
            filter,
        ],
        Some(RowId::Container(_))
            if app
                .attached
                .as_ref()
                .is_some_and(|a| a.container && !matches!(a.state, AttachState::Failed(_))) =>
        {
            vec![
                ("pgup", "scroll back", KeyCode::PageUp),
                ("c", "select text", ch('c')),
                ("z", "zoom", ch('z')),
                filter,
                ("n", "next problem", ch('n')),
            ]
        }
        Some(RowId::Container(_)) => vec![filter, ("n", "next problem", ch('n'))],
        Some(RowId::Gap(_)) => vec![filter, ("n", "next problem", ch('n'))],
        Some(RowId::Task(_)) => vec![("s", "run", ch('s')), filter],
        Some(RowId::Unit(_)) => {
            let Some(unit) = app.selected_unit() else {
                return vec![filter];
            };
            if tree::role(unit) == Role::External {
                return vec![("o", "open in browser", ch('o')), filter];
            }
            let task = tree::role(unit) != Role::Service && tree::role(unit) != Role::Exec;
            match unit.state.as_str() {
                ORPHANED => vec![("x", "end it", ch('x')), ("n", "next problem", ch('n'))],
                "failed" | "exited" if !task || unit.exit_code != Some(0) => vec![
                    ("r", if task { "run again" } else { "restart" }, ch('r')),
                    ("n", "next problem", ch('n')),
                    ("pgup", "scroll back", KeyCode::PageUp),
                    ("c", "select text", ch('c')),
                    filter,
                ],
                state if crate::app::is_active(state) && task => vec![
                    ("enter", "type into it", KeyCode::Enter),
                    ("x", "stop", ch('x')),
                    ("c", "select text", ch('c')),
                    ("z", "zoom", ch('z')),
                ],
                state if crate::app::is_active(state) => vec![
                    ("enter", "type into it", KeyCode::Enter),
                    ("x", "stop", ch('x')),
                    ("r", "restart", ch('r')),
                    ("o", "open in browser", ch('o')),
                    ("c", "select text", ch('c')),
                    ("z", "zoom", ch('z')),
                    filter,
                ],
                _ if task => vec![("s", "run", ch('s')), filter],
                _ => vec![("s", "start", ch('s')), filter],
            }
        }
        None => Vec::new(),
    }
}

/// The footer: hints for the selection, `? all keys  q quit` on the right.
/// While typing, one banner says how to get back.
fn render_footer(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let width = usize::from(area.width);
    if app.focus == Focus::Terminal && app.view == View::Project {
        let label = app
            .attached_unit()
            .map(|u| clean(u.label()))
            .or_else(|| app.attached.as_ref().map(|a| clean(&a.unit)))
            .unwrap_or_default();
        let back = if app.full_screen() {
            "ctrl+] returns to the list"
        } else {
            "esc or ctrl+] returns to the list"
        };
        let live = app
            .attached
            .as_ref()
            .is_some_and(|a| a.state == AttachState::Live);
        // ` TYPING into Gateway API · keys go to the program · esc or ctrl+] returns to the list`
        let spans = vec![
            Span::styled(" TYPING", theme.focus().add_modifier(Modifier::BOLD)),
            Span::raw(format!(" into {label}")),
            Span::styled(
                if live {
                    " · keys go to the program · "
                } else {
                    " · the program is not live · "
                },
                theme.quiet(),
            ),
            Span::styled(back, theme.key()),
        ];
        frame.render_widget(Paragraph::new(fit(spans, width)), area);
        return;
    }
    let unavailable = app.unavailable();
    // `?` waits for the daemon like every key the unavailable screen does not take.
    let fixed: &[Hint] = if unavailable {
        &[("q", "quit", KeyCode::Char('q'))]
    } else {
        &[
            ("?", "all keys", KeyCode::Char('?')),
            ("q", "quit", KeyCode::Char('q')),
        ]
    };
    // `key label` and three spaces, as the mockups' footer.
    let size = |(key, label, _): &Hint| text::width(key) + text::width(label) + 4;
    let fixed_width: usize = fixed.iter().map(size).sum();
    let budget = width.saturating_sub(1 + fixed_width);
    let mut hints = hints(app);
    // While the header shows a cut error, its full text is one key away.
    let copying = app
        .attached
        .as_ref()
        .is_some_and(|a| matches!(a.mode, PaneMode::Copy(_)));
    let typing = app.filter.as_ref().is_some_and(|f| f.editing);
    if app.view == View::Project
        && !unavailable
        && app.notice.as_ref().is_some_and(|n| n.error)
        && !copying
        && !typing
    {
        // After the row's main key, which stays the first thing to read.
        let at = hints.len().min(1);
        hints.insert(at, ("e", "error details", KeyCode::Char('e')));
    }
    let mut used = 0;
    let mut shown = Vec::new();
    for hint in hints {
        if used + size(&hint) > budget {
            break;
        }
        used += size(&hint);
        shown.push(hint);
    }
    let mut left = vec![Span::raw(" ")];
    let mut x = area.x + 1;
    let record = |spans: &mut Vec<Span<'static>>, x: &mut u16, (key, label, code): Hint| {
        let w = (text::width(key) + 1 + text::width(label)) as u16;
        if code != KeyCode::Null {
            let modifiers = if matches!(code, KeyCode::Char(c) if c.is_ascii_uppercase()) {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            hit(
                app,
                Rect::new(*x, area.y, w, 1),
                Click::Key(code, modifiers),
            );
        }
        spans.push(Span::styled(key, theme.key()));
        spans.push(Span::styled(format!(" {label}   "), theme.quiet()));
        *x += w + 3;
    };
    for hint in shown {
        record(&mut left, &mut x, hint);
    }
    // On a box lane, where its start, stop and logs are, in what is left.
    if !typing && !copying && !unavailable {
        let room = budget.saturating_sub(used);
        // The longest form that fits with the three spaces hints keep apart.
        if let Some(note) = box_note(app)
            .into_iter()
            .find(|note| text::width(note) + 3 <= room)
        {
            left.push(Span::styled(note, theme.quiet()));
        }
    }
    let mut right = Vec::new();
    let mut rx = area.right().saturating_sub(fixed_width as u16);
    for hint in fixed.iter().copied() {
        record(&mut right, &mut rx, hint);
    }
    // Hints that fill the budget exactly leave no room for the gap `spread`
    // keeps; the last one's padding goes instead of being cut into `…`.
    if spans_width(&left) + spans_width(&right) >= width
        && let Some(last) = left.last_mut()
    {
        last.content = last.content.trim_end().to_owned().into();
    }
    frame.render_widget(Paragraph::new(spread(left, right, width)), area);
}

/// The footer's note on a box lane, longest first for the room there is:
/// `read-only: manage it on hetzner (ssh hetzner)` (the ssh part only over
/// ssh), `read-only: manage it on hetzner`, `read-only`. Empty elsewhere.
fn box_note(app: &App) -> Vec<String> {
    let Some(lane) = app.selected_lane().filter(|_| app.view == View::Project) else {
        return Vec::new();
    };
    let Some((host, _)) = app.box_lane(&lane.reference) else {
        return Vec::new();
    };
    let name = clean(&host.host);
    let short = format!("read-only: manage it on {name}");
    let mut notes = Vec::new();
    if host.over_ssh() {
        notes.push(format!("{short} (ssh {name})"));
    }
    notes.extend([short, "read-only".to_owned()]);
    notes
}

/// The failed action on `target`, if any.
pub(crate) fn target_error<'a>(
    app: &'a App,
    lane: &str,
    unit: Option<&str>,
) -> Option<&'a crate::app::ErrorEntry> {
    let target = match unit {
        Some(unit) => Target::Unit {
            lane: lane.into(),
            unit: unit.into(),
        },
        None => Target::Lane(lane.into()),
    };
    app.target_error(&target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(index: u32, branch: &str, name: Option<&str>) -> LaneSummary {
        LaneSummary {
            reference: format!("acme@{index}"),
            project: "acme".into(),
            index: Some(index),
            branch: Some(branch.into()),
            name: name.map(str::to_owned),
            ..LaneSummary::default()
        }
    }

    #[test]
    fn lanes_go_by_branch_and_show_their_number_only_when_it_is_shared() {
        let main = lane(0, "main", None);
        let feat = lane(1, "feat/x", None);
        let again = lane(2, "main", None);
        // An extra lane of main's folder: same branch, its own name.
        let review = lane(3, "main", Some("review"));
        let all = [&main, &feat, &again, &review];
        assert_eq!(lane_label(&feat, &all), ("feat/x".into(), None));
        assert_eq!(lane_label(&main, &all), ("main".into(), Some("@0".into())));
        assert_eq!(lane_label(&again, &all), ("main".into(), Some("@2".into())));
        assert_eq!(lane_label(&review, &all), ("review".into(), None));
        // Alone, main needs no number.
        assert_eq!(
            lane_label(&main, &[&main, &feat, &review]),
            ("main".into(), None)
        );
    }

    /// Beside box lanes every chip leads with its number: this machine's
    /// branch when there is room, the box's name otherwise.
    #[test]
    fn box_lanes_go_by_number_and_box() {
        let zero = lane(0, "main", None);
        assert_eq!(numbered_label(&zero, None, false), "@0");
        assert_eq!(numbered_label(&zero, None, true), "@0 main");
        let review = lane(1, "main", Some("review"));
        assert_eq!(numbered_label(&review, None, true), "@1 review");
        assert_eq!(numbered_label(&zero, Some("fork-a"), true), "@0 fork-a");
        assert_eq!((box_room(80), box_room(120), box_room(200)), (8, 12, 16));
    }

    /// Forks that share a long start keep what tells them apart.
    #[test]
    fn box_names_drop_the_start_they_share() {
        let forks = ["clickmax-fork-a", "clickmax-fork-b", "clickmax-fork-c"];
        assert_eq!(short_box("clickmax-fork-a", &forks, 8), "…fork-a");
        assert_eq!(short_box("clickmax-fork-b", &forks, 16), "clickmax-fork-b");
        assert_eq!(short_box("clickmax-fork-c", &forks, 12), "…fork-c");
        let vms = ["fan-fix-login-1a2b", "fan-fix-login-3c4d"];
        assert_eq!(short_box("fan-fix-login-1a2b", &vms, 8), "…1a2b");
        assert_eq!(short_box("fan-fix-login-3c4d", &vms, 12), "…login-3c4d");
        // Nothing shared: the end is cut, as for one box alone.
        assert_eq!(
            short_box("clickmax-fork-3f2a", &["clickmax-fork-3f2a"], 8),
            "clickma…"
        );
        assert_eq!(
            short_box("clickmax-fork-a", &["clickmax-fork-a", "gpu"], 8),
            "clickma…"
        );
        // A distinct part longer than the room is cut at its end.
        let long = ["box-aaaaaaaaaaaa", "box-bbbbbbbbbbbb"];
        assert_eq!(short_box("box-aaaaaaaaaaaa", &long, 8), "…aaaaaa…");
    }

    /// An idle unit keeps `state: ready`; its glyph comes from `power`, its
    /// word stays dim, and a started-only unit reads the same once frozen.
    #[test]
    fn idle_units_take_the_power_glyph() {
        let theme = Theme::default();
        let unit = |power: Option<&str>, check: Option<&str>| UnitSummary {
            name: "api".into(),
            state: "ready".into(),
            kind: "service".into(),
            power: power.map(str::to_owned),
            ready_check: check.map(str::to_owned),
            ..UnitSummary::default()
        };
        let frozen = unit_look(&theme, &unit(Some("frozen"), Some("started")), 0);
        assert_eq!(
            (
                frozen.glyph,
                frozen.tone,
                frozen.word.as_str(),
                frozen.word_tone
            ),
            ("❄", Tone::Frozen, "frozen", Tone::Quiet)
        );
        let throttled = unit_look(&theme, &unit(Some("throttled"), None), 0);
        assert_eq!(
            (throttled.glyph, throttled.tone, throttled.word.as_str()),
            ("z", Tone::Quiet, "throttled")
        );
        assert_eq!(unit_look(&theme, &unit(None, None), 0).glyph, "●");
    }
}
