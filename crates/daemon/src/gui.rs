//! GUI services (`gui: true`): one GUI host process per service replica,
//! `devlanes-gui host`, which serves the unit's windows on its `devlanes_gui`
//! port.
//!
//! The host starts before the unit, through the same runner (own session or
//! job, hidden console, reaped by PID), with the control token in its
//! environment. Its first line of standard output is the ready line
//! ([`devlanes_gui_proto::ready`]): the variables it announces go into the
//! unit's launch and its record, so `env show` lists them. The daemon then
//! opens a `control` connection, spawns the unit and, once the run is
//! recorded with the host as one of its members (so orphan handling reaches
//! it after a crash), tells the host the unit's process identity. The unit's
//! handle is a [`GuiUnit`]: stopping or reaping the unit stops the host after
//! it, freezing reaches the unit only and tells the host, and viewer input
//! the host reports is lane activity.
//!
//! A host that fails to start fails the unit before its command runs. A host
//! that dies later fails nothing: the unit keeps running, and `status`,
//! `debug why`, the unit's log and its launch notes say so. A machine whose
//! composition root names no backend runs GUI services without a host, as
//! before, with a note.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use async_trait::async_trait;
use bytes::Bytes;
use devlanes_core::{
    ExitOutcome, LaunchSpec, OutputChunk, OutputStream, Power, ProcessIdentity, StopPolicy,
    StopSignal, TerminalSize, Unit,
    env::{EnvLayer, Environment, fold_name},
};
use devlanes_gui_client::{Client, ClientOptions, Event, Events};
use devlanes_gui_proto::{
    Role, Token,
    ready::{HostLine, MAX_READY_LINE, Ready, ReadyError, UnavailableReason},
};
use devlanes_proto::{GuiStatus, RpcError};
use serde_json::{Value, json};
use tokio::sync::{OnceCell, broadcast};

use crate::engine::Inner;
use crate::integrations::{ExitDetails, ManagedUnit, UnitRunner};
use crate::lifecycle::SpawnRequest;
use crate::model::{Kind, LaneRt, LaunchRecord, ProjectRt};
use crate::units::UnitHost;

/// The variable that carries the control token to the host. It never goes on
/// the command line, which other accounts can read.
pub const TOKEN_ENV: &str = "DEVLANES_GUI_TOKEN";

/// How long a host has to print its ready line.
pub const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the control connection has to be welcomed.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

/// How a host is stopped: it ends on the first signal, so the grace is short.
const HOST_STOP: StopPolicy = StopPolicy {
    signal: StopSignal::Terminate,
    timeout: Duration::from_secs(5),
};

/// Bytes of a host's standard error kept for failure reasons.
const STDERR_TAIL: usize = 4 * 1024;

/// The note for a GUI service on a machine without a GUI backend.
pub(crate) const NO_BACKEND_NOTE: &str = "gui: this machine has no GUI host yet, so the service's windows open on this machine's desktop and cannot be viewed from another machine";

/// GUI services as the composition root sets them up.
#[derive(Debug, Clone, Default)]
pub struct GuiOptions {
    /// Project files may set `services.<name>.gui`; otherwise the key fails
    /// to load as "not supported yet".
    pub enabled: bool,
    /// The backend `devlanes-gui host --backend` runs on this machine. `None`
    /// when this OS has none: GUI services then run without a host, their
    /// windows on this machine's desktop, with a note.
    pub backend: Option<String>,
    /// The `devlanes-gui` executable, next to the running `devlanes`.
    pub program: PathBuf,
}

