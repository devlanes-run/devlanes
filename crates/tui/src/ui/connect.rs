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

/// The label column of the form's rows, the marker included.
const LABEL: usize = 13;

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

/// `▌Name         hetzner▏`: a row of the form, the marker and the label in
/// the focus color while it has the keys, padded to `width` so the dialog
/// keeps its width as the text grows. A value too long for the row shows
/// its end; an empty one shows `placeholder` dimmed.
fn input_row(
    theme: &Theme,
    label: &str,
    value: &str,
    placeholder: &str,
    focused: bool,
    width: usize,
) -> Line<'static> {
    let (marker, label_style) = if focused {
        (theme.glyph(Glyph::Marker), theme.focus())
    } else {
        (" ", theme.quiet())
    };
    let room = width.saturating_sub(LABEL + 1).max(2);
    let mut spans = vec![
        Span::styled(marker, theme.focus()),
        Span::styled(format!("{label:<w$}", w = LABEL - 1), label_style),
    ];
    let chars: Vec<char> = value.chars().collect();
    if chars.is_empty() {
        if focused {
            spans.push(Span::raw(cursor(theme)));
        }
        spans.push(Span::styled(text::clip(placeholder, room), theme.quiet()));
    } else {
        // The end of the text stays in view while typing.
        let shown: String = if chars.len() > room {
            let tail: String = chars[chars.len() - (room - 1)..].iter().collect();
            format!("…{tail}")
        } else {
            value.to_owned()
        };
        spans.push(Span::raw(shown));
        if focused {
            spans.push(Span::raw(cursor(theme)));
        }
    }
    pad(spans, width)
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
            (
                format!("connect {host}"),
                body,
                vec![keys(theme, &[("esc", "close")])],
            )
        }
        Step::Waiting => waiting(app, dialog, width),
        Step::Prompt(prompt) => question(theme, &host, prompt, width),
        Step::Failed(worded) => {
            let mut body = wrapped(
                &worded.summary,
                width,
                theme.tone(Tone::Failure).add_modifier(Modifier::BOLD),
            );
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
    let ascii = theme.glyphs == Glyphs::Ascii;
    let option = |reach: Reach, label: &str| {
        let on = form.reach == reach;
        let mark = match (on, ascii) {
            (true, false) => "(•)",
            (true, true) => "(*)",
            (false, _) => "( )",
        };
        let style = if on {
            Style::new().add_modifier(Modifier::BOLD)
        } else {
            theme.quiet()
        };
        Span::styled(format!("{mark} {label}"), style)
    };
    let focused = form.field == Field::Reach;
    let (marker, label_style) = if focused {
        (theme.glyph(Glyph::Marker), theme.focus())
    } else {
        (" ", theme.quiet())
    };
    let mut body = vec![pad(
        vec![
            Span::styled(marker, theme.focus()),
            Span::styled(format!("{:<w$}", "Reach it by", w = LABEL - 1), label_style),
            option(Reach::Ssh, "SSH name"),
            Span::raw("   "),
            option(Reach::Command, "a command"),
        ],
        width,
    )];
    body.push(Line::from(""));
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
    let about = match form.reach {
        Reach::Ssh => {
            "devlanes runs ssh with this name, so your SSH config applies. devlanes never edits it."
                .to_owned()
        }
        Reach::Command => {
            let name = match clean(form.name.trim()) {
                name if name.is_empty() => "<name>".to_owned(),
                name => name,
            };
            format!(
                "The command must run devlanes tunnel --stdio in the box. It is saved as remote.boxes.{name}.command in config.yaml."
            )
        }
    };
    body.extend(wrapped(&about, width, theme.quiet()));
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
        let lead = if prompt.text.starts_with("new ") {
            format!("{host} needs a new pairing code.")
        } else {
            format!("{host} needs a pairing code.")
        };
        body.extend(wrapped(&lead, width, bold));
        body.extend(wrapped(
            &format!("Run devlanes daemon pair on {host}, then type the code here."),
            width,
            theme.quiet(),
        ));
        "Code"
    } else {
        body.extend(wrapped(&format!("{host} asks:"), width, bold));
        for line in prompt.text.lines() {
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
                let body = wrapped("This code expired. p gets a new one.", width, Style::new());
                (body, vec![("p", "new code"), close])
            } else {
                let mut body = vec![
                    Line::from(vec![
                        Span::raw("  "),
                        Span::styled(code.clone(), bold.patch(theme.focus())),
                    ]),
                    Line::from(""),
                ];
                let expires = match left {
                    Some(ms) => format!("It works once and expires in {}.", countdown(ms)),
                    None => "It works once.".to_owned(),
                };
                body.extend(wrapped(&expires, width, Style::new()));
                body.push(Line::from(""));
                body.extend(wrapped(
                    "On the other machine, run this with its SSH name for this one:",
                    width,
                    theme.quiet(),
                ));
                body.extend(wrapped(
                    &format!("devlanes remote connect <box> --code {code}"),
                    width,
                    theme.key(),
                ));
                body.push(Line::from(""));
                body.extend(wrapped(
                    "Or press n in its boxes view. It asks for the code.",
                    width,
                    theme.quiet(),
                ));
                (body, vec![("y", "copy command"), ("p", "new code"), close])
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
                "The remote settings in config.yaml are invalid, so they count as off. devlanes doctor shows the problem."
            } else {
                "remote.allow_connections is false here. Set it to true to pair another machine."
            };
            body.extend(wrapped(why, width, Style::new()));
            (body, vec![close])
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
