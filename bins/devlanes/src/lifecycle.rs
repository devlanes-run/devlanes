//! `daemon stop` and `daemon restart`: the steps [`devlanes_client::restart`]
//! reports, drawn as the TUI's step block on a terminal and as one line per
//! finished step on plain output. Agents and JSON get only the result.
//! `devlanes update` draws its install and restart in the same block.

use std::{
    collections::BTreeSet,
    io::{IsTerminal, Stdout, Write},
    time::{Duration, Instant},
};

use crossterm::{
    cursor::MoveToPreviousLine,
    terminal::{Clear, ClearType},
};
use devlanes_cli::{OutputMode, Rendered, Runtime};
use devlanes_client::{
    Client, Paths,
    restart::{self, Progress, RowState, Steps},
};
use devlanes_proto::{Context, Method, ResumeLane, RpcError};
use devlanes_tui::{
    steps::{self, Live, LogoState},
    theme::{Theme, Tone},
};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{driver::rpc_error, output::emit};

/// How often a running step's spinner and time move.
const FRAME: Duration = Duration::from_millis(100);

/// Where the steps go.
enum Sink {
    /// The block, redrawn in place.
    Live(Live<Stdout>),
    /// One line per finished row; `printed` holds the keys already out.
    Plain { printed: BTreeSet<String> },
    /// Nothing until the result (agents, JSON, `--quiet`).
    Quiet,
}

/// How a stop or restart ended, for its title and logo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Running,
    Done,
    Failed,
}

/// What the block shows the steps of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    Stop,
    Restart,
    /// `devlanes update` from `from` to `to`: the install, then the restart.
    Update {
        from: String,
        to: String,
    },
}

/// The rows a process drew, handed to the process it runs next (the new
/// binary after `devlanes update`) through [`STEPS_ENV`], so that one
/// redraws the same block instead of starting another below it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Handoff {
    /// Lines the block took on the terminal, which the next process erases.
    lines: u16,
    rows: Vec<HandoffRow>,
}

/// A finished row of a [`Handoff`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct HandoffRow {
    key: String,
    text: String,
    detail: Option<String>,
    elapsed_ms: u64,
}

/// The environment variable that carries a [`Handoff`].
pub(crate) const STEPS_ENV: &str = devlanes_tui::UPDATE_STEPS_ENV;

/// The steps of one stop, restart or update and how they are shown.
pub(crate) struct View {
    sink: Sink,
    theme: Theme,
    steps: Steps,
    kind: Kind,
    outcome: Outcome,
    spin: u64,
}

impl View {
    /// The view `runtime` asks for, for a `kind` of block, in the TUI's
    /// colors ([`crate::wiring::tui_theme`]).
    pub(crate) fn new(runtime: &Runtime, paths: &Paths, kind: Kind) -> Self {
        let theme = crate::wiring::tui_theme(paths);
        let sink = if runtime.quiet || matches!(runtime.mode, OutputMode::Json | OutputMode::Agent)
        {
            Sink::Quiet
        } else if runtime.mode == OutputMode::Human && runtime.stdout_tty {
            let width = crossterm::terminal::size().map_or(80, |(width, _)| width);
            Sink::Live(Live::new(std::io::stdout(), theme, width))
        } else {
            Sink::Plain {
                printed: BTreeSet::new(),
            }
        };
        Self {
            sink,
            theme,
            steps: Steps::default(),
            kind,
            outcome: Outcome::Running,
            spin: 0,
        }
    }

    /// Whether the steps are drawn as the live block.
    pub(crate) fn live(&self) -> bool {
        matches!(self.sink, Sink::Live(_))
    }

    /// Sets the row `key` and shows it; see [`Steps::set`].
    pub(crate) fn set(&mut self, key: &str, text: String, state: RowState, detail: Option<String>) {
        self.steps.set(key, text, state, detail, Instant::now());
        self.draw();
    }

    /// Runs `work` while the spinner of the running row turns.
    pub(crate) async fn spin<T>(&mut self, work: impl Future<Output = T>) -> T {
        let mut tick = tokio::time::interval(FRAME);
        tokio::pin!(work);
        loop {
            tokio::select! {
                result = &mut work => return result,
                _ = tick.tick(), if self.live() => self.draw(),
            }
        }
    }