/// Why a GUI host did not come up. Every case fails the unit before its
/// command runs.
#[derive(Debug, thiserror::Error)]
pub(crate) enum HostFailure {
    /// No `devlanes-gui` next to `devlanes`.
    #[error("devlanes-gui is not installed at {}", .0.display())]
    Missing(PathBuf),
    /// The runner could not start it.
    #[error("devlanes-gui could not start: {0}")]
    Spawn(devlanes_core::Error),
    /// It ended without a ready line.
    #[error("devlanes-gui exited before it was ready")]
    Exited(Option<ExitOutcome>),
    /// No ready line within [`READY_TIMEOUT`].
    #[error("devlanes-gui printed no ready line within {}s", READY_TIMEOUT.as_secs())]
    Timeout,
    /// The first line is not a ready line this daemon reads.
    #[error("devlanes-gui printed a line devlanes cannot read: {0}")]
    BadLine(#[from] ReadyError),
    /// The first line is not text, or part of it was lost.
    #[error("devlanes-gui printed a line devlanes cannot read: {0}")]
    Unreadable(&'static str),
    /// It is another devlanes release or speaks another protocol.
    #[error("devlanes-gui is version {version} (GUI protocol {protocol}), not {}", devlanes_version::VERSION)]
    Mismatch { version: String, protocol: u16 },
    /// It listens somewhere else than the unit's port.
    #[error("devlanes-gui listens on {0}, not on the unit's devlanes_gui port")]
    WrongAddress(SocketAddr),
    /// Its backend cannot run on this machine.
    #[error("{message}")]
    Unavailable {
        reason: UnavailableReason,
        message: String,
    },
    /// The control connection was not welcomed.
    #[error("devlanes-gui did not accept the daemon's control connection: {0}")]
    Control(String),
}

impl HostFailure {
    /// The unit's error: `gui_host_missing`, `gui_unavailable` with the
    /// backend's `reason`, or `gui_host_failed` with what went wrong in
    /// `reason`. `stderr` is the end of what the host wrote there.
    pub(crate) fn error(&self, unit: &str, lane: &str, stderr: &str) -> RpcError {
        let mut data = json!({"unit": unit, "lane": lane, "message": self.to_string()});
        let code = match self {
            Self::Missing(path) => {
                data["path"] = json!(path);
                "gui_host_missing"
            }
            Self::Unavailable { reason, .. } => {
                data["reason"] = json!(reason.word());
                "gui_unavailable"
            }
            other => {
                data["reason"] = json!(match other {
                    Self::Spawn(_) => "spawn",
                    Self::Exited(_) => "exited",
                    Self::Timeout => "timeout",
                    Self::BadLine(_) | Self::Unreadable(_) => "bad_ready_line",
                    Self::Mismatch { .. } => "version_mismatch",
                    Self::WrongAddress(_) => "wrong_address",
                    _ => "control",
                });
                if let Self::Exited(Some(outcome)) = other {
                    data["exit_code"] = json!(outcome.code);
                    data["signal"] = json!(outcome.signal);
                }
                if let Self::Mismatch { version, .. } = other {
                    data["version"] = json!(version);
                    data["expected"] = json!(devlanes_version::VERSION);
                }
                "gui_host_failed"
            }
        };
        if !stderr.is_empty() {
            data["stderr"] = json!(stderr);
        }
        RpcError::new(code, data, 1)
    }
}

/// What the status of a GUI service run says about its host.
#[derive(Debug, Clone)]
struct Facts {
    /// A `devlanes_proto::GUI_HOST_*` word.
    host: &'static str,
    real_desktop: Option<bool>,
    /// How the host ended while the unit ran.
    died: Option<ExitOutcome>,
    /// The end of the host's standard error when it died.
    stderr: String,
}

/// A GUI service run's host as status and `debug why` read it.
#[derive(Debug)]
pub(crate) struct GuiState(Mutex<Facts>);

impl GuiState {
    /// A run whose host runs, on a display that is or is not the real desktop.
    fn running(real_desktop: bool) -> Arc<Self> {
        Self::with(devlanes_proto::GUI_HOST_RUNNING, Some(real_desktop))
    }

    /// A run without a host: `GUI_HOST_NONE` on a machine without a GUI
    /// backend, `GUI_HOST_STOPPED` for one whose host never started.
    pub(crate) fn without_host(word: &'static str) -> Arc<Self> {
        Self::with(word, None)
    }

    /// A run with these facts.
    fn with(host: &'static str, real_desktop: Option<bool>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Facts {
            host,
            real_desktop,
            died: None,
            stderr: String::new(),
        })))
    }

    /// The facts, also when a panic poisoned the lock.
    fn facts(&self) -> MutexGuard<'_, Facts> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `UnitSummary.gui` for a run that is or is not `alive`: a host does not
    /// outlive its unit.
    pub(crate) fn status(&self, alive: bool) -> GuiStatus {
        let facts = self.facts();
        let host = match facts.host {
            devlanes_proto::GUI_HOST_NONE => devlanes_proto::GUI_HOST_NONE,
            _ if !alive => devlanes_proto::GUI_HOST_STOPPED,
            word => word,
        };
        GuiStatus {
            host: host.into(),
            real_desktop: facts
                .real_desktop
                .filter(|_| host == devlanes_proto::GUI_HOST_RUNNING),
        }
    }

    /// `debug why`'s reason for a run without a working host: its code and
    /// data.
    pub(crate) fn why(&self) -> Option<(&'static str, Value)> {
        let facts = self.facts();
        match facts.host {
            devlanes_proto::GUI_HOST_EXITED => Some((
                "gui_host_exited",
                json!({
                    "exit_code": facts.died.and_then(|outcome| outcome.code),
                    "signal": facts.died.and_then(|outcome| outcome.signal),
                    "stderr": facts.stderr,
                }),
            )),
            devlanes_proto::GUI_HOST_NONE => Some(("gui_no_backend", json!({}))),
            _ => None,
        }
    }

    /// Records that the host ended while the unit ran; false when it was
    /// stopped with the unit.
    fn died(&self, outcome: Option<ExitOutcome>, stderr: String) -> bool {
        let mut facts = self.facts();
        if facts.host != devlanes_proto::GUI_HOST_RUNNING {
            return false;
        }
        facts.host = devlanes_proto::GUI_HOST_EXITED;
        facts.died = outcome;
        facts.stderr = stderr;
        true
    }

    /// Records that the host was stopped with its unit.
    fn stopped(&self) {
        let mut facts = self.facts();
        if facts.host == devlanes_proto::GUI_HOST_RUNNING {
            facts.host = devlanes_proto::GUI_HOST_STOPPED;
        }
    }
}

