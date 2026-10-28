//! `daemon stop` and `daemon restart`: the steps [`devlanes_client::restart`]
//! reports, drawn as the TUI's step block on a terminal and as one line per
//! finished step on plain output. Agents and JSON get only the result.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Stdout,
    time::{Duration, Instant},
};

use devlanes_cli::{OutputMode, Rendered, Runtime};
use devlanes_client::{
    Client, Frame, Paths,
    restart::{self, Progress, RowState, Steps},
};
use devlanes_proto::{Context, Control, LaneSummary, Method, ResumeLane, RpcError, events};
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
/// Longest a resumed lane may show no start before it counts as not started.
const RESUME_SILENCE: Duration = Duration::from_secs(30);

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
/// would stop.
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
    crate::commands::confirm(runtime, question)
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

/// `daemon restart`: asks first when processes run, restarts with its steps,
/// then on a terminal follows the lanes coming back until they are up.
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
    follow(&mut view, paths, &restarted.resuming).await;
    view.finish(Outcome::Done);
    Ok(0)
}

/// Shows each lane in `resuming` coming back as a row of the block, until all
/// are up, failed or did not start. Ctrl-C ends the wait; the starts go on.
/// The restart itself is done either way, so a lane that fails does not fail it.
async fn follow(view: &mut View, paths: &Paths, resuming: &[ResumeLane]) {
    let mut lanes: BTreeMap<String, Resumed> = resuming
        .iter()
        .map(|lane| (lane.lane.clone(), Resumed::new(lane)))
        .collect();
    for lane in lanes.values() {
        lane.show(view);
    }
    watch(view, paths, &mut lanes).await;
    // However the wait ended, no row is left spinning on the final block.
    for lane in lanes.values_mut().filter(|lane| lane.pending()) {
        lane.detached = true;
        lane.show(view);
    }
}

/// Updates the rows of `lanes` from the new daemon's events until none is
/// pending, Ctrl-C, or the daemon stops answering.
async fn watch(view: &mut View, paths: &Paths, lanes: &mut BTreeMap<String, Resumed>) {
    let Ok(mut events) = Client::connect(paths).await else {
        return;
    };
    if events
        .begin_request(Method::Subscribe, json!({}), Context::default())
        .await
        .is_err()
    {
        return;
    }
    // The starts may be under way before the subscription: read where they are.
    if let Ok(mut client) = Client::connect(paths).await
        && let Ok(status) = client
            .request(Method::Status, json!({"all": true}), Context::default())
            .await
        && let Ok(page) = serde_json::from_value::<devlanes_proto::Page<LaneSummary>>(status)
    {
        for summary in &page.items {
            if let Some(lane) = lanes.get_mut(&summary.reference) {
                lane.update(summary);
                lane.show(view);
            }
        }
    }
    let mut tick = tokio::time::interval(FRAME);
    // Created once: on Unix a new listener per pass would let a Ctrl-C between
    // two passes go unnoticed.
    let interrupted = tokio::signal::ctrl_c();
    tokio::pin!(interrupted);
    while lanes.values().any(Resumed::pending) {
        tokio::select! {
            frame = events.next_frame() => {
                let event = match frame {
                    Ok(Frame::Control(Control::Event(event))) => event,
                    Ok(_) => continue,
                    Err(_) => return,
                };
                if event.event != events::LANE_CHANGED {
                    continue;
                }
                let Some(summary) = event
                    .data
                    .get("lane")
                    .cloned()
                    .and_then(|lane| serde_json::from_value::<LaneSummary>(lane).ok())
                else {
                    continue;
                };
                if let Some(lane) = lanes.get_mut(&summary.reference) {
                    lane.update(&summary);
                    lane.show(view);
                }
            }
            _ = tick.tick() => {
                for lane in lanes.values().filter(|lane| lane.silent()) {
                    lane.show(view);
                }
                view.draw();
            }
            _ = &mut interrupted => return,
        }
    }
}

/// One lane a restart starts again, as the follow sees it.
struct Resumed {
    reference: String,
    /// The services it starts again.
    units: BTreeSet<String>,
    up: usize,
    failed: usize,
    /// It reported a start, so a lane back to idle has finished starting.
    started: bool,
    finished: bool,
    /// The wait ended while it still started.
    detached: bool,
    since: Instant,
}

impl Resumed {
    /// A lane that has not reported anything yet.
    fn new(lane: &ResumeLane) -> Self {
        Self {
            reference: lane.lane.clone(),
            units: lane.units.iter().cloned().collect(),
            up: 0,
            failed: 0,
            started: false,
            finished: false,
            detached: false,
            since: Instant::now(),
        }
    }

    /// Reads where its services are from `summary`.
    fn update(&mut self, summary: &LaneSummary) {
        let mine = summary
            .units
            .iter()
            .filter(|unit| self.units.contains(&unit.name));
        let (mut up, mut failed, mut busy) = (0, 0, 0);
        for unit in mine {
            match unit.state.as_str() {
                "ready" | "running" => up += 1,
                "failed" | "exited" => failed += 1,
                "starting" | "queued" => busy += 1,
                _ => {}
            }
        }
        self.up = up;
        self.failed = failed;
        // A service that already failed also shows the start happened.
        self.started |= busy > 0 || up > 0 || failed > 0 || summary.state == "starting";
        self.finished = self.started && busy == 0 && summary.state != "starting";
    }

    /// Still worth waiting for.
    fn pending(&self) -> bool {
        !self.finished && !self.detached && !self.silent()
    }

    /// Nothing started within [`RESUME_SILENCE`].
    fn silent(&self) -> bool {
        !self.started && self.since.elapsed() >= RESUME_SILENCE
    }

    /// Its row in `view`.
    fn show(&self, view: &mut View) {
        let total = self.units.len();
        let ready = format!("{} of {total} ready", self.up);
        let (text, state, detail) = if self.detached {
            (
                format!("starting {}", self.reference),
                RowState::Done,
                Some(format!("{ready}, still starting; see devlanes status")),
            )
        } else if self.silent() {
            // A lane the new daemon could not start is only in its log.
            (
                format!("{} did not start", self.reference),
                RowState::Failed,
                Some("see devlanes daemon logs".to_owned()),
            )
        } else if !self.finished {
            (
                format!("starting {}", self.reference),
                RowState::Running,
                Some(ready),
            )
        } else if self.failed > 0 || self.up < total {
            (
                format!("started {}", self.reference),
                RowState::Failed,
                Some(format!("{ready}, {} failed", self.failed)),
            )
        } else {
            (
                format!("started {}", self.reference),
                RowState::Done,
                Some(format!("{total} ready")),
            )
        };
        view.steps
            .set(&self.reference, text, state, detail, Instant::now());
        view.draw();
    }
}

/// `1 process`, `3 processes`.
fn processes_word(n: usize) -> String {
    if n == 1 {
        "1 process".to_owned()
    } else {
        format!("{n} processes")
    }
}
