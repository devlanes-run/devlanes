//! What the shell shows once the TUI closed: the devlanes banner, the lane
//! that was on screen, whether the daemon still runs, a newer release with
//! the command that installs it, and the command that stops the lane while it
//! runs.

use std::{
    fmt::Write as _,
    io::{self, Write as _},
    time::Duration,
};

use devlanes_proto::{LaneSummary, release::UpdateInfo};
use devlanes_update::{InstallMethod, update_command, version::is_newer};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{
    ansi,
    api::{ApiError, DaemonApi},
    banner,
    steps::INDENT,
    theme::{Theme, Tone},
    tree,
};

/// How long the daemon gets to answer `status` and `daemon.status` before
/// the summary says it did not.
const STATUS_TIMEOUT: Duration = Duration::from_secs(1);
/// The banner's tallest font is too much for a shell: the medium and small
/// ones fit in this many rows.
const BANNER_ROWS: u16 = 5;
/// Columns of a line's label, the gap after it included.
const LABEL: usize = 10;

/// The daemon as it answered after the TUI closed.
#[derive(Debug, Clone)]
pub enum Daemon {
    /// It answered with every lane it knows, the newer release its
    /// `daemon.status` reported, if any, and how soon it exits on its own
    /// when nothing keeps it up ([`stopping_in`]).
    Running {
        lanes: Vec<LaneSummary>,
        update: Option<UpdateInfo>,
        stopping_in: Option<Duration>,
    },
    /// Nothing listens on its socket any more.
    Stopped,
    /// It is there but `status` failed or took too long.
    Unanswered,
}

impl Daemon {
    /// Asks `api` for every lane and for its status, within [`STATUS_TIMEOUT`].
    /// A `daemon.status` that fails or does not decode only leaves out the
    /// release.
    pub async fn ask(api: &dyn DaemonApi) -> Self {
        let both = async { tokio::join!(api.status(), api.daemon_status()) };
        match tokio::time::timeout(STATUS_TIMEOUT, both).await {
            Ok((Ok(lanes), status)) => {
                let status = status.ok();
                Self::Running {
                    lanes,
                    update: status
                        .as_ref()
                        .and_then(|status| status.get("update").cloned())
                        .and_then(|update| serde_json::from_value(update).ok()),
                    stopping_in: status.as_ref().and_then(stopping_in),
                }
            }
            Ok((Err(ApiError::Disconnected(_)), _)) => Self::Stopped,
            Ok((Err(_), _)) | Err(_) => Self::Unanswered,
        }
    }
}

/// How soon the daemon exits on its own now that this TUI closed, from
/// `daemon.status`'s `idle_exit`; `None` for a daemon that never does, or
/// while lanes, boxes or another watcher keep it up. This TUI's own event
/// subscription may not have ended yet when the daemon answers, so one
/// watcher is taken to be this TUI.
fn stopping_in(status: &serde_json::Value) -> Option<Duration> {
    let idle_exit = status.get("idle_exit")?;
    let idle = idle_exit.get("idle")?.as_bool()?;
    let watchers = idle_exit.get("watchers")?.as_u64()?;
    let after = idle_exit.get("after_ms")?.as_u64()?;
    (idle && watchers <= 1).then(|| Duration::from_millis(after))
}

/// `15s`, or whole minutes from a minute on.
fn short_duration(duration: Duration) -> String {
    match duration.as_secs() {
        secs @ 0..60 => format!("{secs}s"),
        secs => format!("{} min", secs / 60),
    }
}

/// Asks the daemon how things stand and prints the summary on stdout, for
/// `lane`, the lane the TUI showed last, and `method`, how devlanes was
/// installed (`None` before the TUI found out). Runs after the terminal left
/// TUI mode; a failed write is dropped, as the TUI already closed.
pub async fn print(
    api: &dyn DaemonApi,
    theme: &Theme,
    lane: Option<&str>,
    method: Option<&InstallMethod>,
) {
    let daemon = Daemon::ask(api).await;
    let width = crossterm::terminal::size().map_or(80, |(width, _)| width);
    let text = render(theme, width, lane, method, &daemon);
    let mut stdout = io::stdout();
    let _ = stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush());
}