/// A host that printed its ready line and welcomed the control connection.
pub(crate) struct StartedHost {
    host: Arc<dyn ManagedUnit>,
    ready: Ready,
    control: Client,
    events: Events,
    stderr: Arc<Mutex<String>>,
}

impl StartedHost {
    /// What the host announced.
    pub(crate) fn ready(&self) -> &Ready {
        &self.ready
    }

    /// Stops the host, for a unit that did not start after all.
    pub(crate) async fn abandon(self) {
        self.control.close();
        let _ = self.host.stop(HOST_STOP).await;
    }

    /// The unit's handle: `unit` with this host attached, and the control
    /// connection's events.
    pub(crate) fn attach(self, unit: Arc<dyn ManagedUnit>) -> (Arc<GuiUnit>, Events) {
        let gui = Arc::new(GuiUnit {
            unit,
            host: self.host,
            control: self.control,
            state: GuiState::running(self.ready.real_desktop),
            stderr: self.stderr,
            finished: OnceCell::new(),
        });
        (gui, self.events)
    }
}

/// What a host starts for.
pub(crate) struct HostRequest<'a> {
    /// The `--backend` name.
    pub backend: &'a str,
    /// The `--service` label viewers show.
    pub service: String,
    /// The unit's `devlanes_gui` port.
    pub port: u16,
}

