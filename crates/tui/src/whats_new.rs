//! "What's new": the release notes the binary hands over in
//! [`crate::Options`], shown once after the welcome when this devlanes is
//! newer than the one seen before, and from the Esc menu. Each release under
//! its version, minor entries first and marked `breaking`, inline code in
//! the focus color. j/k, PgUp/PgDn and the wheel scroll a long list.

use std::cell::{Cell, RefCell};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use devlanes_update::notes::{Level, WhatsNew};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::menu::{centered, floating_box};
use crate::text::{self, clean};
use crate::theme::{Glyph, Theme, Tone};

/// The marker before a minor entry, which may break what worked.
const BREAKING: &str = "breaking ";

/// Columns of text at most, so lines stay readable on a wide screen.
const TEXT_WIDTH: usize = 76;

/// The open dialog.
#[derive(Debug, Default)]
pub struct WhatsNewDialog {
    /// Lines scrolled past at the top.
    pub scroll: usize,
    /// The furthest `scroll` that still fills the box, as last drawn; only
    /// the renderer knows the wrapped length.
    max_scroll: Cell<usize>,
    /// The box of the last render, for the mouse.
    hits: RefCell<Option<Rect>>,
}

/// What a key or click did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Stay,
    Close,
    Quit,
}

impl WhatsNewDialog {
    /// j/k and the arrows scroll a line, PgUp/PgDn ten, g/G to either end;
    /// Enter or Esc closes; `q` and Ctrl+C quit as they do everywhere else.
    pub fn key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let max = self.max_scroll.get();
        // The box may have grown since the scroll was set.
        self.scroll = self.scroll.min(max);
        match key.code {
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Enter | KeyCode::Esc => return Action::Close,
            KeyCode::Down | KeyCode::Char('j') => self.scroll = (self.scroll + 1).min(max),
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll = (self.scroll + 10).min(max),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Home | KeyCode::Char('g') => self.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => self.scroll = max,
            _ => {}
        }
        Action::Stay
    }

    /// The wheel scrolls; a click outside the box closes it.
    pub fn mouse(&mut self, mouse: MouseEvent) -> Action {
        let max = self.max_scroll.get();
        self.scroll = self.scroll.min(max);
        match mouse.kind {
            MouseEventKind::ScrollDown => self.scroll = (self.scroll + 3).min(max),
            MouseEventKind::ScrollUp => self.scroll = self.scroll.saturating_sub(3),
            MouseEventKind::Down(MouseButton::Left) => {
                let inside = self
                    .hits
                    .borrow()
                    .is_some_and(|rect| rect.contains(Position::new(mouse.column, mouse.row)));
                if !inside {
                    return Action::Close;
                }
            }
            _ => {}
        }
        Action::Stay
    }
}

/// `text` as spans, the parts between backticks in `code` without the
/// backticks. `in_code` carries an open span over to the next wrapped line.
pub(crate) fn code_spans(text: &str, in_code: &mut bool, code: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, part) in text.split('`').enumerate() {
        if index > 0 {
            *in_code = !*in_code;
        }
        if !part.is_empty() {
            let style = if *in_code { code } else { Style::new() };
            spans.push(Span::styled(part.to_owned(), style));
        }
    }
    spans
}

/// Every line of `notes` wrapped to `width`: a heading per release, then its
/// entries as bullets with a hanging indent.
fn lines(theme: &Theme, notes: Option<&WhatsNew>, width: usize) -> Vec<Line<'static>> {
    let Some(notes) = notes.filter(|notes| !notes.is_empty()) else {
        return vec![Line::styled(
            "This build carries no release notes.",
            theme.quiet(),
        )];
    };
    let bullet = format!("{} ", theme.glyph(Glyph::Folded));
    let mut lines = Vec::new();
    for release in &notes.releases {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::styled(
            clean(&release.version),
            theme.focus().add_modifier(Modifier::BOLD),
        ));
        for entry in &release.entries {
            let marker = match entry.level {
                Level::Minor => BREAKING,
                Level::Patch => "",
            };
            let indent = text::width(&bullet) + marker.len();
            let mut in_code = false;
            for (index, row) in text::wrap(&clean(&entry.line), width.saturating_sub(indent))
                .into_iter()
                .enumerate()
            {
                let mut spans = if index == 0 {
                    vec![
                        Span::styled(bullet.clone(), theme.quiet()),
                        Span::styled(marker, theme.tone(Tone::Attention)),
                    ]
                } else {
                    vec![Span::raw(" ".repeat(indent))]
                };
                spans.extend(code_spans(&row, &mut in_code, theme.focus()));
                lines.push(Line::from(spans));
            }
        }
    }
    lines
}

