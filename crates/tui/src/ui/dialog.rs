//! Dialogs: confirmations, the profile picker and the key list. They keep a
//! box because they float over content.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use super::fit;
use crate::app::{App, Dialog, ProfileChoice, ProfilesState, View};
use crate::text::{self, clean, clip};
use crate::theme::{Glyph, Theme, Tone};

/// Every binding, grouped by where it works. `None` keys are section names.
const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "list",
        &[
            ("j k  up down", "move; k from the top selects the lane"),
            // One line for both, so the not mirrored glyph fits 60 rows.
            ("g G  t  i", "the lane, the last row, the tasks, the infra"),
            ("h l", "fold, unfold; h moves to the section"),
            (
                "enter  space",
                "fold a section, or type into a running unit",
            ),
            (
                "[ ]  1-9",
                "previous, next lane; lane by its place in the bar",
            ),
            (
                "n N  e",
                "next, previous problem; every error in full (or click it)",
            ),
            (
                "s  x  r",
                "start, stop, restart the selection; s runs a task",
            ),
            ("s  S", "on the lane: its default services; every service"),
            ("r", "on the lane: reload changes, else restart it"),
            // One line for both, so the whole list still fits 60 rows.
            (
                "p  C",
                "start the lane with profiles, clean it (asks first)",
            ),
            ("o", "open the unit's address in a browser"),
            ("m", "use a grey lane instead of the one in use"),
            ("/", "filter by name or +tag; enter keeps, esc clears"),
        ],
    ),
    (
        "pane",
        &[
            ("enter, click", "type into the unit"),
            (
                "esc  ctrl+]",
                "stop typing (ctrl+] in full-screen programs)",
            ),
            ("z", "zoom the pane"),
            (
                "pgup pgdn  wheel",
                "scroll; older lines come from stored logs",
            ),
            (
                "c  v  y  drag",
                "select text, mark, copy (OSC 52), or drag to copy",
            ),
        ],
    ),
    (
        "monitor (M)",
        &[
            ("←→  < >", "sort by the previous, next column"),
            ("i", "reverse the sort"),
            ("/  e", "filter, grouped or flat"),
            ("h l  z", "fold, unfold a lane; detail graphs fill the page"),
            (
                "c  m",
                "cpu or memory in the big box (memory: a square per core)",
            ),
            ("s  x  r", "start, stop, restart the unit or lane"),
            ("enter", "open the unit in its project"),
        ],
    ),
    (
        "daemon (D)",
        &[
            ("R  S", "restart or stop the daemon; s starts it again"),
            ("X", "clean the lanes whose worktree is gone"),
            ("j k  g G", "scroll the log (pauses), oldest, newest"),
            ("F  d", "pause or follow the log, fold or unfold repeats"),
            ("w  /  T", "level floor, text filter, targets"),
            (
                "c",
                "copy mode: v select, y copy, Y whole entries as in the file",
            ),
            ("i", "every check, or problems only"),
        ],
    ),
    (
        "boxes (B)",
        &[
            ("enter", "connect, sign in, type a code or disconnect"),
            ("d", "disconnect the selected box"),
            ("n  p", "connect a new box, show a pairing code"),
            ("x", "revoke the selected client"),
        ],
    ),
    (
        "everywhere",
        &[
            (
                "tab  shift+tab",
                "next, previous project; shift+1-9 picks one by number",
            ),
            ("M  D  B", "monitor, daemon, boxes; again or esc returns"),
            ("esc", "one step back, then the menu: options, help, quit"),
            (
                "u  q",
                "hide the version notice; quit, asking first while lanes run",
            ),
        ],
    ),
];

/// The section of the screen in view, listed first in the help.
fn first_section(app: &App) -> &'static str {
    match app.view {
        View::Monitor => "monitor (M)",
        View::Daemon => "daemon (D)",
        View::Boxes => "boxes (B)",
        View::Project => "list",
    }
}

