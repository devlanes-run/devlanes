//! The boxes view's modals on screen: the new connection form, the attached
//! `connect` with its questions and outcome, and the pairing code for
//! another machine. Each returns the dialog's title, body and keys for
//! [`super::dialog::render`] to fit and frame.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::dialog::keys;
use super::fit;
use crate::app::App;
use crate::connect::{ConnectDialog, Field, PairDialog, PairState, Prompt, Reach, Step, countdown};
use crate::text::{self, clean};
use crate::theme::{Glyph, Glyphs, Theme, Tone};

/// What a dialog shows: its title, its body and its keys.
type Parts = (String, Vec<Line<'static>>, Vec<Line<'static>>);

/// The label column of the form's rows: the marker, a space and the label.
const LABEL: usize = 12;

/// `text` wrapped in `width` cells, in `style`.
fn wrapped(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    text::wrap(&clean(text), width.max(1))
        .into_iter()
        .map(|line| Line::styled(line, style))
        .collect()
}

/// The typing cursor.
fn cursor(theme: &Theme) -> &'static str {
    if theme.glyphs == Glyphs::Ascii {
        "_"
    } else {
        "▏"
    }
}

/// `▌ Name       ` or `  Name       `: the marker and the label of a form
/// row, as the options page starts its rows.
fn label_spans(theme: &Theme, label: &str, focused: bool) -> Vec<Span<'static>> {
    let marker = if focused {
        theme.glyph(Glyph::Marker)
    } else {
        " "
    };
    vec![
        Span::styled(marker, theme.focus()),
        Span::raw(format!(" {label:<w$}", w = LABEL - 2)),
    ]
}

/// What a placeholder looks like: dimmer than typed text, and slanted where
/// the terminal can, so it never reads as a value.
fn placeholder_style(theme: &Theme) -> Style {
    theme.quiet().add_modifier(Modifier::ITALIC)
}

/// `▌ Name        hetzner▏      `: a text row of a form, padded to `width`
/// so the dialog keeps its width as the text grows. While it has the keys
/// the row is selected as the options page selects one, and its field is
/// cut out of the selection like the value being edited there. A value too
/// long for the field shows its end; an empty one shows `placeholder`.
fn input_row(
    theme: &Theme,
    label: &str,
    value: &str,
    placeholder: &str,
    focused: bool,
    width: usize,
) -> Line<'static> {
    let mut spans = label_spans(theme, label, focused);
    // A space each side of the text inside the field.
    let room = width.saturating_sub(LABEL + 2).max(2);
    let field = if focused {
        crate::settings::editing_style(theme)
    } else {
        Style::new()
    };
    let mut cells = vec![Span::styled(" ", field)];
    let chars: Vec<char> = value.chars().collect();
    if chars.is_empty() {
        if focused {
            cells.push(Span::styled(cursor(theme), field));
        }
        let hint = text::clip(placeholder, room.saturating_sub(1));
        let style = if focused {
            field
                .remove_modifier(Modifier::BOLD)
                .patch(placeholder_style(theme))
        } else {
            placeholder_style(theme)
        };
        cells.push(Span::styled(hint, style));
    } else {
        // The end of the text stays in view while typing.
        let room = room.saturating_sub(usize::from(focused));
        let shown: String = if chars.len() > room {
            let tail: String = chars[chars.len() - (room - 1)..].iter().collect();
            format!("…{tail}")
        } else {
            value.to_owned()
        };
        cells.push(Span::styled(shown, field));
        if focused {
            cells.push(Span::styled(cursor(theme), field));
        }
    }
    if !focused {
        spans.extend(cells);
        return pad(spans, width);
    }
    let used = super::spans_width(&cells);
    cells.push(Span::styled(
        " ".repeat(width.saturating_sub(LABEL + used)),
        field,
    ));
    let mut spans = selected(theme, spans);
    spans.extend(cells);
    fit(spans, width)
}