/// Starts the GUI host for one replica: spawns `devlanes-gui host` through
/// `runner` listening on the request's port, reads its ready line and opens
/// the control connection. The host gets the unit's environment and working
/// directory, so a backend finds the session the unit would
/// (`XDG_RUNTIME_DIR` and the like). On failure the host is stopped again,
/// and the end of its standard error comes with the failure.
pub(crate) async fn start_host(
    runner: &dyn UnitRunner,
    program: &std::path::Path,
    request: HostRequest<'_>,
    unit: &LaunchSpec,
) -> Result<StartedHost, (HostFailure, String)> {
    let path = program.to_owned();
    let present = tokio::task::spawn_blocking(move || path.is_file())
        .await
        .unwrap_or(false);
    if !present {
        return Err((HostFailure::Missing(program.to_owned()), String::new()));
    }
    let token = token().map_err(|error| (HostFailure::Spawn(error), String::new()))?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, request.port));
    let mut env = unit.env.clone();
    devlanes_core::env::overlay(&mut env, [(TOKEN_ENV.to_owned(), token.clone())]);
    let spec = LaunchSpec {
        program: program.display().to_string(),
        args: vec![
            "host".into(),
            "--backend".into(),
            request.backend.into(),
            "--listen".into(),
            address.to_string(),
            "--service".into(),
            request.service,
        ],
        cwd: unit.cwd.clone(),
        env,
        pty: false,
        size: TerminalSize::default(),
    };
    let host = runner
        .spawn(spec)
        .await
        .map_err(|error| (HostFailure::Spawn(error), String::new()))?;
    // The first subscriber sees everything since the spawn.
    let mut output = host.output();
    let stderr = Arc::new(Mutex::new(String::new()));
    let read = read_line(&mut output, &stderr, READY_TIMEOUT).await;
    let ready = match read.and_then(|line| check(&line, address)) {
        Ok(ready) => ready,
        Err(failure) => {
            let failure = match failure {
                HostFailure::Exited(_) => HostFailure::Exited(
                    tokio::time::timeout(HOST_STOP.timeout, host.wait())
                        .await
                        .ok()
                        .and_then(Result::ok),
                ),
                other => other,
            };
            let _ = host.stop(HOST_STOP).await;
            return Err((failure, tail(&stderr)));
        }
    };
    // Kept for the reason of a host that dies later.
    tokio::spawn(drain_stderr(output, stderr.clone()));
    let options = ClientOptions::new(
        Role::Control,
        format!("devlanes-daemon:{}", std::process::id()),
        "devlanes daemon",
    )
    .with_token(Token::new(token));
    let connected = tokio::time::timeout(
        CONTROL_TIMEOUT,
        devlanes_gui_client::connect(address, options),
    )
    .await;
    let (control, events) = match connected {
        Ok(Ok(connected)) => connected,
        Ok(Err(error)) => {
            let _ = host.stop(HOST_STOP).await;
            return Err((HostFailure::Control(error.to_string()), tail(&stderr)));
        }
        Err(_) => {
            let _ = host.stop(HOST_STOP).await;
            let message = format!("no answer within {}s", CONTROL_TIMEOUT.as_secs());
            return Err((HostFailure::Control(message), tail(&stderr)));
        }
    };
    Ok(StartedHost {
        host,
        ready,
        control,
        events,
        stderr,
    })
}

/// A random control token: 32 bytes as hex.
fn token() -> Result<String, devlanes_core::Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| devlanes_core::Error::Io(std::io::Error::other(error.to_string())))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The first line of standard output, without its line ending, read with a
/// cap of [`MAX_READY_LINE`] and within `timeout`. Standard error goes into
/// `stderr`, which keeps its last [`STDERR_TAIL`] bytes.
pub(crate) async fn read_line(
    output: &mut broadcast::Receiver<OutputChunk>,
    stderr: &Mutex<String>,
    timeout: Duration,
) -> Result<String, HostFailure> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut line: Vec<u8> = Vec::new();
    loop {
        let chunk = match tokio::time::timeout_at(deadline, output.recv()).await {
            Err(_) => return Err(HostFailure::Timeout),
            Ok(Err(broadcast::error::RecvError::Closed)) => return Err(HostFailure::Exited(None)),
            // Lost bytes could be part of the line.
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                return Err(HostFailure::Unreadable("part of it was lost"));
            }
            Ok(Ok(chunk)) => chunk,
        };
        if chunk.stream == OutputStream::Stderr {
            keep_tail(stderr, &chunk.bytes);
            continue;
        }
        let end = chunk.bytes.iter().position(|byte| *byte == b'\n');
        line.extend_from_slice(&chunk.bytes[..end.unwrap_or(chunk.bytes.len())]);
        if line.len() > MAX_READY_LINE + 1 {
            return Err(ReadyError::TooLong(line.len()).into());
        }
        if end.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return String::from_utf8(line).map_err(|_| HostFailure::Unreadable("it is not UTF-8"));
        }
    }
}