    /// The block as drawn so far, for the process this one runs next; `None`
    /// unless the block is live.
    pub(crate) fn handoff(&self) -> Option<Handoff> {
        if !self.live() {
            return None;
        }
        let width = crossterm::terminal::size().map_or(80, |(width, _)| width);
        let now = Instant::now();
        let block = steps::render(
            &self.theme,
            width,
            self.logo(),
            self.title(),
            &self.steps,
            now,
            self.spin,
        );
        Some(Handoff {
            // The blank line above the block counts, as for `Live`.
            lines: u16::try_from(block.matches('\n').count() + 1).unwrap_or(u16::MAX),
            rows: self
                .steps
                .rows()
                .iter()
                .map(|row| HandoffRow {
                    key: row.key.clone(),
                    text: row.text.clone(),
                    detail: row.detail.clone(),
                    elapsed_ms: u64::try_from(row.elapsed.unwrap_or_default().as_millis())
                        .unwrap_or(u64::MAX),
                })
                .collect(),
        })
    }

    /// Takes over the block a previous process drew: erases it and keeps its
    /// rows, so the next draw puts the whole block back in its place.
    pub(crate) fn take_over(&mut self, handoff: Handoff) {
        let now = Instant::now();
        for row in handoff.rows {
            let started = now
                .checked_sub(Duration::from_millis(row.elapsed_ms))
                .unwrap_or(now);
            self.steps
                .set(&row.key, row.text.clone(), RowState::Running, None, started);
            self.steps
                .set(&row.key, row.text, RowState::Done, row.detail, now);
        }
        if self.live() && handoff.lines > 0 {
            let mut stdout = std::io::stdout();
            let _ = crossterm::execute!(
                stdout,
                MoveToPreviousLine(handoff.lines),
                Clear(ClearType::FromCursorDown)
            );
            let _ = stdout.flush();
        }
    }

    /// Folds `progress` in and shows it.
    fn apply(&mut self, progress: &Progress) {
        self.steps.apply(progress, Instant::now());
        self.draw();
    }

    /// Redraws the block, or prints the rows that finished since last time.
    fn draw(&mut self) {
        self.spin = self.spin.wrapping_add(1);
        let (logo, title) = (self.logo(), self.title());
        match &mut self.sink {
            Sink::Live(live) => {
                // A terminal that refuses the write is no reason to stop the restart.
                let _ = live.update(logo, title, &self.steps, Instant::now(), self.spin);
            }
            Sink::Plain { printed } => {
                let mut out = String::new();
                for row in self.steps.rows() {
                    if row.state != RowState::Running && printed.insert(row.key.clone()) {
                        out.push_str(&steps::plain_line(&self.theme, row));
                        out.push('\n');
                    }
                }
                if !out.is_empty() {
                    emit(&Rendered {
                        stdout: out,
                        ..Rendered::default()
                    });
                }
            }
            Sink::Quiet => {}
        }
    }

    /// Draws the block a last time for `outcome` and leaves it on screen.
    pub(crate) fn finish(&mut self, outcome: Outcome) {
        self.outcome = outcome;
        let (logo, title) = (self.logo(), self.title());
        match &mut self.sink {
            Sink::Live(live) => {
                let _ = live.finish(logo, title, &self.steps, Instant::now(), self.spin);
            }
            Sink::Plain { .. } => self.draw(),
            Sink::Quiet => {}
        }
    }