/// `▌ Reach by    (•) SSH name    ( ) command`: how the box is reached, the
/// choice in bold. While the row has the keys it is selected and the choice
/// is cut out of the selection like a field.
fn reach_row(theme: &Theme, reach: Reach, focused: bool, width: usize) -> Line<'static> {
    let ascii = theme.glyphs == Glyphs::Ascii;
    let mut spans = label_spans(theme, "Reach by", focused);
    if focused {
        spans = selected(theme, spans);
    }
    let pick = if focused {
        crate::settings::editing_style(theme)
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    };
    let other = if focused {
        theme.selection()
    } else {
        theme.quiet()
    };
    for (index, (option, label)) in [(Reach::Ssh, "SSH name"), (Reach::Command, "command")]
        .into_iter()
        .enumerate()
    {
        let on = option == reach;
        let mark = match (on, ascii) {
            (true, false) => "(•)",
            (true, true) => "(*)",
            (false, _) => "( )",
        };
        if index > 0 {
            spans.push(Span::styled("  ", other));
        }
        spans.push(Span::styled(
            format!(" {mark} {label} "),
            if on { pick } else { other },
        ));
    }
    if focused {
        let used = super::spans_width(&spans);
        spans.push(Span::styled(
            " ".repeat(width.saturating_sub(used)),
            theme.selection(),
        ));
    }
    pad(spans, width)
}

/// `spans` in the selection's colors, as a selected row of the options page.
fn selected(theme: &Theme, spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    spans
        .into_iter()
        .map(|span| span.patch_style(theme.selection()))
        .collect()
}

/// `text`'s first sentence and what follows it, each ending in a period:
/// `hetzner does not accept connections. Set … there` gives
/// `hetzner does not accept connections.` and `Set … there.`.
fn sentences(text: &str) -> (String, Option<String>) {
    let stop = |part: &str| {
        let part = part.trim();
        if part.ends_with(['.', '?', '!']) {
            part.to_owned()
        } else {
            format!("{part}.")
        }
    };
    match text.split_once(". ") {
        Some((head, rest)) if !rest.trim().is_empty() => (stop(head), Some(stop(rest))),
        _ => (stop(text), None),
    }
}

/// `spans` cut to `width` cells, then filled with spaces to it.
fn pad(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut line = fit(spans, width);
    let used = line.width();
    if used < width {
        line.spans.push(Span::raw(" ".repeat(width - used)));
    }
    line
}

/// The connect modal's parts for its step.
pub(super) fn connect(app: &App, dialog: &ConnectDialog, width: usize) -> Parts {
    let theme = &app.theme;
    let host = clean(&dialog.host);
    match &dialog.step {
        Step::Form => form(theme, dialog, width),
        Step::Saving => {
            let body = vec![Line::from(vec![
                Span::styled(
                    format!("{} ", theme.spinner(app.spin)),
                    theme.tone(Tone::Busy),
                ),
                Span::raw(format!("saving remote.boxes.{host}.command…")),
            ])];
            // It takes a moment and then connects, so no key stops it.
            (format!("connect {host}"), body, Vec::new())
        }
        Step::Waiting => waiting(app, dialog, width),
        Step::Prompt(prompt) => question(theme, &host, prompt, width),
        Step::Failed(worded) => {
            // The first sentence says what happened; what to do follows plain.
            let (head, rest) = sentences(&worded.summary);
            let mut body = wrapped(
                &head,
                width,
                theme.tone(Tone::Failure).add_modifier(Modifier::BOLD),
            );
            if let Some(rest) = rest {
                body.extend(wrapped(&rest, width, Style::new()));
            }
            for detail in &worded.detail {
                body.extend(wrapped(detail, width, Style::new()));
            }
            if let Some(hint) = &worded.hint {
                body.extend(wrapped(&format!("run: {hint}"), width, theme.quiet()));
            }
            let tail = vec![keys(theme, &[("enter", "try again"), ("esc", "close")])];
            (format!("connect {host}"), body, tail)
        }
    }
}