/// Fits a dialog into `rows`: the tail always shows; the body loses its last
/// lines, the final kept one replaced by `…`.
fn compact(
    mut body: Vec<Line<'static>>,
    tail: Vec<Line<'static>>,
    rows: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    // A blank line separates body and tail when there is room for it.
    if body.len() + tail.len() < rows && !body.is_empty() {
        body.push(Line::from(""));
    }
    if body.len() + tail.len() <= rows {
        body.extend(tail);
        return body;
    }
    if tail.len() >= rows {
        let skip = tail.len() - rows;
        return tail.into_iter().skip(skip).collect();
    }
    let room = rows - tail.len();
    body.truncate(room.saturating_sub(1));
    body.push(Line::styled("…", theme.quiet()));
    body.extend(tail);
    body
}

/// First index of a `room`-row window over `len` items that shows `selected`.
fn window(len: usize, selected: usize, room: usize) -> usize {
    if len <= room {
        return 0;
    }
    selected.saturating_sub(room / 2).min(len - room)
}

/// The marker of a lane chip this machine does not mirror, as the legend
/// shows it: the hollow dot, or the parentheses ascii glyphs put around the
/// lane's name.
fn unmirrored(theme: &Theme) -> &'static str {
    match theme.unmirrored() {
        ("", close) => close.trim_start(),
        _ => "()",
    }
}

/// The glyph legend as a grid in as many columns as fit `max` cells, four at
/// most: three lines on a wide screen, so the keys below still fit 60 rows,
/// and one more line for each column a narrow screen drops.
fn legend_lines(legend: &[(&'static str, Style, &'static str)], max: usize) -> Vec<Line<'static>> {
    const GAP: usize = 2;
    let widths = |columns: usize| -> Vec<usize> {
        (0..columns)
            .map(|column| {
                legend
                    .iter()
                    .skip(column)
                    .step_by(columns)
                    .map(|(glyph, _, word)| text::width(glyph) + 1 + text::width(word))
                    .max()
                    .unwrap_or(0)
            })
            .collect()
    };
    // Two cells of indent, then `glyph word` per entry; the ascii marker
    // `()` takes two cells.
    let total =
        |widths: &[usize]| 2 + widths.iter().sum::<usize>() + GAP * widths.len().saturating_sub(1);
    let columns = (1..=4)
        .rev()
        .find(|columns| total(&widths(*columns)) <= max)
        .unwrap_or(1);
    let widths = widths(columns);
    legend
        .chunks(columns)
        .map(|row| {
            let mut line = vec![Span::raw("  ")];
            for (index, (glyph, style, word)) in row.iter().enumerate() {
                line.push(Span::styled(format!("{glyph} "), *style));
                line.push(Span::raw(if index + 1 < row.len() {
                    let pad = widths[index] + GAP - text::width(glyph) - 1;
                    format!("{word:<pad$}")
                } else {
                    (*word).to_owned()
                }));
            }
            fit(line, max)
        })
        .collect()
}

/// `key action   key action`: the keys bold, the labels dim.
pub(super) fn keys(theme: &Theme, pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, label)) in pairs.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled((*key).to_owned(), theme.key()));
        spans.push(Span::styled(format!(" {label}"), theme.quiet()));
    }
    Line::from(spans)
}

/// `▌[x] web      default    +web, +api`: the checkbox and name, then the
/// markers, then the include selectors dimmed.
fn profile_row(theme: &Theme, item: &ProfileChoice, selected: bool, width: usize) -> Line<'static> {
    let check = if item.checked { "[x]" } else { "[ ]" };
    let profile = &item.profile;
    let marker = if selected {
        theme.glyph(Glyph::Marker)
    } else {
        " "
    };
    let mut spans = vec![
        Span::styled(marker, theme.focus()),
        Span::raw(format!("{check} ")),
        Span::styled(
            format!("{:<9}", clean(&profile.name)),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<9}", if profile.is_default { "default" } else { "" }),
            theme.quiet(),
        ),
        Span::styled(
            format!("{:<9}", if profile.skips_infra { "no infra" } else { "" }),
            theme.quiet(),
        ),
    ];
    if !profile.include.is_empty() {
        spans.push(Span::styled(
            clean(&profile.include.join(", ")),
            theme.quiet(),
        ));
    }
    let line = fit(spans, width);
    if selected {
        line.patch_style(theme.selection())
    } else {
        line
    }
}

