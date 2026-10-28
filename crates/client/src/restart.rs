//! `daemon stop` and `daemon restart`, for the CLI and the TUI alike.
//!
//! A restart starts the new daemon before it stops the old one: the new one
//! waits for the singleton lock, then starts the lanes that ran from the
//! [`ResumePlan`] this client wrote on its stdin. Once the shutdown is asked,
//! nothing depends on this client any more, so quitting the TUI or pressing
//! Ctrl-C partway still ends with a running daemon. The restart is over when
//! the new daemon answers; the lanes come back as ordinary starts.
//!
//! Both report [`Progress`]; [`Steps`] folds it into the rows clients draw.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use devlanes_proto::{
    Actor, Context, Control, DaemonStopping, LaneSummary, Method, Outcome, Page, REPLACE_STEP,
    ResumeLane, ResumePlan, Selection, ShutdownParams, StoppingState, StoppingStep, events,
};
use serde_json::{Value, json};

use crate::{Client, Error, Frame, Paths, Result};

/// Unit states that count as running for a restart.
const RUNNING: [&str; 4] = ["starting", "queued", "ready", "running"];

/// The lanes with running services, from `status --all`. Tasks, exec units and a
/// previous daemon's orphans are left out: they cannot be started again as they were.
pub fn running_lanes(lanes: &[LaneSummary]) -> Vec<ResumeLane> {
    lanes
        .iter()
        .filter_map(|lane| {
            let units: Vec<String> = lane
                .units
                .iter()
                .filter(|unit| unit.kind == "service" && RUNNING.contains(&unit.state.as_str()))
                .map(|unit| unit.name.clone())
                .collect();
            (!units.is_empty()).then(|| ResumeLane {
                lane: lane.reference.clone(),
                units,
                profiles: lane.profiles.clone(),
            })
        })
        .collect()
}

/// What [`restart`] ended with.
#[derive(Debug, Clone)]
pub struct Restarted {
    /// The new daemon's `daemon.status`.
    pub status: serde_json::Value,
    /// The lanes it starts again, as ordinary starts already under way.
    pub resuming: Vec<ResumeLane>,
}