/// Reads `line` as the host's first line and checks that it belongs to this
/// devlanes and listens on `address`.
fn check(line: &str, address: SocketAddr) -> Result<Ready, HostFailure> {
    match HostLine::parse(line)? {
        HostLine::Unready(unready) => Err(HostFailure::Unavailable {
            reason: unready.reason,
            message: unready.message,
        }),
        HostLine::Ready(ready) => {
            if ready.devlanes != devlanes_version::VERSION
                || ready.protocol != devlanes_gui_proto::PROTOCOL_VERSION
            {
                return Err(HostFailure::Mismatch {
                    version: ready.devlanes,
                    protocol: ready.protocol,
                });
            }
            if ready.address.port() != address.port() {
                return Err(HostFailure::WrongAddress(ready.address));
            }
            Ok(ready)
        }
    }
}

/// Appends `bytes` to `tail`, keeping its last [`STDERR_TAIL`] bytes.
fn keep_tail(tail: &Mutex<String>, bytes: &Bytes) {
    let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
    tail.push_str(&String::from_utf8_lossy(bytes));
    if tail.len() > STDERR_TAIL {
        let mut cut = tail.len() - STDERR_TAIL;
        while !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail.drain(..cut);
    }
}

/// The kept standard error without escapes and surrounding blank lines.
fn tail(stderr: &Mutex<String>) -> String {
    let text = stderr.lock().unwrap_or_else(PoisonError::into_inner).clone();
    strip_ansi_escapes::strip_str(&text).trim().to_owned()
}