/// The summary for a terminal `width` columns wide, with SGR sequences for
/// its colors and a blank line around it.
pub fn render(
    theme: &Theme,
    width: u16,
    lane: Option<&str>,
    method: Option<&InstallMethod>,
    daemon: &Daemon,
) -> String {
    let size = banner::Size::pick(theme, width.saturating_sub(INDENT), BANNER_ROWS);
    let (banner_width, banner_rows) = size.dimensions();
    let lines = lines(theme, lane, method, daemon);
    let text_width = lines.iter().map(Line::width).max().unwrap_or(0);
    let content = u16::try_from(text_width)
        .unwrap_or(u16::MAX)
        .max(banner_width);
    let height = banner_rows + 1 + u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let area = Rect::new(0, 0, INDENT.saturating_add(content), height);
    let mut buf = Buffer::empty(area);
    banner::render(
        &mut buf,
        Rect::new(INDENT, 0, banner_width, banner_rows),
        theme,
        size,
    );
    for (y, line) in (banner_rows + 1..).zip(&lines) {
        buf.set_line(INDENT, y, line, content);
    }
    format!("\n{}\n", ansi::text(&buf))
}

/// The lane, daemon, update and stop lines; the lane's state only when the
/// daemon still knows it, the update only when the release is newer than this
/// binary.
fn lines(
    theme: &Theme,
    lane: Option<&str>,
    method: Option<&InstallMethod>,
    daemon: &Daemon,
) -> Vec<Line<'static>> {
    let label = |text: &str| Span::styled(format!("{text:<LABEL$}"), theme.quiet());
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let (lanes, update) = match daemon {
        Daemon::Running { lanes, update, .. } => (lanes.as_slice(), update.as_ref()),
        Daemon::Stopped | Daemon::Unanswered => (&[][..], None),
    };
    let known = lane.and_then(|reference| lanes.iter().find(|l| l.reference == reference));
    let mut out = Vec::new();
    if let Some(reference) = lane {
        let mut spans = vec![label("Lane"), Span::styled(reference.to_owned(), bold)];
        if let Some(known) = known {
            let (state, tone) = lane_state(known);
            spans.push(Span::raw("  "));
            spans.push(Span::styled(state, theme.tone(tone)));
        }
        out.push(Line::from(spans));
    }
    let mut spans = vec![label("Daemon")];
    match daemon {
        Daemon::Running {
            lanes, stopping_in, ..
        } => {
            let live = lanes.iter().filter(|l| tree::lane_is_live(l)).count();
            let running = match live {
                0 => ", no lanes running".to_owned(),
                1 => ", 1 lane running".to_owned(),
                n => format!(", {n} lanes running"),
            };
            // "running" alone reads as running until stopped by hand.
            match stopping_in {
                Some(after) => spans.push(Span::styled(
                    format!("stopping in {}", short_duration(*after)),
                    theme.tone(Tone::Busy),
                )),
                None => spans.push(Span::styled("running", theme.tone(Tone::Ok))),
            }
            spans.push(Span::styled(running, theme.quiet()));
        }
        Daemon::Stopped => spans.push(Span::styled("not running", theme.quiet())),
        Daemon::Unanswered => {
            spans.push(Span::styled("not answering", theme.tone(Tone::Attention)));
        }
    }
    out.push(Line::from(spans));
    if let Some(update) = update.filter(|u| is_newer(&u.latest, devlanes_version::VERSION)) {
        let breaking = if update.bump == "minor" {
            ", breaking changes"
        } else {
            ""
        };
        // Installed in the background, the next `devlanes` switches to it.
        let (state, command) = if update.state == "ready" {
            let version = update.installed.as_deref().unwrap_or(&update.latest);
            (
                format!("{version} ready"),
                Some("run devlanes to switch to it".to_owned()),
            )
        } else {
            (
                format!("{} available{breaking}", update.latest),
                method.map(|method| update_command(method, &update.latest)),
            )
        };
        let mut spans = vec![
            label("Update"),
            Span::styled(state, theme.tone(Tone::Attention)),
        ];
        if let Some(command) = command {
            spans.push(Span::styled(" · ", theme.quiet()));
            spans.push(Span::styled(command, bold));
        }
        out.push(Line::from(spans));
    }
    if let Some(known) = known.filter(|l| tree::lane_is_live(l)) {
        out.push(Line::from(vec![
            label("Stop"),
            Span::styled(format!("devlanes stop --lane {}", known.reference), bold),
        ]));
    }
    out
}