/// Restarts the daemon of `paths`: starts the new daemon, which waits for the
/// old one, asks the old one to stop (with `yes`; confirm first) and returns
/// once the new one answers. The lanes that ran start again on the new daemon
/// with `env` and `actor`. Check the user config before calling: a new daemon
/// refuses an invalid one, after the old one stopped everything.
pub async fn restart(
    paths: &Paths,
    env: BTreeMap<String, String>,
    actor: Actor,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Restarted> {
    // Any protocol: restarting is how an incompatible daemon gets replaced.
    let mut old = match Client::connect_any(paths).await {
        Ok(client) => client,
        Err(error) if error.is_not_running() => return start(paths, progress).await,
        Err(error) => return Err(error),
    };
    let lanes = if old.compatible() {
        old.lanes_that_run().await.unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read what runs; nothing starts again");
            Vec::new()
        })
    } else {
        Vec::new()
    };
    let pid = old
        .request(Method::DaemonStatus, json!({}), Context::default())
        .await
        .ok()
        .and_then(|status| status.get("pid").and_then(Value::as_u64))
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or(0);
    // Before anything stops, so no step is missed.
    let mut events = subscribe(&old, paths).await;
    let plan = ResumePlan {
        lanes: lanes.clone(),
        env,
        actor,
    };
    let line = serde_json::to_vec(&plan)
        .map_err(|error| Error::Startup(format!("unwritable restart plan: {error}")))?;
    let owned = paths.clone();
    tokio::task::spawn_blocking(move || owned.prepare()).await??;
    let begun = tokio::time::Instant::now();
    let replace = crate::Replace { pid, plan: line };
    let (mut lines, _, handover) = crate::start_daemon(paths, &[], begun, Some(replace)).await?;
    let handover =
        handover.ok_or_else(|| Error::Startup("the new daemon got no restart plan".into()))?;
    // The new daemon reports this step once other clients wait for it. Only
    // then is the old one asked to stop, so a new daemon that cannot take
    // over never costs the running lanes: on any other answer `handover` is
    // dropped, the new daemon gives up, and the old one keeps running.
    let mut step = loop {
        let line = next_line(&mut lines, Some(STARTUP_STEP_TIMEOUT), paths).await?;
        match crate::startup_step(&line) {
            Some(step) if step == REPLACE_STEP => break step,
            Some(_) => {}
            None => return Err(crate::startup_failure(paths, &line, None)),
        }
    };
    handover.go().await?;
    progress(Progress::Stopping {
        processes: old.peer.managed_processes as u64,
    });
    let params = serde_json::to_value(ShutdownParams { restart: true }).unwrap_or_default();
    let id = old
        .begin_request(Method::DaemonShutdown, params, yes())
        .await?;
    // While the old daemon stops, the new one only waits; its lines are kept
    // for after, and an answer it gives meanwhile is its last one.
    let mut answer = None;
    loop {
        tokio::select! {
            stopping = next_stopping(&mut events) => match stopping {
                Some(stopping) => progress(Progress::Step(stopping)),
                None => events = None,
            },
            done = shutdown_answer(&mut old, id) => {
                // The old daemon exits after a failed shutdown too, and the new
                // one takes over all the same.
                if let Err(error) = done {
                    tracing::warn!(%error, "the old daemon's shutdown reported an error");
                }
                break;
            }
            line = lines.recv(), if answer.is_none() => {
                // A new daemon that dies now is a failure for after the old one
                // stopped, when the lanes can still start elsewhere.
                match line.ok_or_else(reader_stopped).and_then(|line| Ok(line?)) {
                    Ok(line) => match crate::startup_step(&line) {
                        Some(next) => step = next,
                        None => answer = Some(Ok(line)),
                    },
                    Err(error) => answer = Some(Err(error)),
                }
            }
        }
    }
    drain(&mut events, progress).await;
    progress(Progress::Stopped);
    progress(Progress::Starting(step.clone()));
    let answer = match answer {
        Some(answer) => answer,
        None => loop {
            let line = match next_line(&mut lines, Some(STARTUP_STEP_TIMEOUT), paths).await {
                Ok(line) => line,
                Err(error) => break Err(error),
            };
            match crate::startup_step(&line) {
                Some(next) => {
                    progress(Progress::Starting(next.clone()));
                    step = next;
                }
                None => break Ok(line),
            }
        },
    };
    let failure = match answer {
        // `already-running`: another daemon took the lock first and was handed the lanes.
        Ok(answer) if answer == b"ready\n" || answer == b"already-running\n" => None,
        Ok(answer) => Some(crate::startup_failure(paths, &answer, Some(step))),
        Err(error) => Some(error),
    };
    if let Some(failure) = failure {
        return take_over_here(paths, &plan, progress, failure).await;
    }
    let mut client = Client::connect_while_starting(paths).await?;
    let status = client
        .request(Method::DaemonStatus, json!({}), Context::default())
        .await?;
    progress(started(&status));
    Ok(Restarted {
        status,
        resuming: lanes,
    })
}

/// The old daemon stopped but the new one did not come up (`failure`): a
/// daemon of this client's own starts the plan's lanes, so they are not
/// lost. When even that daemon cannot start, the new one's failure is the
/// one to report.
async fn take_over_here(
    paths: &Paths,
    plan: &ResumePlan,
    progress: &mut (dyn FnMut(Progress) + Send),
    failure: Error,
) -> Result<Restarted> {
    tracing::warn!(error = %failure, "the new daemon did not take over; starting another");
    // The old daemon answers its shutdown before it exits; a daemon spawned
    // while it still holds the lock would only report `already-running`.
    let deadline = tokio::time::Instant::now() + STARTUP_STEP_TIMEOUT;
    while crate::lock_held(paths).await && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let Ok(mut client) = Client::connect_or_spawn(paths).await else {
        return Err(failure);
    };
    for lane in &plan.lanes {
        let selection = Selection {
            units: lane.units.clone(),
            profiles: lane.profiles.clone(),
            ..Selection::default()
        };
        let context = Context {
            lane: Some(lane.lane.clone()),
            env: Some(plan.env.clone()),
            actor: plan.actor.clone(),
            ..Context::default()
        };
        let params = serde_json::to_value(selection).unwrap_or_default();
        // Detached starts: the daemon goes on when this connection ends.
        client.begin_request(Method::Start, params, context).await?;
    }
    let status = client
        .request(Method::DaemonStatus, json!({}), Context::default())
        .await?;
    progress(started(&status));
    Ok(Restarted {
        status,
        resuming: plan.lanes.clone(),
    })
}