    /// Marks what was running as failed with `error`, and draws it.
    pub(crate) fn fail(&mut self, error: &RpcError) {
        let message = error
            .data
            .get("message")
            .or_else(|| error.data.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(error.code.as_str())
            .to_owned();
        self.steps.fail(&message, Instant::now());
        self.finish(Outcome::Failed);
    }

    /// Grey while it runs; afterwards as the daemon is: running or not.
    fn logo(&self) -> LogoState {
        match (self.outcome, &self.kind) {
            (Outcome::Running, _) => LogoState::Busy,
            (Outcome::Done, Kind::Restart | Kind::Update { .. }) => LogoState::Online,
            (Outcome::Done, Kind::Stop) | (Outcome::Failed, _) => LogoState::Down,
        }
    }

    /// `devlanes is restarting · 0.6.1`, then `devlanes restarted` or
    /// `devlanes did not restart`; an update says `is updating · 0.9.0 → 0.9.1`.
    fn title(&self) -> Line<'static> {
        let (words, failed) = match (self.outcome, &self.kind) {
            (Outcome::Running, Kind::Restart) => ("is restarting", false),
            (Outcome::Running, Kind::Stop) => ("is stopping", false),
            (Outcome::Running, Kind::Update { .. }) => ("is updating", false),
            (Outcome::Done, Kind::Restart) => ("restarted", false),
            (Outcome::Done, Kind::Stop) => ("stopped", false),
            (Outcome::Done, Kind::Update { .. }) => ("updated", false),
            (Outcome::Failed, Kind::Restart) => ("did not restart", true),
            (Outcome::Failed, Kind::Stop) => ("did not stop", true),
            (Outcome::Failed, Kind::Update { .. }) => ("did not update", true),
        };
        let words = if failed {
            Span::styled(format!(" {words}"), self.theme.tone(Tone::Failure))
        } else {
            Span::raw(format!(" {words}"))
        };
        let version = match &self.kind {
            Kind::Update { from, to } if self.outcome == Outcome::Running => {
                format!(" · {from} → {to}")
            }
            Kind::Update { to, .. } => format!(" · {to}"),
            Kind::Stop | Kind::Restart => format!(" · {}", devlanes_version::VERSION),
        };
        Line::from(vec![
            Span::styled("devlanes", Style::new().add_modifier(Modifier::BOLD)),
            words,
            Span::styled(version, self.theme.quiet()),
        ])
    }

    /// Runs `work` while the spinner turns, feeding it what it reports.
    async fn drive<T>(
        &mut self,
        work: impl Future<Output = devlanes_client::Result<T>>,
        mut reports: tokio::sync::mpsc::UnboundedReceiver<Progress>,
    ) -> devlanes_client::Result<T> {
        let mut tick = tokio::time::interval(FRAME);
        tokio::pin!(work);
        let result = loop {
            tokio::select! {
                result = &mut work => break result,
                Some(progress) = reports.recv() => self.apply(&progress),
                _ = tick.tick(), if self.live() => self.draw(),
            }
        };
        while let Ok(progress) = reports.try_recv() {
            self.apply(&progress);
        }
        result
    }
}

/// A progress callback that queues reports for [`View::drive`].
fn reporter() -> (
    impl FnMut(Progress) + Send,
    tokio::sync::mpsc::UnboundedReceiver<Progress>,
) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    (
        move |progress| {
            let _ = sender.send(progress);
        },
        receiver,
    )
}

/// Asks before stopping `processes` processes, unless `--yes` said so. Without
/// a terminal to ask on, refuses with the count, so an agent sees what it
/// would stop. A "y" on a terminal erases the question, so the step block
/// takes its place; an "n" leaves it above the shell prompt.
fn confirmed(
    runtime: &Runtime,
    context: &Context,
    question: &str,
    processes: usize,
) -> Result<bool, RpcError> {
    if processes == 0 || context.yes {
        return Ok(true);
    }
    if !runtime.can_prompt() {
        return Err(RpcError::new(
            "confirmation_required",
            json!({"flag": "--yes", "processes": processes}),
            2,
        ));
    }
    let yes = crate::commands::confirm(runtime, question)?;
    let mut stderr = std::io::stderr();
    if yes && stderr.is_terminal() {
        // The answer's newline left the cursor on the line below the question.
        let _ = crossterm::execute!(stderr, MoveToPreviousLine(1), Clear(ClearType::CurrentLine));
    }
    Ok(yes)
}

/// How [`stop_daemon`] ended.
#[derive(Debug)]
pub(crate) enum Stopped {
    /// No daemon ran.
    NotRunning,
    /// The person said no; nothing stopped.
    Declined,
    /// The daemon stopped. `answer` is its report, `None` when it went away
    /// before answering; `live` tells whether the step block showed it.
    Done { answer: Option<Value>, live: bool },
}