/// `rect` moved up to two rows within `area` so that neither its top nor
/// its bottom row lies on a border of the screen under it. A border there
/// runs into the dialog's own, and the two read as one box.
fn off_borders(frame: &mut Frame, area: Rect, rect: Rect) -> Rect {
    // The column beside the dialog shows what runs under its rows.
    let beside = if rect.x > area.x {
        rect.x - 1
    } else if rect.right() < area.right() {
        rect.right()
    } else {
        return rect;
    };
    let buffer = frame.buffer_mut();
    let border = |y: u16| {
        buffer
            .cell((beside, y))
            .is_some_and(|cell| matches!(cell.symbol(), "─" | "┌" | "┐" | "└" | "┘" | "├" | "┤"))
    };
    let clear = |rect: Rect| !border(rect.y) && !border(rect.bottom() - 1);
    if rect.height == 0 || clear(rect) {
        return rect;
    }
    for shift in [-1i32, 1, -2, 2] {
        let Some(y) = u16::try_from(i32::from(rect.y) + shift).ok() else {
            continue;
        };
        let moved = Rect { y, ..rect };
        if moved.y >= area.y && moved.bottom() <= area.bottom() && clear(moved) {
            return moved;
        }
    }
    rect
}

/// Modal dialogs over a cleared box.
pub fn render(app: &App, dialog: &Dialog, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    // Text wraps to the widest box that fits, leaving a column of margin each side.
    let max = usize::from(area.width.saturating_sub(6)).max(1);
    let narrow = max.min(64);
    let wrapped = |text: &str, width: usize, style: Style| -> Vec<Line<'static>> {
        text::wrap(&clean(text), width)
            .into_iter()
            .map(|line| Line::styled(line, style))
            .collect()
    };
    let rows = usize::from(area.height.saturating_sub(2)).max(1);
    // `body` may be cut to fit; `tail` (the keys) always stays.
    let (title, body, tail): (String, Vec<Line>, Vec<Line>) = match dialog {
        // The Esc menu and its options page draw themselves.
        Dialog::Menu(menu) => return crate::menu::render(theme, menu, app.version, frame, area),
        Dialog::Settings(settings) => {
            return crate::settings::render(theme, settings, frame, area);
        }
        Dialog::Welcome(welcome) => return crate::welcome::render(theme, welcome, frame, area),
        Dialog::Connect(dialog) => super::connect::connect(app, dialog, narrow),
        Dialog::Pair(dialog) => super::connect::pair(app, dialog, narrow),
        // No border title: the bold prompt already says what it is about.
        Dialog::Confirm {
            prompt,
            detail,
            action,
            strict,
            ..
        } => {
            let mut body = wrapped(prompt, narrow, Style::new().add_modifier(Modifier::BOLD));
            if !detail.is_empty() {
                body.push(Line::from(""));
            }
            for sentence in detail {
                body.extend(wrapped(sentence, narrow, Style::new()));
            }
            let tail = if *strict {
                keys(theme, &[("y", action), ("n", "cancel")])
            } else {
                keys(theme, &[("enter", action), ("esc", "cancel")])
            };
            (String::new(), body, vec![tail])
        }
        Dialog::Targets(picker) => {
            let width = max.min(56);
            let tail = vec![
                Line::styled("With none checked, every target shows.", theme.quiet()),
                Line::from(""),
                keys(
                    theme,
                    &[
                        ("space", "check"),
                        ("a", "none"),
                        ("enter", "apply"),
                        ("esc", "cancel"),
                    ],
                ),
            ];
            let room = rows.saturating_sub(tail.len() + 1).max(1);
            let start = window(picker.items.len(), picker.index, room);
            let body = picker
                .items
                .iter()
                .enumerate()
                .skip(start)
                .take(room)
                .map(|(index, (target, count, checked))| {
                    let selected = index == picker.index;
                    let marker = if selected {
                        theme.glyph(Glyph::Marker)
                    } else {
                        " "
                    };
                    let check = if *checked { "[x]" } else { "[ ]" };
                    let name_width = width.saturating_sub(12);
                    let line = fit(
                        vec![
                            Span::styled(marker, theme.focus()),
                            Span::raw(format!("{check} ")),
                            Span::raw(format!(
                                "{:<name_width$} ",
                                clip(&clean(target), name_width)
                            )),
                            Span::styled(format!("{count:>5}"), theme.quiet()),
                        ],
                        width,
                    );
                    if selected {
                        line.patch_style(theme.selection())
                    } else {
                        line
                    }
                })
                .collect();
            ("daemon.log targets".to_owned(), body, tail)
        }
        Dialog::Quit(quit) => {
            let lanes = quit.lanes.join(", ");
            let prompt = if quit.lanes.len() == 1 {
                format!("{lanes} is running.")
            } else {
                format!("{lanes} are running.")
            };
            let mut body = wrapped(&prompt, narrow, Style::new().add_modifier(Modifier::BOLD));
            body.push(Line::from(""));
            for (index, choice) in crate::app::QUIT_CHOICES.iter().enumerate() {
                let selected = index == quit.index;
                let marker = if selected {
                    theme.glyph(Glyph::Marker)
                } else {
                    " "
                };
                let line = fit(
                    vec![
                        Span::styled(marker, theme.focus()),
                        Span::raw(format!("{choice} ")),
                    ],
                    narrow,
                );
                body.push(if selected {
                    line.patch_style(theme.selection())
                } else {
                    line
                });
            }
            body.push(Line::from(""));
            let check = if quit.remember { "[x]" } else { "[ ]" };
            body.push(Line::from(vec![
                Span::raw(format!(" {check} remember this choice ")),
                Span::styled("(tui.on_quit)", theme.quiet()),
            ]));
            let tail = keys(
                theme,
                &[
                    ("enter", "choose"),
                    ("space", "remember"),
                    ("esc", "cancel"),
                ],
            );
            ("quit".to_owned(), body, vec![tail])
        }
        Dialog::Profiles { lane, state } => {
            let mut body = Vec::new();
            let tail = match state {
                ProfilesState::Loading => {
                    body.extend(wrapped("loading…", narrow, theme.quiet()));
                    vec![keys(theme, &[("esc", "cancel")])]
                }
                ProfilesState::Unsupported => {
                    body.extend(wrapped(
                        "This daemon is too old for profiles. Upgrade devlanes and restart the daemon.",
                        narrow,
                        Style::new(),
                    ));
                    vec![keys(theme, &[("esc", "close")])]
                }
                ProfilesState::Ready { items, .. } if items.is_empty() => {
                    body.extend(wrapped(
                        "devlanes.yaml defines no profiles.",
                        narrow,
                        Style::new(),
                    ));
                    vec![keys(theme, &[("esc", "close")])]
                }
                ProfilesState::Ready { items, index } => {
                    // Matches what Enter sends with nothing checked.
                    let empty = if items.iter().any(|item| item.profile.is_default) {
                        "With none checked, enter starts the default profile."
                    } else {
                        "With none checked, enter starts the lane without profiles."
                    };
                    let mut tail = wrapped(empty, narrow, theme.quiet());
                    tail.push(Line::from(""));
                    tail.push(keys(
                        theme,
                        &[("space", "check"), ("enter", "start"), ("esc", "cancel")],
                    ));
                    let width = max.min(72);
                    let room = rows.saturating_sub(tail.len() + 1).max(1);
                    let start = window(items.len(), *index, room);
                    body.extend(
                        items
                            .iter()
                            .enumerate()
                            .skip(start)
                            .take(room)
                            .map(|(i, item)| profile_row(theme, item, i == *index, width)),
                    );
                    tail
                }
            };
            (format!("start {} with profiles", clean(lane)), body, tail)
        }
        Dialog::Errors(state) => {
            let Some(entry) = app.errors.get(state.index) else {
                return;
            };
            let width = max.min(100);
            let mut text = Vec::new();
            for (index, line) in entry.lines().iter().enumerate() {
                let style = if index == 0 {
                    theme.tone(Tone::Failure).add_modifier(Modifier::BOLD)
                } else if line.starts_with("command: ") || line.starts_with("run: ") {
                    theme.quiet()
                } else {
                    Style::new()
                };
                text.extend(wrapped(line, width, style));
            }
            // The keys take one row and the blank line before them another.
            let room = rows.saturating_sub(2).max(1);
            let max_scroll = text.len().saturating_sub(room);
            state.max_scroll.set(max_scroll);
            let scroll = state.scroll.min(max_scroll);
            let mut pairs = Vec::new();
            if app.errors.len() > 1 {
                pairs.push(("h l", "older, newer"));
            }
            if max_scroll > 0 {
                pairs.push(("j k", "scroll"));
            }
            pairs.extend([("y", "copy"), ("esc", "close")]);
            let tail = vec![keys(theme, &pairs)];
            let mut title = if app.errors.len() > 1 {
                format!("error {} of {}", state.index + 1, app.errors.len())
            } else {
                "error".to_owned()
            };
            // Says there is more below, and how far down the view is.
            if max_scroll > 0 {
                let shown = (scroll + room).min(text.len());
                title.push_str(&format!(
                    " · lines {}-{shown} of {}",
                    scroll + 1,
                    text.len()
                ));
            }
            let body = text.into_iter().skip(scroll).take(room).collect();
            (title, body, tail)
        }
        Dialog::Help => {
            let first = first_section(app);
            let mut sections: Vec<&(&str, &[(&str, &str)])> = KEYS.iter().collect();
            sections.sort_by_key(|(name, _)| *name != first);
            // The glyphs first: normal states have no word in the list, so
            // this is where their meaning is written down.
            let legend = [
                (
                    theme.glyph(Glyph::Ready),
                    theme.tone(Tone::Ok),
                    "ready (check passed)",
                ),
                (
                    theme.glyph(Glyph::Running),
                    theme.tone(Tone::Ok),
                    "running (no ready check)",
                ),
                (theme.glyph(Glyph::Throttled), theme.quiet(), "throttled"),
                (
                    theme.glyph(Glyph::Frozen),
                    theme.tone(Tone::Frozen),
                    "frozen",
                ),
                (
                    theme.glyph(Glyph::Stopped),
                    theme.quiet(),
                    "stopped, not run",
                ),
                (theme.spinner(3), Style::new(), "starting, stopping"),
                (theme.glyph(Glyph::Queued), theme.quiet(), "queued"),
                (
                    theme.glyph(Glyph::Failed),
                    theme.tone(Tone::Failure),
                    "failed",
                ),
                (
                    theme.glyph(Glyph::Orphaned),
                    theme.tone(Tone::Attention),
                    "orphaned",
                ),
                (theme.glyph(Glyph::Done), theme.tone(Tone::Ok), "task done"),
                (theme.glyph(Glyph::Bar), Style::new(), "the unit's color"),
                (
                    unmirrored(theme),
                    theme.quiet(),
                    "not in use here, m uses it",
                ),
            ];
            let mut body = vec![Line::styled("glyphs", theme.quiet())];
            body.extend(legend_lines(&legend, max));
            for (name, pairs) in sections {
                if !body.is_empty() {
                    body.push(Line::from(""));
                }
                body.push(Line::styled((*name).to_owned(), theme.quiet()));
                for (key, what) in *pairs {
                    body.push(fit(
                        vec![
                            Span::styled(format!("  {key:<18}"), theme.key()),
                            Span::raw((*what).to_owned()),
                        ],
                        max,
                    ));
                }
            }
            (
                "keys".into(),
                body,
                vec![Line::styled("any key closes", theme.quiet())],
            )
        }
    };
    let lines = compact(body, tail, rows, theme);
    // Wide enough for the title too, which is cut only on a narrow screen.
    let content_width = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(text::width(&clean(&title)) + 2);
    let width = u16::try_from(content_width + 4)
        .unwrap_or(area.width)
        .clamp(20.min(area.width), area.width);
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(area.height)
        .min(area.height);
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let rect = off_borders(frame, area, rect);
    *app.dialog_area.borrow_mut() = Some(rect);
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme.focus());
    if !title.is_empty() {
        block = block.title(Span::raw(format!(
            " {} ",
            clip(&clean(&title), usize::from(width.saturating_sub(4)))
        )));
    }
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(lines).block(block.padding(ratatui::widgets::Padding::horizontal(1))),
        rect,
    );
}
