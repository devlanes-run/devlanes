//! Release highlights: the notes the binary hands over in
//! [`crate::Options`], shown once after the welcome when this devlanes is
//! newer than the one seen before, and from the Esc menu. Breaking entries
//! first, marked `breaking`, then the highlights, inline code in the focus
//! color; `o` opens the release page. j/k, PgUp/PgDn and the wheel scroll a
//! long list.

use std::cell::{Cell, RefCell};

use devlanes_update::notes::WhatsNew;
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::keys::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
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
    /// Open the release page in the browser and keep the dialog.
    OpenPage,
}

impl WhatsNewDialog {
    /// j/k and the arrows scroll a line, PgUp/PgDn ten, g/G to either end;
    /// `o` opens the release page; Enter or Esc closes; `q` and Ctrl+C quit
    /// as they do everywhere else.
    pub fn key(&mut self, key: KeyEvent) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let max = self.max_scroll.get();
        // The box may have grown since the scroll was set.
        self.scroll = self.scroll.min(max);
        match key.code {
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('o') => return Action::OpenPage,
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

/// Every line of `notes` wrapped to `width`: the breaking entries, then the
/// highlights, each as a bullet with a hanging indent. A highlight is its
/// bold title with the body under it; a breaking entry without one is its
/// first line.
fn lines(theme: &Theme, notes: Option<&WhatsNew>, width: usize) -> Vec<Line<'static>> {
    let Some(notes) = notes.filter(|notes| !notes.is_empty()) else {
        return vec![Line::styled(
            "This build carries no release highlights.",
            theme.quiet(),
        )];
    };
    let summary = notes.summary();
    let bullet = format!("{} ", theme.glyph(Glyph::Folded));
    let mut lines = Vec::new();
    let mut previous_titled = false;
    for item in summary.breaking.iter().chain(&summary.highlights) {
        let titled = item.title.is_some();
        // Plain breaking lines sit together; anything with a title stands apart.
        if !lines.is_empty() && (titled || previous_titled) {
            lines.push(Line::from(""));
        }
        previous_titled = titled;
        let marker = if item.breaking { BREAKING } else { "" };
        let indent = text::width(&bullet);
        let lead = |theme: &Theme| {
            vec![
                Span::styled(bullet.clone(), theme.quiet()),
                Span::styled(marker, theme.tone(Tone::Attention)),
            ]
        };
        let mut in_code = false;
        match &item.title {
            Some(title) => {
                let mut spans = lead(theme);
                spans.push(Span::styled(
                    clean(title),
                    Style::new().add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::from(spans));
                for row in text::wrap(&clean(&item.text), width.saturating_sub(indent)) {
                    let mut spans = vec![Span::raw(" ".repeat(indent))];
                    spans.extend(code_spans(&row, &mut in_code, theme.focus()));
                    lines.push(Line::from(spans));
                }
            }
            None => {
                let hanging = indent + marker.len();
                let rows = text::wrap(&clean(&item.text), width.saturating_sub(hanging));
                for (index, row) in rows.into_iter().enumerate() {
                    let mut spans = if index == 0 {
                        lead(theme)
                    } else {
                        vec![Span::raw(" ".repeat(hanging))]
                    };
                    spans.extend(code_spans(&row, &mut in_code, theme.focus()));
                    lines.push(Line::from(spans));
                }
            }
        }
    }
    if summary.more > 0 {
        let noun = if summary.more == 1 {
            "highlight"
        } else {
            "highlights"
        };
        lines.push(Line::from(""));
        lines.push(Line::styled(
            format!("and {} more {noun} on the release page", summary.more),
            theme.quiet(),
        ));
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
    let title = notes.map_or_else(|| "Release highlights".to_owned(), WhatsNew::title);
    // The border, a blank row above and below the text, and a row of the
    // screen above and below the box.
    let room = usize::from(area.height.saturating_sub(6)).max(1);
    let max_scroll = text.len().saturating_sub(room);
    dialog.max_scroll.set(max_scroll);
    let scroll = dialog.scroll.min(max_scroll);
    let shown = text.len().min(room);
    // Without notes there is no release to open.
    let has_page = notes.is_some_and(|notes| !notes.is_empty());
    let hint = if max_scroll > 0 {
        // Short, so it still fits the border of the narrowest box.
        let page = if has_page { "o page · " } else { "" };
        format!(
            " lines {}-{} of {} · j k scroll · {page}enter closes ",
            scroll + 1,
            scroll + shown,
            text.len()
        )
    } else {
        let page = if has_page { "o release page · " } else { "" };
        format!(" {page}enter or esc closes ")
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
    use devlanes_update::notes::{Entry, Highlight, Level, Release};

    /// Notes for `versions`, each with a breaking entry, a patch entry that
    /// is a highlight and one that is not.
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
                            line: "Removed `devlanes lane new <branch>` and the `lanes.worktree_dir` config key.".into(),
                            body: String::new(),
                            highlight: None,
                        },
                        Entry {
                            level: Level::Patch,
                            line: "Added `devlanes reload`.".into(),
                            body: String::new(),
                            highlight: Some(Highlight {
                                title: format!("Reload a lane in {version}"),
                                body: "Run `devlanes reload` to apply project file edits.".into(),
                            }),
                        },
                        Entry {
                            level: Level::Patch,
                            line: "Fixed a crash that stays out of the dialog.".into(),
                            body: String::new(),
                            highlight: None,
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
    fn breaking_entries_come_first_then_highlights_with_code_unquoted() {
        let shown = notes(&["0.9.1", "0.9.0"]);
        let screen = draw(&WhatsNewDialog::default(), Some(&shown), 100, 30);
        for wanted in [
            "Release highlights since 0.8.0",
            "breaking Removed devlanes lane new <branch>",
            "Reload a lane in 0.9.1",
            "Reload a lane in 0.9.0",
            "Run devlanes reload to apply project file edits.",
            "o release page · enter or esc closes",
        ] {
            assert!(screen.contains(wanted), "{wanted}\n{screen}");
        }
        // Ordinary entries stay on the release page.
        assert!(!screen.contains("stays out"), "{screen}");
        assert!(!screen.contains('`'), "{screen}");
        let breaking = screen.find("breaking Removed").expect("breaking");
        assert!(breaking < screen.find("Reload a lane").expect("highlight"));
        let screen = draw(&WhatsNewDialog::default(), None, 80, 24);
        assert!(screen.contains("no release highlights"), "{screen}");
        assert!(!screen.contains("release page"), "{screen}");
    }

    #[test]
    fn highlights_past_the_limit_point_to_the_release_page() {
        let versions = [
            "0.9.7", "0.9.6", "0.9.5", "0.9.4", "0.9.3", "0.9.2", "0.9.1",
        ];
        let screen = draw(&WhatsNewDialog::default(), Some(&notes(&versions)), 100, 60);
        assert!(
            screen.contains("and 2 more highlights on the release page"),
            "{screen}"
        );
        assert!(!screen.contains("Reload a lane in 0.9.1"), "{screen}");
    }

    #[test]
    fn a_highlight_that_breaks_is_listed_once_in_its_own_words() {
        let mut shown = notes(&["0.9.1"]);
        shown.releases[0].entries[0].highlight = Some(Highlight {
            title: "Setup replaces certs".into(),
            body: "Run `devlanes setup` instead of `devlanes certs trust`.".into(),
        });
        let screen = draw(&WhatsNewDialog::default(), Some(&shown), 100, 30);
        assert!(screen.contains("breaking Setup replaces certs"), "{screen}");
        assert!(
            screen.contains("Run devlanes setup instead of devlanes certs trust."),
            "{screen}"
        );
        assert!(!screen.contains("lanes.worktree_dir"), "{screen}");
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
        assert_eq!(press(&mut dialog, KeyCode::Char('o')), Action::OpenPage);
    }
}