/// Stops the daemon with its steps, asking first when processes run:
/// `<question> 3 processes?` (`--yes` in `context` skips it; without a
/// terminal it fails with `confirmation_required`). Renders no result.
pub(crate) async fn stop_daemon(
    runtime: &Runtime,
    paths: &Paths,
    context: &Context,
    question: &str,
) -> Result<Stopped, RpcError> {
    let processes = match Client::connect_any(paths).await {
        Ok(client) => client.peer.managed_processes,
        Err(error) if error.is_not_running() => return Ok(Stopped::NotRunning),
        Err(error) => return Err(rpc_error(error)),
    };
    let question = format!("{question} {}?", processes_word(processes));
    if !confirmed(runtime, context, &question, processes)? {
        return Ok(Stopped::Declined);
    }
    let mut view = View::new(runtime, paths, Kind::Stop);
    let (mut report, reports) = reporter();
    let result = view
        .drive(restart::stop(paths, &mut report), reports)
        .await
        .map_err(rpc_error);
    let answer = match result {
        Ok(answer) => answer,
        Err(error) => {
            view.fail(&error);
            return Err(error);
        }
    };
    view.finish(Outcome::Done);
    Ok(Stopped::Done {
        // Null: the daemon went away before it answered, so there is no report.
        answer: answer.filter(|answer| !answer.is_null()),
        live: view.live(),
    })
}

/// `daemon stop` with its steps, in every output mode; the report comes once
/// everything stopped.
pub async fn stop(runtime: &Runtime, paths: &Paths, context: Context) -> Result<u8, RpcError> {
    let (answer, live) = match stop_daemon(runtime, paths, &context, "stop the daemon and").await? {
        Stopped::NotRunning => {
            if !runtime.quiet {
                emit(&Rendered {
                    stderr: "daemon is not running\n".into(),
                    ..Rendered::default()
                });
            }
            return Ok(0);
        }
        Stopped::Declined => return Ok(devlanes_cli::exit::USAGE),
        Stopped::Done { answer, live } => (answer, live),
    };
    let Some(answer) = answer else {
        return Ok(0);
    };
    // The block says what stopped; a failed `lane.stop` task needs its line too.
    let failed_task = answer
        .get("tasks")
        .and_then(Value::as_array)
        .is_some_and(|tasks| tasks.iter().any(|task| task["exit_code"] != json!(0)));
    if !live || failed_task {
        return crate::commands::render_result(runtime, Method::DaemonShutdown, &answer, None);
    }
    Ok(0)
}

/// `daemon restart`: asks first when processes run, restarts with its steps
/// and returns once the new daemon answers, naming the lanes that start again.
pub async fn restart(runtime: &Runtime, paths: &Paths, context: Context) -> Result<u8, RpcError> {
    // The new daemon refuses an invalid user config, after the old one stopped
    // every lane, so it fails here first.
    crate::user_config::load(paths).await?;
    match Client::connect_any(paths).await {
        Ok(client) => {
            let processes = client.peer.managed_processes;
            let question = format!("restart the daemon and stop {}?", processes_word(processes));
            if !confirmed(runtime, &context, &question, processes)? {
                return Ok(devlanes_cli::exit::USAGE);
            }
        }
        Err(error) if error.is_not_running() => {}
        Err(error) => return Err(rpc_error(error)),
    }
    let view = View::new(runtime, paths, Kind::Restart);
    restart_in(runtime, paths, context, view).await
}

