//! The screen in place of the body while the daemon is not online: the step
//! block of `daemon stop` and `daemon restart` (logo, what is happening, one
//! row per step) and, below it, the live `daemon.log` with the daemon page's
//! follow, pause and filters. Every view draws it; none is switched to.

use devlanes_client::restart::Steps;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::app::{App, DaemonState};
use crate::exit_summary::INDENT;

/// Draws the step block at the top of `area` and the log below it.
pub fn render(app: &App, frame: &mut Frame, area: Rect) {
    let none = Steps::default();
    let steps = app.daemon_state.steps().unwrap_or(&none);
    // One blank row above, indented like the CLI prints the same block.
    let block = Rect::new(
        area.x + INDENT,
        area.y + 1,
        area.width.saturating_sub(2 * INDENT),
        crate::steps::height(steps),
    )
    .intersection(area);
    crate::steps::draw(
        frame.buffer_mut(),
        block,
        &app.theme,
        app.daemon_state.logo(),
        title(app),
        steps,
        app.now(),
        app.spin,
    );
    // The daemon page's checks box is not on screen; the wheel and presses
    // must not find it where it was.
    app.daemon_checks_box.set(Rect::default());
    let top = block.bottom() + 1;
    let log = Rect::new(area.x, top, area.width, area.bottom().saturating_sub(top));
    if log.height >= 3 {
        super::daemon::render_log(app, frame, log);
    }
}

/// What is happening, in bold, and what the user can do about it, quiet.
fn title(app: &App) -> Line<'static> {
    let (title, hint) = match &app.daemon_state {
        DaemonState::Stopping { restart: true, .. }
        | DaemonState::Starting { restart: true, .. } => ("devlanes is restarting", None),
        DaemonState::Stopping { .. } => ("devlanes is stopping", None),
        DaemonState::Starting { .. } => ("starting the daemon", None),
        DaemonState::Stopped { .. } => ("the daemon is stopped", Some("s starts it")),
        DaemonState::Unreachable | DaemonState::Online => (
            "the daemon is not answering",
            Some("retrying on its own, s starts it"),
        ),
    };
    let mut spans = vec![Span::styled(
        title,
        Style::new().add_modifier(Modifier::BOLD),
    )];
    if let Some(hint) = hint {
        spans.push(Span::styled(format!(" · {hint}"), app.theme.quiet()));
    }
    Line::from(spans)
}