/// Draws the dialog for `notes` over `area`.
pub fn render(
    theme: &Theme,
    dialog: &WhatsNewDialog,
    notes: Option<&WhatsNew>,
    frame: &mut Frame,
    area: Rect,
) {
    // Two columns of border and padding on each side.
    let width = usize::from(area.width.saturating_sub(8)).clamp(1, TEXT_WIDTH);
    let text = lines(theme, notes, width);
    let title = notes.map_or_else(|| "What's new".to_owned(), WhatsNew::title);
    // The border, a blank row above and below the text, and a row of the
    // screen above and below the box.
    let room = usize::from(area.height.saturating_sub(6)).max(1);
    let max_scroll = text.len().saturating_sub(room);
    dialog.max_scroll.set(max_scroll);
    let scroll = dialog.scroll.min(max_scroll);
    let shown = text.len().min(room);
    let hint = if max_scroll > 0 {
        format!(
            " lines {}-{} of {} · j k scroll · enter closes ",
            scroll + 1,
            scroll + shown,
            text.len()
        )
    } else {
        " enter or esc closes ".to_owned()
    };
    // Wide enough for the title and the hint in the border too.
    let content_width = text
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(text::width(&title) + 2)
        .max(text::width(&hint).saturating_sub(4));
    let outer = centered(
        area,
        u16::try_from(content_width + 6).unwrap_or(u16::MAX),
        u16::try_from(shown + 4).unwrap_or(u16::MAX),
    );
    frame.render_widget(Clear, outer);
    frame.render_widget(
        floating_box(theme)
            .title(Span::styled(
                format!(" {title} "),
                Style::new().add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::from(Span::styled(hint, theme.quiet())).right_aligned()),
        outer,
    );
    let inner = Rect::new(
        outer.x + 3,
        outer.y + 2,
        outer.width.saturating_sub(6),
        outer.height.saturating_sub(4),
    )
    .intersection(outer);
    frame.render_widget(
        Paragraph::new(text.into_iter().skip(scroll).take(room).collect::<Vec<_>>()),
        inner,
    );
    *dialog.hits.borrow_mut() = Some(outer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use devlanes_update::notes::{Entry, Release};

    /// Notes for `versions`, each with a minor and a patch entry.
    fn notes(versions: &[&str]) -> WhatsNew {
        WhatsNew {
            since: Some("0.8.0".into()),
            releases: versions
                .iter()
                .map(|version| Release {
                    version: (*version).into(),
                    entries: vec![
                        Entry {
                            level: Level::Minor,
                            line: "Removed `devlanes lane new <branch>` and the `lanes.worktree_dir` config key; devlanes no longer creates worktrees.".into(),
                            body: String::new(),
                        },
                        Entry {
                            level: Level::Patch,
                            line: "Added `devlanes reload`.".into(),
                            body: String::new(),
                        },
                    ],
                })
                .collect(),
        }
    }

    fn draw(dialog: &WhatsNewDialog, notes: Option<&WhatsNew>, width: u16, height: u16) -> String {
        let theme = Theme::default();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
                .expect("terminal");
        terminal
            .draw(|frame| render(&theme, dialog, notes, frame, frame.area()))
            .expect("draw");
        terminal.backend().to_string()
    }

    #[test]
    fn notes_show_under_their_versions_with_code_unquoted() {
        let shown = notes(&["0.9.1", "0.9.0"]);
        let screen = draw(&WhatsNewDialog::default(), Some(&shown), 100, 30);
        for wanted in [
            "What's new since 0.8.0",
            "0.9.1",
            "0.9.0",
            "breaking Removed devlanes lane new <branch>",
            "Added devlanes reload.",
            "enter or esc closes",
        ] {
            assert!(screen.contains(wanted), "{wanted}\n{screen}");
        }
        assert!(!screen.contains('`'), "{screen}");
        let screen = draw(&WhatsNewDialog::default(), None, 80, 24);
        assert!(screen.contains("no release notes"), "{screen}");
    }

    #[test]
    fn code_spans_carry_over_wrapped_lines() {
        let code = Style::new().add_modifier(Modifier::BOLD);
        let mut in_code = false;
        let first = code_spans("Run `devlanes", &mut in_code, code);
        assert!(in_code);
        assert_eq!(first[1].content, "devlanes");
        assert_eq!(first[1].style, code);
        let second = code_spans("update` now", &mut in_code, code);
        assert!(!in_code);
        assert_eq!(second[0].style, code);
        assert_eq!(second[1].style, Style::new());
    }

    #[test]
    fn keys_and_the_wheel_scroll_within_the_notes() {
        let shown = notes(&["0.9.3", "0.9.2", "0.9.1", "0.9.0"]);
        let mut dialog = WhatsNewDialog::default();
        draw(&dialog, Some(&shown), 60, 12);
        let max = dialog.max_scroll.get();
        assert!(max > 0);
        let press =
            |dialog: &mut WhatsNewDialog, code| dialog.key(KeyEvent::new(code, KeyModifiers::NONE));
        assert_eq!(press(&mut dialog, KeyCode::Char('j')), Action::Stay);
        assert_eq!(dialog.scroll, 1);
        press(&mut dialog, KeyCode::PageDown);
        assert_eq!(dialog.scroll, (11).min(max));
        press(&mut dialog, KeyCode::Char('G'));
        assert_eq!(dialog.scroll, max);
        press(&mut dialog, KeyCode::Char('j'));
        assert_eq!(dialog.scroll, max);
        press(&mut dialog, KeyCode::PageUp);
        press(&mut dialog, KeyCode::Char('g'));
        assert_eq!(dialog.scroll, 0);
        let screen = draw(&dialog, Some(&shown), 60, 12);
        assert!(
            screen.contains(&format!("lines 1-6 of {}", max + 6)),
            "{screen}"
        );

        let wheel = |kind, column| MouseEvent {
            kind,
            column,
            row: 6,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            dialog.mouse(wheel(MouseEventKind::ScrollDown, 30)),
            Action::Stay
        );
        assert_eq!(dialog.scroll, 3.min(max));
        dialog.mouse(wheel(MouseEventKind::ScrollUp, 30));
        assert_eq!(dialog.scroll, 0);
        assert_eq!(
            dialog.mouse(wheel(MouseEventKind::Down(MouseButton::Left), 30)),
            Action::Stay
        );
        assert_eq!(
            dialog.mouse(wheel(MouseEventKind::Down(MouseButton::Left), 0)),
            Action::Close
        );
        assert_eq!(press(&mut dialog, KeyCode::Esc), Action::Close);
        assert_eq!(press(&mut dialog, KeyCode::Enter), Action::Close);
        assert_eq!(press(&mut dialog, KeyCode::Char('q')), Action::Quit);
    }
}