/// Restarts the daemon with its steps in `view` and returns once the new
/// daemon answers, naming the lanes that start again: the rest of `daemon
/// restart`, and of `devlanes update`. It neither asks nor checks the user
/// config; callers do both first, since the new daemon refuses an invalid
/// config after the old one stopped every lane.
pub(crate) async fn restart_in(
    runtime: &Runtime,
    paths: &Paths,
    context: Context,
    mut view: View,
) -> Result<u8, RpcError> {
    let (mut report, reports) = reporter();
    let env = context.env.clone().unwrap_or_default();
    let work = restart::restart(paths, env, context.actor.clone(), &mut report);
    let restarted = match view.drive(work, reports).await.map_err(rpc_error) {
        Ok(restarted) => restarted,
        Err(error) => {
            view.fail(&error);
            return Err(error);
        }
    };
    let names: Vec<&str> = restarted
        .resuming
        .iter()
        .map(|lane| lane.lane.as_str())
        .collect();
    let mut status = restarted.status.clone();
    if status.is_object() {
        status["running"] = json!(true);
        status["resuming"] = json!(names);
    }
    if !view.live() {
        view.finish(Outcome::Done);
        let code = crate::commands::render_result(runtime, Method::DaemonStatus, &status, None)?;
        if !names.is_empty() && !runtime.quiet && runtime.mode != OutputMode::Json {
            emit(&Rendered {
                stdout: format!(
                    "starting again: {}\nnext: devlanes status\n",
                    names.join(", ")
                ),
                ..Rendered::default()
            });
        }
        return Ok(code);
    }
    if !restarted.resuming.is_empty() {
        view.steps.set(
            "resuming",
            resuming_text(&restarted.resuming),
            RowState::Done,
            Some("devlanes status".to_owned()),
            Instant::now(),
        );
    }
    view.finish(Outcome::Done);
    Ok(0)
}

/// `starting again: acme@1 (10 services), web@2 (1 service)`.
fn resuming_text(resuming: &[ResumeLane]) -> String {
    let lanes: Vec<String> = resuming
        .iter()
        .map(|lane| {
            let services = match lane.units.len() {
                1 => "1 service".to_owned(),
                n => format!("{n} services"),
            };
            format!("{} ({services})", lane.lane)
        })
        .collect();
    format!("starting again: {}", lanes.join(", "))
}

/// `1 process`, `3 processes`.
pub(crate) fn processes_word(n: usize) -> String {
    if n == 1 {
        "1 process".to_owned()
    } else {
        format!("{n} processes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stop, restart or update draws its block in the TUI's colors without
    /// claiming "What's new": the next TUI still shows it.
    #[test]
    fn the_step_block_leaves_whats_new_to_the_next_tui() {
        use devlanes_cli::clap::Parser;
        let home = tempfile::tempdir().expect("home");
        let paths = Paths::new(home.path().join("home")).expect("paths");
        paths.prepare().expect("directories");
        let log = devlanes_update::NoticeLog {
            seen_version: Some("0.0.1".into()),
            ..devlanes_update::NoticeLog::default()
        };
        log.save(&paths.home).expect("notices");
        let before = std::fs::read(paths.home.join("notices.json")).expect("read");
        let cli = devlanes_cli::Cli::parse_from(["devlanes", "--output", "plain"]);
        let runtime = Runtime::resolve(&cli, devlanes_cli::Signals::default()).expect("runtime");
        for kind in [
            Kind::Stop,
            Kind::Restart,
            Kind::Update {
                from: "0.0.1".into(),
                to: devlanes_version::VERSION.into(),
            },
        ] {
            drop(View::new(&runtime, &paths, kind));
        }
        // Claiming takes the update lock and records the version.
        assert!(!paths.home.join("update").exists(), "nothing claimed");
        assert_eq!(
            std::fs::read(paths.home.join("notices.json")).expect("read"),
            before
        );
        let shown = devlanes_update::claim_whats_new(
            &paths.home,
            devlanes_version::VERSION,
            false,
            &crate::notes::embedded(),
        );
        assert!(shown.is_some(), "the next TUI shows what's new");
    }

    #[test]
    fn resuming_names_each_lane_with_its_services() {
        let lane = |name: &str, units: &[&str]| ResumeLane {
            lane: name.to_owned(),
            units: units.iter().map(|unit| (*unit).to_owned()).collect(),
            profiles: Vec::new(),
        };
        assert_eq!(
            resuming_text(&[lane("acme@1", &["api", "web"]), lane("acme@2", &["api"])]),
            "starting again: acme@1 (2 services), acme@2 (1 service)"
        );
    }
}
