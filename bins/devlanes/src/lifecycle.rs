//! `daemon stop` and `daemon restart`: the steps [`devlanes_client::restart`]
//! reports, drawn as the TUI's step block on a terminal and as one line per
//! finished step on plain output. Agents and JSON get only the result.

use std::{
    collections::BTreeSet,
    io::{IsTerminal, Stdout},
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
enum Outcome {
    Running,
    Done,
    Failed,
}

/// The steps of one stop or restart and how they are shown.
struct View {
    sink: Sink,
    theme: Theme,
    steps: Steps,
    /// A restart; otherwise a stop.
    restart: bool,
    outcome: Outcome,
    spin: u64,
}

impl View {
    /// The view `runtime` asks for, for a restart or a stop.
    async fn new(runtime: &Runtime, paths: &Paths, restart: bool) -> Self {
        let theme = crate::wiring::tui_options(paths).await.theme();
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
            restart,
            outcome: Outcome::Running,
            spin: 0,
        }
    }

    /// Whether the steps are drawn as the live block.
    fn live(&self) -> bool {
        matches!(self.sink, Sink::Live(_))
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
    fn finish(&mut self, outcome: Outcome) {
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
    fn fail(&mut self, error: &RpcError) {
        let message = error
            .data
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(error.code.as_str())
            .to_owned();
        self.steps.fail(&message, Instant::now());
        self.finish(Outcome::Failed);
    }

    /// Grey while it runs; afterwards as the daemon is: running or not.
    fn logo(&self) -> LogoState {
        match (self.outcome, self.restart) {
            (Outcome::Running, _) => LogoState::Busy,
            (Outcome::Done, true) => LogoState::Online,
            (Outcome::Done, false) | (Outcome::Failed, _) => LogoState::Down,
        }
    }

    /// `devlanes is restarting · 0.6.1`, then `devlanes restarted` or
    /// `devlanes did not restart`.
    fn title(&self) -> Line<'static> {
        let (words, failed) = match (self.outcome, self.restart) {
            (Outcome::Running, true) => ("is restarting", false),
            (Outcome::Running, false) => ("is stopping", false),
            (Outcome::Done, true) => ("restarted", false),
            (Outcome::Done, false) => ("stopped", false),
            (Outcome::Failed, true) => ("did not restart", true),
            (Outcome::Failed, false) => ("did not stop", true),
        };
        let words = if failed {
            Span::styled(format!(" {words}"), self.theme.tone(Tone::Failure))
        } else {
            Span::raw(format!(" {words}"))
        };
        Line::from(vec![
            Span::styled("devlanes", Style::new().add_modifier(Modifier::BOLD)),
            words,
            Span::styled(
                format!(" · {}", devlanes_version::VERSION),
                self.theme.quiet(),
            ),
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

/// `daemon stop` with its steps, in every output mode; the report comes once
/// everything stopped.
pub async fn stop(runtime: &Runtime, paths: &Paths, context: Context) -> Result<u8, RpcError> {
    let processes = match Client::connect_any(paths).await {
        Ok(client) => client.peer.managed_processes,
        Err(error) if error.is_not_running() => {
            if !runtime.quiet {
                emit(&Rendered {
                    stderr: "daemon is not running\n".into(),
                    ..Rendered::default()
                });
            }
            return Ok(0);
        }
        Err(error) => return Err(rpc_error(error)),
    };
    let question = format!("stop the daemon and {}?", processes_word(processes));
    if !confirmed(runtime, &context, &question, processes)? {
        return Ok(devlanes_cli::exit::USAGE);
    }
    let mut view = View::new(runtime, paths, false).await;
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
    // Null: the daemon went away before it answered, so there is no report.
    let Some(answer) = answer.filter(|answer| !answer.is_null()) else {
        return Ok(0);
    };
    // The block says what stopped; a failed `lane.stop` task needs its line too.
    let failed_task = answer
        .get("tasks")
        .and_then(Value::as_array)
        .is_some_and(|tasks| tasks.iter().any(|task| task["exit_code"] != json!(0)));
    if !view.live() || failed_task {
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
    let mut view = View::new(runtime, paths, true).await;
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
fn processes_word(n: usize) -> String {
    if n == 1 {
        "1 process".to_owned()
    } else {
        format!("{n} processes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