/// Stops the daemon of `paths` (with `yes`; confirm first). `Ok(None)` when
/// none ran; otherwise its shutdown answer, which comes once everything
/// stopped.
pub async fn stop(
    paths: &Paths,
    progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<Option<Value>> {
    // Any protocol: `daemon.shutdown` is frozen, so an old daemon stops too.
    let mut old = match Client::connect_any(paths).await {
        Ok(client) => client,
        Err(error) if error.is_not_running() => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut events = subscribe(&old, paths).await;
    progress(Progress::Stopping {
        processes: old.peer.managed_processes as u64,
    });
    let id = old
        .begin_request(Method::DaemonShutdown, json!({}), yes())
        .await?;
    let answer = loop {
        tokio::select! {
            stopping = next_stopping(&mut events) => match stopping {
                Some(stopping) => progress(Progress::Step(stopping)),
                None => events = None,
            },
            done = shutdown_answer(&mut old, id) => break done?,
        }
    };
    drain(&mut events, progress).await;
    progress(Progress::Stopped);
    Ok(Some(answer))
}

/// A restart with no daemon running is a start.
async fn start(paths: &Paths, progress: &mut (dyn FnMut(Progress) + Send)) -> Result<Restarted> {
    progress(Progress::Starting(String::new()));
    let mut client = Client::connect_or_spawn(paths).await?;
    let status = client
        .request(Method::DaemonStatus, json!({}), Context::default())
        .await?;
    progress(started(&status));
    Ok(Restarted {
        status,
        resuming: Vec::new(),
    })
}

/// [`Progress::Started`] from a `daemon.status` answer.
fn started(status: &Value) -> Progress {
    Progress::Started {
        version: status
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        pid: status.get("pid").and_then(Value::as_u64),
    }
}

/// The context of a shutdown the caller already confirmed.
fn yes() -> Context {
    Context {
        yes: true,
        ..Context::default()
    }
}

/// A subscription to the daemon `old` is connected to, for its
/// [`events::DAEMON_STOPPING`]; none for another protocol, whose daemon may not
/// know the method, or when it cannot be opened (the stop goes on without steps).
async fn subscribe(old: &Client, paths: &Paths) -> Option<Client> {
    if !old.compatible() {
        return None;
    }
    let mut client = Client::connect(paths).await.ok()?;
    client
        .begin_request(Method::Subscribe, json!({}), Context::default())
        .await
        .ok()?;
    // The daemon greets a new subscriber with a `resync` once it is
    // registered; asking for the shutdown before then could miss its first steps.
    let registered = async {
        loop {
            match client.next_frame().await {
                Ok(Frame::Control(Control::Event(_))) => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    };
    match tokio::time::timeout(SUBSCRIBE_WAIT, registered).await {
        Ok(false) => None,
        Ok(true) | Err(_) => Some(client),
    }
}

/// How long [`subscribe`] waits for the daemon to register the subscription.
const SUBSCRIBE_WAIT: Duration = Duration::from_secs(2);

/// The next [`DaemonStopping`] on `events`; `None` once the subscription
/// ended. Never resolves without one, so it can sit in a `select!`.
async fn next_stopping(events: &mut Option<Client>) -> Option<DaemonStopping> {
    let Some(client) = events.as_mut() else {
        return std::future::pending().await;
    };
    loop {
        match client.next_frame().await {
            Ok(Frame::Control(Control::Event(event))) if event.event == events::DAEMON_STOPPING => {
                if let Ok(stopping) = serde_json::from_value(event.data) {
                    return Some(stopping);
                }
            }
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

/// Delivers the steps still on their way after the shutdown answered: the
/// subscription is a separate connection, so its last events may trail the
/// answer. Ends when the daemon closes it, or after a short wait.
async fn drain(events: &mut Option<Client>, progress: &mut (dyn FnMut(Progress) + Send)) {
    let deadline = tokio::time::Instant::now() + DRAIN_WAIT;
    while events.is_some() {
        match tokio::time::timeout_at(deadline, next_stopping(events)).await {
            Ok(Some(stopping)) => progress(Progress::Step(stopping)),
            Ok(None) | Err(_) => *events = None,
        }
    }
}

/// How long [`drain`] waits for the last shutdown steps.
const DRAIN_WAIT: Duration = Duration::from_secs(1);

/// The answer to the `daemon.shutdown` request `id` on `old`. A connection
/// that closes first counts as done: the daemon is gone either way.
async fn shutdown_answer(old: &mut Client, id: u64) -> Result<Value> {
    loop {
        match old.next_frame().await {
            Ok(Frame::Control(Control::Response(response))) if response.id == id => {
                return match response.result {
                    Outcome::Ok { data } => Ok(data),
                    Outcome::Error { error } => Err(Error::Rpc(error)),
                };
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(%error, "the daemon closed the connection before answering its shutdown");
                return Ok(Value::Null);
            }
        }
    }
}

/// The next line of a new daemon's readiness pipe, within `wait` when given.
async fn next_line(
    lines: &mut crate::ReadyLines,
    wait: Option<Duration>,
    paths: &Paths,
) -> Result<Vec<u8>> {
    let line = match wait {
        Some(wait) => tokio::time::timeout(wait, lines.recv())
            .await
            .map_err(|_| Error::StartupStalled {
                step: None,
                log: paths.logs.join("daemon.log"),
            })?,
        None => lines.recv().await,
    };
    Ok(line.ok_or_else(reader_stopped)??)
}

/// The readiness pipe's reader ended before the daemon answered.
fn reader_stopped() -> Error {
    Error::Startup("the readiness reader stopped".into())
}

/// Longest the new daemon may stay in one startup step once the old one is
/// gone, as for any spawned daemon.
const STARTUP_STEP_TIMEOUT: Duration = crate::STARTUP_STEP_TIMEOUT;

impl Client {
    /// [`running_lanes`] of every project this daemon knows.
    async fn lanes_that_run(&mut self) -> Result<Vec<ResumeLane>> {
        let status = self
            .request(Method::Status, json!({"all": true}), Context::default())
            .await?;
        let page: Page<LaneSummary> = serde_json::from_value(status)
            .map_err(|error| Error::Startup(format!("unreadable status: {error}")))?;
        Ok(running_lanes(&page.items))
    }
}

/// What a stop or a restart reports as it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// The old daemon was asked to stop; it manages `processes` processes.
    Stopping { processes: u64 },
    /// A step of the old daemon's shutdown. Daemons from before
    /// [`devlanes_proto::events::DAEMON_STOPPING`] send none.
    Step(DaemonStopping),
    /// The old daemon is gone.
    Stopped,
    /// The new daemon reports a startup step, such as `waiting for the old
    /// daemon` or `state database`.
    Starting(String),
    /// The new daemon answers.
    Started { version: String, pid: Option<u64> },
}

/// How far a [`Row`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Running,
    Done,
    Failed,
}

/// One line of a stop or restart as clients draw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Identifies the row across updates: `stop`, a [`StoppingStep`] name,
    /// `start`, or what a client adds with [`Steps::set`] (a lane reference).
    pub key: String,
    /// What the step does or did: `stopping 26 processes in 1 lane`.
    pub text: String,
    /// What it is at now, shown after the text: `4 left`, `state database`.
    pub detail: Option<String>,
    pub state: RowState,
    /// When the row began, for the time a running row shows.
    pub started: Instant,
    /// How long it took, once done or failed.
    pub elapsed: Option<Duration>,
}

/// The rows of one stop or restart, in the order their steps began.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Steps {
    rows: Vec<Row>,
}

/// Row of the shutdown as a whole, until the daemon reports its own steps.
const STOP: &str = "stop";
/// Row of the new daemon's startup.
const START: &str = "start";

impl Steps {
    /// The rows, oldest first.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The row still running, if any: the newest one.
    pub fn running(&self) -> Option<&Row> {
        self.rows
            .iter()
            .rev()
            .find(|row| row.state == RowState::Running)
    }

    /// Folds one report in, at `now`.
    pub fn apply(&mut self, progress: &Progress, now: Instant) {
        match progress {
            Progress::Stopping { processes } => {
                let text = match processes {
                    0 => "stopping the daemon".to_owned(),
                    n => format!(
                        "stopping the daemon and {}",
                        count(*n, "process", "processes")
                    ),
                };
                self.set(STOP, text, RowState::Running, None, now);
            }
            Progress::Step(step) => self.step(step, now),
            Progress::Stopped => {
                // A daemon without step events had only the generic row.
                if let Some(row) = self.row_mut(STOP)
                    && row.state == RowState::Running
                {
                    row.text = "stopped the daemon".to_owned();
                }
                self.finish_running(now, |key| key != START);
            }
            Progress::Starting(step) => {
                self.finish_running(now, |key| key != START);
                // A daemon this client could not follow reports no step.
                let detail = (!step.is_empty()).then(|| step.clone());
                self.set(
                    START,
                    "starting the new daemon".to_owned(),
                    RowState::Running,
                    detail,
                    now,
                );
            }
            Progress::Started { version, pid } => {
                self.finish_running(now, |key| key != START);
                let detail = pid.map(|pid| format!("pid {pid}"));
                self.set(
                    START,
                    format!("started devlanes {version}"),
                    RowState::Done,
                    detail,
                    now,
                );
            }
        }
    }

    /// Adds the row `key` or updates it. A row that leaves [`RowState::Running`]
    /// gets its elapsed time; one that changes text keeps its start time.
    pub fn set(
        &mut self,
        key: &str,
        text: String,
        state: RowState,
        detail: Option<String>,
        now: Instant,
    ) {
        match self.row_mut(key) {
            Some(row) => {
                if row.state == RowState::Running && state != RowState::Running {
                    row.elapsed = Some(now.saturating_duration_since(row.started));
                }
                row.text = text;
                row.detail = detail;
                row.state = state;
            }
            None => self.rows.push(Row {
                key: key.to_owned(),
                text,
                detail,
                state,
                started: now,
                elapsed: (state != RowState::Running).then_some(Duration::ZERO),
            }),
        }
    }

    /// Marks the running rows failed, the newest with `message` as its detail.
    pub fn fail(&mut self, message: &str, now: Instant) {
        let mut first = true;
        for row in self.rows.iter_mut().rev() {
            if row.state != RowState::Running {
                continue;
            }
            row.state = RowState::Failed;
            row.elapsed = Some(now.saturating_duration_since(row.started));
            if first {
                row.detail = Some(message.to_owned());
                first = false;
            }
        }
    }

    /// One [`DaemonStopping`] report: the step's own row replaces the generic one.
    fn step(&mut self, step: &DaemonStopping, now: Instant) {
        let key = step_key(step.step);
        if step.state == StoppingState::Skipped {
            self.rows.retain(|row| row.key != key);
            return;
        }
        if let Some(index) = self.rows.iter().position(|row| row.key == STOP) {
            // The daemon's own steps say more; the generic row goes once they come.
            if self.rows[index].state == RowState::Running {
                self.rows.remove(index);
            }
        }
        let done = step.state == StoppingState::Done;
        let text = step_text(step, done);
        let detail = match (step.step, step.state, step.left) {
            (StoppingStep::Units, StoppingState::Progress, Some(left)) => {
                Some(format!("{left} left"))
            }
            _ => None,
        };
        let state = if done {
            RowState::Done
        } else {
            RowState::Running
        };
        self.set(key, text, state, detail, now);
        // The daemon measured the step itself; that beats when the event arrived.
        if let (true, Some(ms), Some(row)) = (done, step.elapsed_ms, self.row_mut(key)) {
            row.elapsed = Some(Duration::from_millis(ms));
        }
    }

    /// Ends every running row whose key `matches`, as done.
    fn finish_running(&mut self, now: Instant, matches: impl Fn(&str) -> bool) {
        for row in &mut self.rows {
            if row.state == RowState::Running && matches(&row.key) {
                row.state = RowState::Done;
                row.elapsed = Some(now.saturating_duration_since(row.started));
                row.detail = None;
            }
        }
    }

    /// The row `key`.
    fn row_mut(&mut self, key: &str) -> Option<&mut Row> {
        self.rows.iter_mut().find(|row| row.key == key)
    }
}

/// The row key of a shutdown step.
fn step_key(step: StoppingStep) -> &'static str {
    match step {
        StoppingStep::Remote => "remote",
        StoppingStep::Thaw => "thaw",
        StoppingStep::Tasks => "tasks",
        StoppingStep::Units => "units",
        StoppingStep::State => "state",
        StoppingStep::Other => "other",
    }
}

/// What a shutdown step does, or did once `done`.
fn step_text(step: &DaemonStopping, done: bool) -> String {
    let total = step.total.unwrap_or(0);
    match (step.step, done) {
        (StoppingStep::Remote, false) => format!("disconnecting {}", count(total, "box", "boxes")),
        (StoppingStep::Remote, true) => format!("disconnected {}", count(total, "box", "boxes")),
        (StoppingStep::Thaw, false) => {
            format!("waking {}", count(total, "idle lane", "idle lanes"))
        }
        (StoppingStep::Thaw, true) => format!("woke {}", count(total, "idle lane", "idle lanes")),
        (StoppingStep::Tasks, false) => {
            format!(
                "running {}",
                count(total, "lane.stop task", "lane.stop tasks")
            )
        }
        (StoppingStep::Tasks, true) => {
            format!("ran {}", count(total, "lane.stop task", "lane.stop tasks"))
        }
        (StoppingStep::Units, done) => {
            let verb = if done { "stopped" } else { "stopping" };
            let processes = count(total, "process", "processes");
            match step.lanes {
                Some(lanes) => format!("{verb} {processes} in {}", count(lanes, "lane", "lanes")),
                None => format!("{verb} {processes}"),
            }
        }
        (StoppingStep::State, false) => "saving state".to_owned(),
        (StoppingStep::State, true) => "saved state".to_owned(),
        (StoppingStep::Other, false) => "shutting down".to_owned(),
        (StoppingStep::Other, true) => "shut down".to_owned(),
    }
}

/// `1 process`, `26 processes`.
fn count(n: u64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devlanes_proto::UnitSummary;

    /// A unit summary with `kind` and `state`.
    fn unit(name: &str, kind: &str, state: &str) -> UnitSummary {
        UnitSummary {
            name: name.into(),
            kind: kind.into(),
            state: state.into(),
            ..UnitSummary::default()
        }
    }

    /// A `daemon.stopping` report of an explicit restart.
    fn stopping(step: StoppingStep, state: StoppingState) -> DaemonStopping {
        DaemonStopping {
            reason: "explicit".into(),
            restart: true,
            step,
            state,
            total: None,
            left: None,
            lanes: None,
            elapsed_ms: None,
        }
    }

    #[test]
    /// Only lanes with running services count, with those services and profiles.
    fn running_lanes_keep_services_and_profiles() {
        let lanes = vec![
            LaneSummary {
                reference: "acme@0".into(),
                profiles: vec!["backend".into()],
                units: vec![
                    unit("api", "service", "ready"),
                    unit("web", "service", "idle"),
                    unit("worker", "service", "starting"),
                    unit("migrate", "task", "running"),
                    unit("probe", "exec", "ready"),
                    unit("old", "service", devlanes_proto::ORPHANED),
                ],
                ..LaneSummary::default()
            },
            LaneSummary {
                reference: "acme@1".into(),
                units: vec![unit("api", "service", "stopped")],
                ..LaneSummary::default()
            },
        ];
        assert_eq!(
            running_lanes(&lanes),
            [ResumeLane {
                lane: "acme@0".into(),
                units: vec!["api".into(), "worker".into()],
                profiles: vec!["backend".into()],
            }]
        );
    }

    #[test]
    /// A restart reads as one row per step that ran, each with its time: the
    /// generic stop row gives way to the daemon's steps, a skipped step leaves
    /// no row, and the daemon's own measure wins over arrival times.
    fn a_restart_folds_into_rows() {
        let begun = Instant::now();
        let at = |ms: u64| begun + Duration::from_millis(ms);
        let mut steps = Steps::default();
        steps.apply(&Progress::Stopping { processes: 26 }, at(0));
        assert_eq!(steps.rows()[0].text, "stopping the daemon and 26 processes");
        steps.apply(
            &Progress::Step(stopping(StoppingStep::Remote, StoppingState::Skipped)),
            at(5),
        );
        let mut units = stopping(StoppingStep::Units, StoppingState::Started);
        units.total = Some(26);
        units.lanes = Some(1);
        steps.apply(&Progress::Step(units.clone()), at(10));
        units.state = StoppingState::Progress;
        units.left = Some(4);
        steps.apply(&Progress::Step(units.clone()), at(3000));
        let running = steps.running().expect("units runs");
        assert_eq!(running.text, "stopping 26 processes in 1 lane");
        assert_eq!(running.detail.as_deref(), Some("4 left"));
        units.state = StoppingState::Done;
        units.left = None;
        units.elapsed_ms = Some(5800);
        steps.apply(&Progress::Step(units), at(5900));
        steps.apply(&Progress::Stopped, at(6000));
        steps.apply(
            &Progress::Starting("waiting for the old daemon".into()),
            at(6000),
        );
        steps.apply(&Progress::Starting("state database".into()), at(6050));
        assert_eq!(
            steps.running().and_then(|row| row.detail.as_deref()),
            Some("state database")
        );
        steps.apply(
            &Progress::Started {
                version: "0.6.1".into(),
                pid: Some(4242),
            },
            at(6100),
        );
        let rows: Vec<(&str, RowState, Option<Duration>)> = steps
            .rows()
            .iter()
            .map(|row| (row.text.as_str(), row.state, row.elapsed))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "stopped 26 processes in 1 lane",
                    RowState::Done,
                    Some(Duration::from_millis(5800))
                ),
                (
                    "started devlanes 0.6.1",
                    RowState::Done,
                    Some(Duration::from_millis(100))
                ),
            ]
        );
        assert!(steps.running().is_none());
    }

    #[test]
    /// A daemon without step events keeps the generic row until it is gone.
    fn an_old_daemon_has_one_stop_row() {
        let begun = Instant::now();
        let mut steps = Steps::default();
        steps.apply(&Progress::Stopping { processes: 1 }, begun);
        steps.apply(&Progress::Stopped, begun + Duration::from_secs(2));
        assert_eq!(steps.rows().len(), 1);
        assert_eq!(steps.rows()[0].text, "stopped the daemon");
        assert_eq!(steps.rows()[0].elapsed, Some(Duration::from_secs(2)));
    }

    #[test]
    /// A failure marks what was running, and says why on the newest row.
    fn a_failure_ends_the_running_rows() {
        let begun = Instant::now();
        let mut steps = Steps::default();
        steps.apply(&Progress::Stopping { processes: 0 }, begun);
        steps.apply(&Progress::Stopped, begun);
        steps.apply(&Progress::Starting("state database".into()), begun);
        steps.fail("USER_CONFIG_INVALID", begun + Duration::from_secs(1));
        let last = steps.rows().last().expect("start row");
        assert_eq!(last.state, RowState::Failed);
        assert_eq!(last.detail.as_deref(), Some("USER_CONFIG_INVALID"));
        assert_eq!(steps.rows()[0].state, RowState::Done);
    }
}
