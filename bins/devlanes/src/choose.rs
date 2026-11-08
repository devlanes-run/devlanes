//! A command run without the argument it needs: list what it could be from the
//! daemon, then let a person pick on a terminal or print the options otherwise.
//!
//! The picker is drawn with crossterm, already a dependency of this binary (attach
//! and the TUI use it), rather than a picker crate: one screen of lines with a
//! cursor needs no more, and the selection logic lives in `devlanes_cli::Picker`.

use std::io::Write;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};
use devlanes_cli::{
    Choice, ChoiceKind, NoOptions, OutputMode, Picker, PickerKey, PickerStep, Runtime,
    lane_options, missing_argument, no_options, task_options, unit_options,
};
use devlanes_client::{Client, Paths};
use devlanes_proto::{Context, LaneSummary, Method, Page, RpcError, features};
use serde_json::json;

use crate::{driver::rpc_error, output::emit};

/// Exit status when the person cancels the picker.
pub const CANCELLED: u8 = 130;

/// What [`choose`] found.
pub enum Chosen {
    /// The values to append to the command line.
    Values(Vec<String>),
    /// Nothing more to do; the process exits with this status.
    Exit(u8),
}

/// Finds the options for `choice`, then picks on a terminal or reports them.
pub async fn choose(
    runtime: &Runtime,
    paths: &Paths,
    choice: &Choice,
    context: &Context,
    args: &[String],
) -> Result<Chosen, RpcError> {
    let cwd = context.cwd.display().to_string();
    let mut client = Client::connect_or_spawn(paths).await.map_err(rpc_error)?;
    let query = Context {
        env: None,
        ..context.clone()
    };
    let (options, lane) = match choice.kind {
        ChoiceKind::Lane => {
            let data = client
                .request(Method::LaneList, json!({}), query)
                .await
                .map_err(rpc_error)?;
            (lane_options(&data), String::new())
        }
        ChoiceKind::Task => {
            let data = client
                .request(Method::TaskList, json!({}), query)
                .await
                .map_err(rpc_error)?;
            // `null` in a project nobody started.
            let lane = data["lane"].as_str().unwrap_or("this lane").to_owned();
            (task_options(&data), lane)
        }
        kind => {
            // The daemon says which lane the command would use; an older one cannot.
            if !client.supports(features::STATUS_HERE) {
                return Err(client.daemon_outdated("choosing a unit"));
            }
            let data = client
                .request(Method::Status, json!({"here_for": choice.method}), query)
                .await
                .map_err(rpc_error)?;
            let here = data["here"].as_str().map(str::to_owned);
            let no_config = data["here_error"]["code"] == "no_config";
            let lanes: Vec<LaneSummary> = serde_json::from_value::<Page<LaneSummary>>(data)
                .map(|page| page.items)
                .unwrap_or_default();
            let found = here.and_then(|here| lanes.into_iter().find(|lane| lane.reference == here));
            let Some(lane) = found else {
                let why = if no_config {
                    NoOptions::NoConfig { cwd }
                } else {
                    NoOptions::NoLane { cwd }
                };
                emit(&no_options(choice, &why, runtime));
                return Ok(Chosen::Exit(devlanes_cli::exit::NOT_FOUND));
            };
            (unit_options(kind, &lane), lane.reference.clone())
        }
    };
    drop(client);
    if options.is_empty() {
        let rendered = no_options(choice, &NoOptions::Empty { lane }, runtime);
        let code = rendered.exit_code;
        emit(&rendered);
        return Ok(Chosen::Exit(code));
    }
    if !interactive(runtime) {
        let rendered = missing_argument(choice, &options, args, runtime);
        let code = rendered.exit_code;
        emit(&rendered);
        return Ok(Chosen::Exit(code));
    }
    let title = format!(
        "devlanes {}: choose {}{}",
        choice.name(),
        if choice.multi { "" } else { "a " },
        if choice.multi {
            format!("{}s", choice.kind.noun())
        } else {
            choice.kind.noun().to_owned()
        }
    );
    let picker = Picker::new(title, options, choice.multi);
    let picked = tokio::task::spawn_blocking(move || pick(picker))
        .await
        .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))?
        .map_err(|error| RpcError::new("io", json!({"message": error.to_string()}), 1))?;
    Ok(match picked {
        Some(values) => Chosen::Values(values),
        None => Chosen::Exit(CANCELLED),
    })
}