/// The new connection form: how the box is reached, its name, its command
/// for a box reached by one, what that means, and what is wrong.
fn form(theme: &Theme, dialog: &ConnectDialog, width: usize) -> Parts {
    let form = &dialog.form;
    let focused = form.field == Field::Reach;
    let mut body = vec![reach_row(theme, form.reach, focused, width)];
    let name_hint = match form.reach {
        Reach::Ssh => "hetzner or me@203.0.113.4",
        Reach::Command => "a name for it, like acme",
    };
    body.push(input_row(
        theme,
        "Name",
        &form.name,
        name_hint,
        form.field == Field::Name,
        width,
    ));
    if form.reach == Reach::Command {
        body.push(input_row(
            theme,
            "Command",
            &form.command,
            "docker exec -i acme devlanes tunnel --stdio",
            form.field == Field::Command,
            width,
        ));
    }
    body.push(Line::from(""));
    let name = match clean(form.name.trim()) {
        name if name.is_empty() => "<name>".to_owned(),
        name => name,
    };
    let about = match form.reach {
        Reach::Ssh => vec![format!(
            "devlanes runs ssh {name}, so your SSH config applies."
        )],
        Reach::Command => vec![
            "The command must run devlanes tunnel --stdio in the box.".to_owned(),
            format!("devlanes saves it as remote.boxes.{name}.command."),
        ],
    };
    for line in about {
        body.extend(wrapped(&line, width, theme.quiet()));
    }
    if form.configured().is_some() {
        body.push(Line::from(""));
        body.extend(wrapped(
            &format!(
                "{} has a command in config.yaml, so devlanes runs that instead of ssh.",
                clean(form.name.trim())
            ),
            width,
            theme.tone(Tone::Attention),
        ));
    }
    if let Some(error) = &form.error {
        body.push(Line::from(""));
        body.extend(wrapped(error, width, theme.tone(Tone::Attention)));
    }
    let mut pairs = vec![("tab", "next field")];
    if focused {
        pairs.push(("space", "switch"));
    }
    pairs.extend([("enter", "connect"), ("esc", "cancel")]);
    ("connect a box".to_owned(), body, vec![keys(theme, &pairs)])
}

/// The attached `connect` while the link settles: its state, the last
/// error, ssh's notices, and that closing leaves it to the daemon.
fn waiting(app: &App, dialog: &ConnectDialog, width: usize) -> Parts {
    let theme = &app.theme;
    let link = &dialog.link;
    let mut body = vec![Line::from(vec![
        Span::styled(
            format!("{} ", theme.spinner(app.spin)),
            theme.tone(Tone::Busy),
        ),
        Span::raw(link.words()),
    ])];
    if let Some(error) = &link.error {
        body.extend(wrapped(error, width, theme.tone(Tone::Attention)));
    }
    for note in &link.notes {
        body.extend(wrapped(note, width, theme.quiet()));
    }
    body.push(Line::from(""));
    body.extend(wrapped(
        "The daemon keeps connecting if you close this.",
        width,
        theme.quiet(),
    ));
    let title = format!("connect {}", clean(&dialog.host));
    (title, body, vec![keys(theme, &[("esc", "close")])])
}