/// Keeps reading the host's output after the ready line, its standard error
/// into `stderr`, until the host ends.
async fn drain_stderr(mut output: broadcast::Receiver<OutputChunk>, stderr: Arc<Mutex<String>>) {
    loop {
        match output.recv().await {
            Ok(chunk) if chunk.stream == OutputStream::Stderr => keep_tail(&stderr, &chunk.bytes),
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Applies what `ready` announces to the unit's `spec` and returns the
/// launch record with it: its `set` variables over every other layer, as
/// [`EnvLayer::Gui`], and its `remove` variables taken out, names matched the
/// way the OS matches them.
pub(crate) fn apply_env(
    spec: &mut LaunchSpec,
    launch: &LaunchRecord,
    ready: &Ready,
) -> LaunchRecord {
    let removed: Vec<String> = ready
        .remove
        .iter()
        .map(|name| fold_name(name.clone()))
        .collect();
    spec.env
        .retain(|name, _| !removed.contains(&fold_name(name.clone())));
    devlanes_core::env::overlay(
        &mut spec.env,
        ready
            .set
            .iter()
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    let mut env = Environment {
        values: launch.env.values.clone(),
        sources: launch.env.sources.clone(),
        names: launch.env.names.clone(),
    };
    for key in &removed {
        env.values.remove(key);
        env.sources.remove(key);
        env.names.remove(key);
    }
    for (name, value) in &ready.set {
        let key = fold_name(name.clone());
        if key == *name {
            env.names.remove(&key);
        } else {
            env.names.insert(key.clone(), name.clone());
        }
        env.sources.insert(key.clone(), EnvLayer::Gui);
        env.values.insert(key, value.clone());
    }
    let mut notes = launch.notes.clone();
    if !ready.remove.is_empty() {
        notes.push(format!(
            "gui: the GUI host removed {} from this unit's environment",
            ready.remove.join(", ")
        ));
    }
    LaunchRecord {
        env,
        notes,
        left_out: launch.left_out.clone(),
    }
}

/// A GUI service run: the unit's process with its host attached. The unit is
/// what every caller sees (output, identity, exit); the host follows it.
pub(crate) struct GuiUnit {
    unit: Arc<dyn ManagedUnit>,
    host: Arc<dyn ManagedUnit>,
    control: Client,
    state: Arc<GuiState>,
    stderr: Arc<Mutex<String>>,
    /// Set once the host was stopped with the unit.
    finished: OnceCell<()>,
}

impl GuiUnit {
    /// What status reads about the host.
    pub(crate) fn state(&self) -> Arc<GuiState> {
        self.state.clone()
    }

    /// The host process's identity, recorded with the unit's run.
    pub(crate) fn host_identity(&self) -> Option<ProcessIdentity> {
        self.host.identity()
    }

    /// Tells the host the unit's process identity, once its run is recorded.
    /// A Windows job name comes with the Windows host.
    pub(crate) fn tell_identity(&self) {
        let Some(identity) = self.unit.identity() else {
            tracing::warn!("the unit has no identity to tell its GUI host");
            return;
        };
        if let Err(error) = self
            .control
            .unit_process(identity.pid, identity.start_time, None)
        {
            tracing::debug!(%error, "the GUI host did not get the unit's identity");
        }
    }

    /// Resolves when the host ended on its own while the unit ran, with how
    /// and the end of its standard error; never when the host was stopped
    /// with the unit.
    pub(crate) async fn host_died(&self) -> (Option<ExitOutcome>, String) {
        let outcome = self.host.wait().await.ok();
        let stderr = tail(&self.stderr);
        if self.finished.initialized() || !self.state.died(outcome, stderr.clone()) {
            return std::future::pending().await;
        }
        self.control.close();
        (outcome, stderr)
    }

    /// Stops the host after the unit ended with `code`, once. The exit notice
    /// has no answer to wait for; the host reports its windows Closed when it
    /// stops either way.
    async fn finish(&self, code: Option<i32>) {
        self.finished
            .get_or_init(|| async {
                let _ = self.control.unit_exited(code);
                self.state.stopped();
                if let Err(error) = self.host.stop(HOST_STOP).await {
                    tracing::warn!(%error, "the GUI host did not stop");
                }
                let _ = tokio::time::timeout(HOST_STOP.timeout, self.host.wait()).await;
                self.control.close();
            })
            .await;
    }
}

#[async_trait]
impl Unit for GuiUnit {
    /// The unit's terminal.
    async fn input(&self, bytes: Bytes) -> Result<(), devlanes_core::Error> {
        self.unit.input(bytes).await
    }

    /// The unit's terminal.
    async fn resize(&self, size: TerminalSize) -> Result<(), devlanes_core::Error> {
        self.unit.resize(size).await
    }

    /// Stops the unit, then its host, so the host is gone (and its port free)
    /// when the stop returns.
    async fn stop(&self, policy: StopPolicy) -> Result<(), devlanes_core::Error> {
        self.unit.stop(policy).await?;
        let code = self
            .unit
            .exit_details()
            .and_then(|details| details.outcome.code);
        self.finish(code).await;
        Ok(())
    }

    /// Waits for the unit's exit and stops the host after it, so a reaped
    /// unit never leaves its host running.
    async fn wait(&self) -> Result<ExitOutcome, devlanes_core::Error> {
        let outcome = self.unit.wait().await;
        self.finish(outcome.as_ref().ok().and_then(|outcome| outcome.code))
            .await;
        outcome
    }

    /// The unit's output.
    fn output(&self) -> broadcast::Receiver<OutputChunk> {
        self.unit.output()
    }

    /// The unit's processes and, while it runs, the host's, so metrics count
    /// the host and a recorded run reaches it.
    async fn subjects(&self) -> Result<Vec<ProcessIdentity>, devlanes_core::Error> {
        let mut subjects = self.unit.subjects().await?;
        if !self.finished.initialized()
            && let Ok(host) = self.host.subjects().await
        {
            subjects.extend(host);
        }
        Ok(subjects)
    }

    /// The unit's terminal.
    fn renders_screen(&self) -> bool {
        self.unit.renders_screen()
    }

    /// Changes the unit alone and tells the host when the lane freezes and
    /// thaws: the host keeps running, so a click in a frozen lane's window
    /// still reaches the daemon and thaws the lane.
    async fn set_power(&self, power: Power) -> Result<(), devlanes_core::Error> {
        self.unit.set_power(power).await?;
        let told = match power {
            Power::Frozen => self.control.lane_frozen(true),
            Power::Normal => self.control.lane_frozen(false),
            Power::Throttled => Ok(()),
        };
        if let Err(error) = told {
            tracing::debug!(%error, "the GUI host was not told the lane's power");
        }
        Ok(())
    }
}

#[async_trait]
impl ManagedUnit for GuiUnit {
    /// The unit's leader.
    fn identity(&self) -> Option<ProcessIdentity> {
        self.unit.identity()
    }

    /// The unit's ports; the host's port is devlanes' own.
    async fn listening_ports(&self) -> Vec<u16> {
        self.unit.listening_ports().await
    }

    /// The unit's.
    fn exit_details(&self) -> Option<ExitDetails> {
        self.unit.exit_details()
    }
}

/// Forwards the activity the host reports on its control connection to the
/// idle board as activity of `lane` until the connection closes.
pub(crate) async fn forward_activity(
    mut events: Events,
    board: Arc<crate::idle::IdleBoard>,
    lane: String,
) {
    while let Some(event) = events.next().await {
        match event {
            Event::Activity => board.activity(&lane, crate::idle::Activity::Gui),
            Event::Closed(_) => return,
            _ => {}
        }
    }
}

/// What [`Inner::start_gui`] brought up for a unit.
pub(crate) enum GuiStart {
    /// Not a GUI service, or a command that never gets a host.
    NotGui,
    /// A GUI service on a machine without a GUI backend: it runs as before.
    NoBackend,
    /// The host runs and its variables are in the unit's launch.
    Host(StartedHost),
}

impl Inner {
    /// Starts the GUI host of `request`'s service replica, for a GUI service
    /// on a machine with a backend, and puts the host's variables into the
    /// request's launch and record. Without a backend the record gets
    /// [`NO_BACKEND_NOTE`]. Fails with the unit's error when the host does not
    /// come up; `reference` is the lane's.
    pub(crate) async fn start_gui(
        &self,
        reference: &str,
        request: &mut SpawnRequest,
    ) -> Result<GuiStart, RpcError> {
        if !request.gui || request.kind != Kind::Service {
            return Ok(GuiStart::NotGui);
        }
        let Some(backend) = self.opts.gui.backend.as_deref() else {
            let mut launch = LaunchRecord {
                env: crate::lifecycle::copy_env(&request.launch.env),
                notes: request.launch.notes.clone(),
                left_out: request.launch.left_out.clone(),
            };
            launch.notes.push(NO_BACKEND_NOTE.to_owned());
            request.launch = Arc::new(launch);
            return Ok(GuiStart::NoBackend);
        };
        let Some(port) = request.ports.get(devlanes_core::ports::GUI_PORT).copied() else {
            return Err(crate::units::internal(
                "gui_host_failed",
                format!("{} has no {} port", request.name, devlanes_core::ports::GUI_PORT),
            ));
        };
        let host = HostRequest {
            backend,
            service: format!("{reference}:{}", request.name),
            port,
        };
        let started = match start_host(
            self.integ.runner.as_ref(),
            &self.opts.gui.program,
            host,
            &request.spec,
        )
        .await
        {
            Ok(started) => started,
            Err((failure, stderr)) => {
                tracing::warn!(unit = %request.name, lane = %reference, "GUI host did not start: {failure}");
                let mut error = failure.error(&request.name, reference, &stderr);
                if matches!(failure, HostFailure::Missing(_)) {
                    // `devlanes update` fetches a missing devlanes-gui for a
                    // script install; a package manager reinstalls both.
                    let (method, _) = self.install_method().await;
                    if !matches!(method, devlanes_update::InstallMethod::Script(_)) {
                        error.data["install"] =
                            json!(method.upgrade_command(devlanes_version::VERSION));
                    }
                }
                return Err(error);
            }
        };
        let launch = apply_env(&mut request.spec, &request.launch, started.ready());
        request.launch = Arc::new(launch);
        Ok(GuiStart::Host(started))
    }

    /// Watches the host of the run `host` of unit `name`: when it ends on its
    /// own while the unit runs, the unit keeps running, its log and launch
    /// notes say so, and `unit.changed` goes out.
    pub(crate) fn watch_gui_host(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        name: &str,
        host: &UnitHost,
        unit: Arc<GuiUnit>,
    ) {
        let (engine, project, lane, name, host) = (
            self.clone(),
            project.clone(),
            lane.clone(),
            name.to_owned(),
            host.clone(),
        );
        tokio::spawn(async move {
            let (outcome, _) = tokio::select! {
                died = unit.host_died() => died,
                _ = host.wait_exit() => return,
            };
            let note = died_note(outcome);
            tracing::warn!(unit = %name, "GUI host exited while its unit runs");
            engine
                .write_note(host.info().log_dir.clone(), &name, &note)
                .await;
            if let Some(rec) = lane.st().unit_mut(&name).filter(|rec| rec.host.same_run(&host)) {
                let mut notes = rec.launch.notes.clone();
                notes.push(note.trim_start_matches(crate::spawn_error::LOG_PREFIX).trim().to_owned());
                rec.launch = Arc::new(LaunchRecord {
                    env: crate::lifecycle::copy_env(&rec.launch.env),
                    notes,
                    left_out: rec.launch.left_out.clone(),
                });
            }
            engine.emit_unit(&project, &lane, &name, &host);
        });
    }
}

/// The `[devlanes]` line for a host that ended while its unit runs on.
pub(crate) fn died_note(outcome: Option<ExitOutcome>) -> String {
    let how = match outcome {
        Some(ExitOutcome {
            code: Some(code), ..
        }) => format!(" with exit code {code}"),
        Some(ExitOutcome {
            signal: Some(signal),
            ..
        }) => format!(" by signal {signal}"),
        _ => String::new(),
    };
    format!(
        "{} the GUI host exited{how}; the service keeps running, but its windows cannot be viewed until it restarts",
        crate::spawn_error::LOG_PREFIX
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host's output channel and the sender a test writes it with.
    fn output() -> (
        broadcast::Sender<OutputChunk>,
        broadcast::Receiver<OutputChunk>,
    ) {
        broadcast::channel(16)
    }

    /// A chunk of `stream`.
    fn chunk(stream: OutputStream, text: &[u8]) -> OutputChunk {
        OutputChunk {
            stream,
            bytes: Bytes::copy_from_slice(text),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_ready_line_is_read_whole_capped_and_in_time() {
        let stderr = Mutex::new(String::new());
        // A line in pieces, with standard error in between and a CRLF end.
        let (tx, mut rx) = output();
        tx.send(chunk(OutputStream::Stdout, b"{\"rea")).ok();
        tx.send(chunk(OutputStream::Stderr, b"starting\n")).ok();
        tx.send(chunk(OutputStream::Stdout, b"dy\":1}\r\nmore")).ok();
        let line = read_line(&mut rx, &stderr, READY_TIMEOUT).await;
        assert_eq!(line.expect("a line"), "{\"ready\":1}");
        assert_eq!(tail(&stderr), "starting");

        let (tx, mut rx) = output();
        let long = vec![b'x'; MAX_READY_LINE + 2];
        tx.send(chunk(OutputStream::Stdout, &long)).ok();
        let line = read_line(&mut rx, &stderr, READY_TIMEOUT).await;
        assert!(
            matches!(line, Err(HostFailure::BadLine(ReadyError::TooLong(_)))),
            "{line:?}"
        );

        let (tx, mut rx) = output();
        let started = tokio::time::Instant::now();
        let line = read_line(&mut rx, &stderr, READY_TIMEOUT).await;
        assert!(matches!(line, Err(HostFailure::Timeout)), "{line:?}");
        assert!(started.elapsed() >= READY_TIMEOUT);
        drop(tx);

        let (tx, mut rx) = output();
        drop(tx);
        let line = read_line(&mut rx, &stderr, READY_TIMEOUT).await;
        assert!(matches!(line, Err(HostFailure::Exited(_))), "{line:?}");
    }

    #[test]
    fn a_ready_line_of_another_release_or_port_is_refused() {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 41234));
        let line = |version: &str, port: u16| {
            Ready::new(
                SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                version,
                &[],
                false,
            )
            .to_line()
        };
        assert!(check(&line(devlanes_version::VERSION, 41234), address).is_ok());
        let other = check(&line("0.0.1", 41234), address);
        assert!(matches!(other, Err(HostFailure::Mismatch { .. })), "{other:?}");
        let moved = check(&line(devlanes_version::VERSION, 41235), address);
        assert!(matches!(moved, Err(HostFailure::WrongAddress(_))), "{moved:?}");
        let unready = devlanes_gui_proto::ready::Unready::new(
            devlanes_version::VERSION,
            UnavailableReason::NoDesktopSession,
            "no desktop session",
        );
        let refused = check(&unready.to_line(), address);
        assert!(
            matches!(
                refused,
                Err(HostFailure::Unavailable {
                    reason: UnavailableReason::NoDesktopSession,
                    ..
                })
            ),
            "{refused:?}"
        );
    }
}