/// Only a person at a terminal is asked: human mode outside CI, with stdin,
/// stdout and stderr (where the picker draws) all terminals.
fn interactive(runtime: &Runtime) -> bool {
    use std::io::IsTerminal;
    runtime.mode == OutputMode::Human
        && runtime.ci.is_none()
        && runtime.stdin_tty
        && runtime.stdout_tty
        && std::io::stderr().is_terminal()
}

/// Runs the picker on stderr in raw mode; `None` when cancelled. Blocking.
fn pick(mut picker: Picker) -> std::io::Result<Option<Vec<String>>> {
    let mut out = std::io::stderr();
    terminal::enable_raw_mode()?;
    let height = terminal::size().map_or(12, |(_, rows)| usize::from(rows).clamp(4, 14));
    let mut drawn = 0;
    let result = loop {
        drawn = draw(&mut out, &picker.lines(height), drawn)?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        let Some(key) = map_key(key) else { continue };
        match picker.key(key) {
            PickerStep::Continue => {}
            PickerStep::Done(values) => break Some(values),
            PickerStep::Cancel => break None,
        }
    };
    // Leave nothing behind: the command's own output follows.
    draw(&mut out, &[], drawn)?;
    terminal::disable_raw_mode()?;
    Ok(result)
}

/// Replaces the `drawn` lines drawn last with `lines`; returns how many are on screen.
fn draw(out: &mut impl Write, lines: &[String], drawn: usize) -> std::io::Result<usize> {
    if drawn > 0 {
        crossterm::queue!(
            out,
            cursor::MoveToPreviousLine(u16::try_from(drawn).unwrap_or(u16::MAX))
        )?;
    }
    crossterm::queue!(out, terminal::Clear(terminal::ClearType::FromCursorDown))?;
    for line in lines {
        write!(out, "{line}\r\n")?;
    }
    out.flush()?;
    Ok(lines.len())
}

/// The picker's keys; everything else is ignored.
fn map_key(key: KeyEvent) -> Option<PickerKey> {
    if key.kind == event::KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char('c') if ctrl => PickerKey::Cancel,
        KeyCode::Esc => PickerKey::Cancel,
        KeyCode::Up => PickerKey::Up,
        KeyCode::Char('p') if ctrl => PickerKey::Up,
        KeyCode::Down => PickerKey::Down,
        KeyCode::Char('n') if ctrl => PickerKey::Down,
        KeyCode::Enter => PickerKey::Enter,
        KeyCode::Char(' ') => PickerKey::Toggle,
        KeyCode::Backspace => PickerKey::Backspace,
        KeyCode::Char(c) if !ctrl => PickerKey::Char(c),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Terminal keys map to picker keys; Ctrl-C cancels like Esc.
    #[test]
    fn keys_map_to_picker_keys() {
        let key = |code, modifiers| map_key(KeyEvent::new(code, modifiers));
        assert_eq!(
            key(KeyCode::Esc, KeyModifiers::NONE),
            Some(PickerKey::Cancel)
        );
        assert_eq!(
            key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(PickerKey::Cancel)
        );
        assert_eq!(
            key(KeyCode::Char(' '), KeyModifiers::NONE),
            Some(PickerKey::Toggle)
        );
        assert_eq!(
            key(KeyCode::Char('a'), KeyModifiers::NONE),
            Some(PickerKey::Char('a'))
        );
        assert_eq!(key(KeyCode::F(1), KeyModifiers::NONE), None);
    }

    /// Redrawing moves up over the old lines and clears them first.
    #[test]
    fn redraw_replaces_the_previous_lines() {
        let mut out = Vec::new();
        let drawn = draw(&mut out, &["a".into(), "b".into()], 0).expect("draw");
        assert_eq!(drawn, 2);
        let mut again = Vec::new();
        draw(&mut again, &["c".into()], drawn).expect("draw");
        let text = String::from_utf8_lossy(&again);
        assert!(text.starts_with("\x1b[2F"), "{text:?}");
        assert!(text.ends_with("c\r\n"), "{text:?}");
    }
}