/// The lane's state word, with how many of its services and containers are
/// up while it runs, and its tone.
fn lane_state(lane: &LaneSummary) -> (String, Tone) {
    let counts = tree::counts(lane);
    let tone = match lane.state.as_str() {
        "running" => Tone::Ok,
        "failed" => Tone::Failure,
        "starting" | "stopping" => Tone::Busy,
        _ => Tone::Quiet,
    };
    let mut text = lane.state.clone();
    if tree::lane_is_live(lane) && counts.total > 0 {
        let _ = write!(text, ", {}/{} up", counts.up, counts.total);
    }
    if counts.failed > 0 {
        let _ = write!(text, ", {} failed", counts.failed);
    }
    (text, tone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Glyphs, Tier};

    fn theme(tier: Tier, glyphs: Glyphs) -> Theme {
        Theme { tier, glyphs }
    }

    fn lane(reference: &str, state: &str) -> LaneSummary {
        LaneSummary {
            reference: reference.into(),
            project: "acme".into(),
            index: Some(0),
            state: state.into(),
            ..LaneSummary::default()
        }
    }

    /// A daemon that runs `lanes` and reports no release.
    fn running(lanes: Vec<LaneSummary>) -> Daemon {
        Daemon::Running {
            lanes,
            update: None,
            stopping_in: None,
        }
    }

    /// A release `latest`, `bump` away.
    fn release(latest: &str, bump: &str) -> UpdateInfo {
        UpdateInfo {
            latest: latest.into(),
            bump: bump.into(),
            state: "available".into(),
            installed: None,
            error: None,
            checked_at: 1,
            notes: Vec::new(),
        }
    }

    /// The summary without its SGR sequences, trailing blank lines cut.
    fn text(
        theme: &Theme,
        width: u16,
        lane: Option<&str>,
        method: Option<&InstallMethod>,
        daemon: &Daemon,
    ) -> Vec<String> {
        let rendered = render(theme, width, lane, method, daemon);
        let plain = strip_ansi_escapes::strip_str(&rendered);
        plain.trim_end().lines().map(str::to_owned).collect()
    }

    #[test]
    fn a_running_lane_gets_its_state_and_the_stop_command() {
        let daemon = running(vec![lane("acme@0", "running"), lane("acme@1", "idle")]);
        let rows = text(
            &theme(Tier::Ansi, Glyphs::Unicode),
            42,
            Some("acme@0"),
            None,
            &daemon,
        );
        assert_eq!(
            rows,
            [
                "",
                "  ▀▀▀▀▀   ░█▀▄░█▀▀░█░█░█░░░█▀█░█▀█░█▀▀░█▀▀",
                "   ▀▀▀▀▀  ░█░█░█▀▀░▀▄▀░█░░░█▀█░█░█░█▀▀░▀▀█",
                "    ▀▀▀▀▀ ░▀▀░░▀▀▀░░▀░░▀▀▀░▀░▀░▀░▀░▀▀▀░▀▀▀",
                "",
                "  Lane      acme@0  running",
                "  Daemon    running, 1 lane running",
                "  Stop      devlanes stop --lane acme@0",
            ]
        );
    }

    #[test]
    fn a_newer_release_comes_after_the_daemon_with_its_command() {
        let daemon = Daemon::Running {
            lanes: vec![lane("acme@0", "running")],
            update: Some(release("99.0.1", "patch")),
            stopping_in: None,
        };
        let rows = text(
            &theme(Tier::Plain, Glyphs::Ascii),
            80,
            Some("acme@0"),
            Some(&InstallMethod::Homebrew),
            &daemon,
        );
        assert_eq!(
            rows[3..],
            [
                "  Lane      acme@0  running",
                "  Daemon    running, 1 lane running",
                "  Update    99.0.1 available · devlanes update",
                "  Stop      devlanes stop --lane acme@0",
            ]
        );
        // Installed in the background: the next `devlanes` switches to it.
        let mut ready = release("99.0.1", "patch");
        ready.state = "ready".into();
        ready.installed = Some("99.0.1".into());
        let daemon = Daemon::Running {
            lanes: Vec::new(),
            update: Some(ready),
            stopping_in: None,
        };
        let rows = text(
            &theme(Tier::Plain, Glyphs::Ascii),
            80,
            None,
            Some(&InstallMethod::Homebrew),
            &daemon,
        );
        assert_eq!(
            rows[3..],
            [
                "  Daemon    running, no lanes running",
                "  Update    99.0.1 ready · run devlanes to switch to it",
            ]
        );
        // A minor release says it may break things; without the install
        // method the command is left out.
        let daemon = Daemon::Running {
            lanes: Vec::new(),
            update: Some(release("99.0.0", "minor")),
            stopping_in: None,
        };
        let rows = text(&theme(Tier::Plain, Glyphs::Ascii), 80, None, None, &daemon);
        assert_eq!(
            rows[3..],
            [
                "  Daemon    running, no lanes running",
                "  Update    99.0.0 available, breaking changes",
            ]
        );
        // A release this binary already is, or is past, is no news.
        let daemon = Daemon::Running {
            lanes: Vec::new(),
            update: Some(release("0.0.1", "patch")),
            stopping_in: None,
        };
        let rows = text(&theme(Tier::Plain, Glyphs::Ascii), 80, None, None, &daemon);
        assert_eq!(rows[3..], ["  Daemon    running, no lanes running"]);
    }

    #[test]
    fn a_stopped_daemon_leaves_the_lane_without_state() {
        let rows = text(
            &theme(Tier::Plain, Glyphs::Ascii),
            80,
            Some("acme@0"),
            None,
            &Daemon::Stopped,
        );
        assert_eq!(
            rows,
            [
                "",
                "  devlanes",
                "",
                "  Lane      acme@0",
                "  Daemon    not running",
            ]
        );
    }

    #[test]
    fn an_idle_lane_has_no_stop_command() {
        let daemon = running(vec![lane("acme@0", "idle")]);
        let rows = text(
            &theme(Tier::Ansi, Glyphs::Ascii),
            80,
            Some("acme@0"),
            None,
            &daemon,
        );
        assert_eq!(
            rows[3..],
            [
                "  Lane      acme@0  idle",
                "  Daemon    running, no lanes running"
            ]
        );
        let rows = text(&theme(Tier::Ansi, Glyphs::Ascii), 80, None, None, &daemon);
        assert_eq!(rows[3..], ["  Daemon    running, no lanes running"]);
    }

    #[test]
    fn colors_end_with_a_reset() {
        let rendered = render(
            &theme(Tier::Ansi, Glyphs::Unicode),
            80,
            Some("acme@0"),
            None,
            &Daemon::Unanswered,
        );
        assert!(rendered.contains("not answering"), "{rendered:?}");
        for line in rendered.lines().filter(|line| line.contains('\x1b')) {
            assert!(line.ends_with("\x1b[0m"), "{line:?}");
        }
    }
}