/// A question of the box's link: a pairing code in this modal's words,
/// anything else as ssh asked it, with the answer masked for a secret.
fn question(theme: &Theme, host: &str, prompt: &Prompt, width: usize) -> Parts {
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let mut body = Vec::new();
    let label = if prompt.kind == "code" {
        let new = prompt
            .text
            .first()
            .is_some_and(|line| line.starts_with("new "));
        let lead = if new {
            format!("{host} needs a new pairing code.")
        } else {
            format!("{host} needs a pairing code.")
        };
        body.extend(wrapped(&lead, width, bold));
        for line in [
            format!("On {host}, press p in its Boxes view."),
            "Or run devlanes daemon pair there.".to_owned(),
        ] {
            body.extend(wrapped(&line, width, theme.quiet()));
        }
        "Code"
    } else {
        body.extend(wrapped("ssh asks", width, theme.quiet()));
        for line in &prompt.text {
            body.extend(wrapped(line, width, Style::new()));
        }
        "Answer"
    };
    body.push(Line::from(""));
    let shown = if prompt.masked() {
        "•".repeat(prompt.input.chars().count())
    } else {
        prompt.input.clone()
    };
    body.push(input_row(theme, label, &shown, "", true, width));
    if prompt.masked() {
        body.push(Line::from(""));
        body.extend(wrapped(
            "The answer goes to ssh. devlanes does not keep it.",
            width,
            theme.quiet(),
        ));
    }
    let tail = vec![keys(theme, &[("enter", "send"), ("esc", "close")])];
    (format!("connect {host}"), body, tail)
}

/// The pairing code modal: the code with its countdown and what to run on
/// the other machine, the wait while pairing is paused, or why this
/// machine gives no code.
pub(super) fn pair(app: &App, dialog: &PairDialog, width: usize) -> Parts {
    let theme = &app.theme;
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let close = ("esc", "close");
    let (body, pairs) = match &dialog.state {
        PairState::Loading => (
            vec![Line::styled("getting a code…", theme.quiet())],
            vec![close],
        ),
        PairState::Code { code, valid_ms } => {
            let left = valid_ms.map(|valid| dialog.left(valid));
            if left == Some(0) {
                let body = wrapped("This code expired.", width, bold);
                (body, vec![("p", "new code"), close])
            } else {
                // The code is what the person reads out or types, so it
                // stands alone in the middle, in the attention color.
                let mut body = vec![
                    Line::from(""),
                    Line::styled(code.clone(), bold.patch(theme.tone(Tone::Attention))).centered(),
                ];
                let expires = match left {
                    Some(ms) => format!("It works once and expires in {}.", countdown(ms)),
                    None => "It works once.".to_owned(),
                };
                body.extend(
                    wrapped(&expires, width, theme.quiet())
                        .into_iter()
                        .map(Line::centered),
                );
                body.push(Line::from(""));
                body.extend(wrapped("On the other machine, run", width, Style::new()));
                body.extend(wrapped(
                    &format!("  devlanes remote connect <box> --code {code}"),
                    width,
                    theme.key(),
                ));
                for line in [
                    "<box> is this machine's SSH name there.",
                    "Or press n in its Boxes view and type the code.",
                ] {
                    body.extend(wrapped(line, width, theme.quiet()));
                }
                (body, vec![("y", "copy code"), ("p", "new code"), close])
            }
        }
        PairState::Paused { wait_ms } => {
            let left = dialog.left(*wait_ms);
            let mut body = wrapped("Pairing is paused after 5 wrong codes.", width, bold);
            let again = if left > 0 {
                format!("You can get a code again in {}.", countdown(left))
            } else {
                "You can get a code again now.".to_owned()
            };
            body.extend(wrapped(&again, width, Style::new()));
            (body, vec![("p", "try again"), close])
        }
        PairState::Off { invalid } => {
            let mut body = wrapped("Connections to this machine are off.", width, bold);
            let why = if *invalid {
                "config.yaml has invalid remote settings. Run devlanes doctor to see them."
            } else {
                "Turn on remote.allow_connections to pair another machine."
            };
            body.extend(wrapped(why, width, Style::new()));
            (body, vec![("o", "options"), close])
        }
        PairState::Failed(worded) => {
            let mut body = wrapped(&worded.summary, width, theme.tone(Tone::Failure));
            for detail in &worded.detail {
                body.extend(wrapped(detail, width, theme.quiet()));
            }
            (body, vec![("p", "try again"), close])
        }
    };
    ("pairing code".to_owned(), body, vec![keys(theme, &pairs)])
}
