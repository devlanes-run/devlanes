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
//! `debug why`, the daemon log and the unit's launch notes say so. A machine whose
//! composition root names no backend runs GUI services without a host, as
//! before, with a note.

pub mod lookup;
pub mod viewer;

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

use devlanes_gui_proto::ready::TOKEN_ENV;

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
pub(crate) const NO_BACKEND_NOTE: &str = "gui: this machine has no GUI host, so the service's windows open on this machine's desktop and cannot be viewed from another machine";

/// How long after its control connection closed a host still counts as
/// dying rather than as having lost its connection.
const DEATH_GRACE: Duration = Duration::from_secs(1);

/// How long a host that was told its unit exited gets to report its windows
/// Closed and end by itself, before it is stopped.
const EXIT_GRACE: Duration = Duration::from_secs(3);

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
    /// It speaks another GUI protocol. Another devlanes release that speaks
    /// the same one is accepted, with a note.
    #[error(
        "devlanes-gui {version} speaks GUI protocol {protocol}, and this daemon {}",
        devlanes_gui_proto::PROTOCOL_VERSION
    )]
    Mismatch { version: String, protocol: u16 },
    /// Its ready line has a format this daemon does not read.
    #[error(
        "devlanes-gui {version} prints ready line format {format}, which this daemon does not read"
    )]
    Format { version: String, format: u64 },
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
                    Self::Mismatch { .. } | Self::Format { .. } => "version_mismatch",
                    Self::WrongAddress(_) => "wrong_address",
                    _ => "control",
                });
                if let Self::Exited(Some(outcome)) = other {
                    data["exit_code"] = json!(outcome.code);
                    data["signal"] = json!(outcome.signal);
                }
                if let Self::Mismatch { version, protocol } = other {
                    data["version"] = json!(version);
                    data["protocol"] = json!(protocol);
                    data["daemon"] = json!(devlanes_version::VERSION);
                    data["fix"] = json!(mismatch_fix(version));
                }
                if let Self::Format { version, format } = other {
                    data["version"] = json!(version);
                    data["format"] = json!(format);
                    data["daemon"] = json!(devlanes_version::VERSION);
                    data["fix"] = json!(mismatch_fix(version));
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

/// What fixes a host of devlanes `version` that speaks another GUI protocol
/// than this daemon: `restart` when the installed `devlanes-gui` is newer,
/// so the `devlanes` installed with it is newer than this running daemon
/// too (an update replaced both and the daemon still runs the old one), and
/// `update` when `devlanes-gui` is the older one.
fn mismatch_fix(version: &str) -> &'static str {
    if devlanes_update::version::is_newer(version, devlanes_version::VERSION) {
        "restart"
    } else {
        "update"
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
    /// The unit is being restarted, so its host goes without telling viewers
    /// the service closed.
    restarting: bool,
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
            restarting: false,
        })))
    }

    /// Marks the run as stopped for a restart: its host is stopped without
    /// the unit's exit, so viewers wait for the next host.
    pub(crate) fn restarting(&self) {
        self.facts().restarting = true;
    }

    /// Whether the run is stopped for a restart.
    fn is_restarting(&self) -> bool {
        self.facts().restarting
    }

    /// Whether the host runs and nothing is ending it.
    fn runs(&self) -> bool {
        self.facts().host == devlanes_proto::GUI_HOST_RUNNING
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
    /// A note for the unit's launch, such as a host of another release.
    note: Option<String>,
    control: Client,
    events: Events,
    stderr: Arc<Mutex<String>>,
}

impl StartedHost {
    /// What the host announced.
    pub(crate) fn ready(&self) -> &Ready {
        &self.ready
    }

    /// Stops the host and waits for it, for a unit that did not start after
    /// all.
    pub(crate) async fn abandon(self) {
        self.control.close();
        stop_host(self.host.as_ref()).await;
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
    let (ready, note) = match read.and_then(|line| check(&line, address)) {
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
            stop_host(host.as_ref()).await;
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
            stop_host(host.as_ref()).await;
            return Err((HostFailure::Control(error.to_string()), tail(&stderr)));
        }
        Err(_) => {
            stop_host(host.as_ref()).await;
            let message = format!("no answer within {}s", CONTROL_TIMEOUT.as_secs());
            return Err((HostFailure::Control(message), tail(&stderr)));
        }
    };
    Ok(StartedHost {
        host,
        ready,
        note,
        control,
        events,
        stderr,
    })
}

/// Stops `host` and waits for it to end, within [`HOST_STOP`]'s grace, so a
/// next host does not find the port still held. A host that already ended
/// has nothing to stop.
async fn stop_host(host: &dyn ManagedUnit) {
    match host.stop(HOST_STOP).await {
        Ok(()) | Err(devlanes_core::Error::Exited) => {}
        Err(error) => tracing::warn!(%error, "the GUI host did not stop"),
    }
    let _ = tokio::time::timeout(HOST_STOP.timeout, host.wait()).await;
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
/// The format version is checked by the reader. Compatibility is the GUI
/// protocol's: after an update the installed `devlanes-gui` is the new
/// release while this daemon still runs the old one, and a host of another
/// release that speaks the same protocol works, with the note this returns.
fn check(line: &str, address: SocketAddr) -> Result<(Ready, Option<String>), HostFailure> {
    let parsed = match HostLine::parse(line) {
        // A later format: when the line still names the host's release, it
        // is a mismatch whose fix that release tells.
        Err(ReadyError::Format(format)) => {
            let version = serde_json::from_str::<Value>(line).ok().and_then(|line| {
                line.get("devlanes")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
            return Err(match version {
                Some(version) => HostFailure::Format { version, format },
                None => ReadyError::Format(format).into(),
            });
        }
        parsed => parsed?,
    };
    match parsed {
        HostLine::Unready(unready) => Err(HostFailure::Unavailable {
            reason: unready.reason,
            message: unready.message,
        }),
        HostLine::Ready(ready) => {
            if ready.protocol != devlanes_gui_proto::PROTOCOL_VERSION {
                return Err(HostFailure::Mismatch {
                    version: ready.devlanes,
                    protocol: ready.protocol,
                });
            }
            if ready.address.port() != address.port() {
                return Err(HostFailure::WrongAddress(ready.address));
            }
            let note = (ready.devlanes != devlanes_version::VERSION).then(|| {
                format!(
                    "gui: the GUI host is devlanes-gui {}, and this daemon devlanes {}; they speak the same GUI protocol",
                    ready.devlanes,
                    devlanes_version::VERSION
                )
            });
            Ok((ready, note))
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
    let text = stderr
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
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
    // The host's value wins, since the unit must reach its display; a value
    // the user set is named, as removals are.
    let replaced: Vec<&str> = ready
        .set
        .keys()
        .filter(|name| {
            matches!(
                env.sources.get(&fold_name((*name).clone())),
                Some(EnvLayer::Global | EnvLayer::Service | EnvLayer::Profile | EnvLayer::Override)
            )
        })
        .map(String::as_str)
        .collect();
    let mut notes = launch.notes.clone();
    if !replaced.is_empty() {
        notes.push(format!(
            "gui: the GUI host's {} replaces the value this unit's config or --env set",
            replaced.join(", ")
        ));
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

    /// Ends the host after the unit ended with `code`, once.
    ///
    /// For a stop, or a unit that exited by itself, the host is told the
    /// unit exited: it reports its windows Closed and ends by itself, and is
    /// stopped only when it has not within [`EXIT_GRACE`]. That order makes
    /// sure viewers see Closed before the host goes. For a restart the host
    /// is stopped without that notice, so viewers see the connection close
    /// and reconnect to the next host.
    async fn finish(&self, code: Option<i32>) {
        self.finished
            .get_or_init(|| async {
                let restarting = self.state.is_restarting();
                self.state.stopped();
                if !restarting && self.control.unit_exited(code).is_ok() {
                    // Closing writes what is queued first.
                    self.control.close();
                    if tokio::time::timeout(EXIT_GRACE, self.host.wait())
                        .await
                        .is_ok()
                    {
                        return;
                    }
                }
                self.control.close();
                stop_host(self.host.as_ref()).await;
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

/// What [`Inner::start_gui`] brought up for a unit.
pub(crate) enum GuiStart {
    /// No host: not a GUI service, a command that never gets one, or a GUI
    /// service whose own spawn will fail first (its working directory is
    /// missing), which status shows as a stopped host.
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
                format!(
                    "{} has no {} port",
                    request.name,
                    devlanes_core::ports::GUI_PORT
                ),
            ));
        };
        // A missing working directory fails the unit's own spawn, which says
        // so better than a host that could not start there.
        let cwd = request.spec.cwd.clone();
        if !tokio::task::spawn_blocking(move || cwd.is_dir())
            .await
            .unwrap_or(true)
        {
            return Ok(GuiStart::NotGui);
        }
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
        let mut launch = apply_env(&mut request.spec, &request.launch, started.ready());
        launch.notes.extend(started.note.clone());
        request.launch = Arc::new(launch);
        Ok(GuiStart::Host(started))
    }

    /// Watches the host of the run `host` of unit `name` until the run ends:
    /// forwards the activity its control connection reports as activity of
    /// lane `reference`; when the host ends on its own while the unit runs,
    /// or its control connection closes while both still run, the unit keeps
    /// running, its launch notes and the daemon log say so, and
    /// `unit.changed` goes out.
    pub(crate) fn watch_gui_host(
        self: &Arc<Self>,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        (name, reference): (&str, &str),
        host: &UnitHost,
        (unit, events): (Arc<GuiUnit>, Events),
    ) {
        let (engine, project, lane, name, reference, host) = (
            self.clone(),
            project.clone(),
            lane.clone(),
            name.to_owned(),
            reference.to_owned(),
            host.clone(),
        );
        let board = self.opts.idle.clone();
        tokio::spawn(async move {
            let mut events = Some(events);
            let note = loop {
                let event = async {
                    match events.as_mut() {
                        Some(events) => events.next().await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    (outcome, _) = unit.host_died() => break died_note(outcome),
                    _ = host.wait_exit() => return,
                    event = event => match event {
                        Some(Event::Activity) => board.activity(&reference, crate::idle::Activity::Gui),
                        Some(Event::Closed(_)) | None => {
                            events = None;
                            // A host that dies closes the connection first.
                            let grace = tokio::time::timeout(DEATH_GRACE, unit.host_died()).await;
                            if let Ok((outcome, _)) = grace {
                                break died_note(outcome);
                            }
                            // Not when the daemon closed it to end the host.
                            if unit.state.runs() && host.is_alive() {
                                tracing::warn!(unit = %name, "the GUI host's control connection closed while both run");
                                break CONTROL_LOST_NOTE.to_owned();
                            }
                        }
                        Some(_) => {}
                    },
                }
            };
            engine.note_unit(&project, &lane, &name, &host, &note).await;
            if note == CONTROL_LOST_NOTE {
                // The host may still die later; that is said too.
                tokio::select! {
                    (outcome, _) = unit.host_died() => {
                        engine.note_unit(&project, &lane, &name, &host, &died_note(outcome)).await;
                    }
                    _ = host.wait_exit() => {}
                }
            }
        });
    }

    /// Adds `note` (a `[devlanes]` line) to the run `host`'s launch notes and
    /// the daemon log, and emits `unit.changed`. The unit's own log is held
    /// by its writer while it runs, so the note does not go there.
    async fn note_unit(
        &self,
        project: &Arc<ProjectRt>,
        lane: &Arc<LaneRt>,
        name: &str,
        host: &UnitHost,
        note: &str,
    ) {
        tracing::warn!(unit = %name, "{note}");
        if let Some(rec) = lane
            .st()
            .unit_mut(name)
            .filter(|rec| rec.host.same_run(host))
        {
            let mut notes = rec.launch.notes.clone();
            notes.push(
                note.trim_start_matches(crate::spawn_error::LOG_PREFIX)
                    .trim()
                    .to_owned(),
            );
            rec.launch = Arc::new(LaunchRecord {
                env: crate::lifecycle::copy_env(&rec.launch.env),
                notes,
                left_out: rec.launch.left_out.clone(),
            });
        }
        self.emit_unit(project, lane, name, host);
    }
}

/// The `[devlanes]` line for a host whose control connection closed while it
/// and its unit run.
const CONTROL_LOST_NOTE: &str = "[devlanes] the daemon lost its control connection to the GUI host; the service and its windows keep running, but freezing and viewer activity no longer reach the host until the service restarts";

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
        tx.send(chunk(OutputStream::Stdout, b"dy\":1}\r\nmore"))
            .ok();
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
    fn a_host_is_judged_by_its_protocol_not_its_release() {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 41234));
        let line = |version: &str, port: u16, protocol: u16| {
            let mut ready = Ready::new(
                SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                version,
                &[],
                false,
            );
            ready.protocol = protocol;
            ready.to_line()
        };
        let ours = devlanes_gui_proto::PROTOCOL_VERSION;
        let (_, note) =
            check(&line(devlanes_version::VERSION, 41234, ours), address).expect("ours");
        assert_eq!(note, None);
        // An update replaced devlanes-gui while this daemon runs on: it works.
        let (_, note) = check(&line("999.0.0", 41234, ours), address).expect("same protocol");
        assert!(note.is_some_and(|note| note.contains("999.0.0")));
        // Another protocol fails, with the fix for which side is behind.
        for (version, fix) in [("999.0.0", "restart"), ("0.0.1", "update")] {
            let failure = check(&line(version, 41234, ours + 1), address).expect_err(version);
            assert!(
                matches!(failure, HostFailure::Mismatch { .. }),
                "{failure:?}"
            );
            let error = failure.error("desktop", "acme", "");
            assert_eq!(error.data["reason"], "version_mismatch");
            assert_eq!(error.data["fix"], fix, "{version}");
        }
        // A ready line format this daemon does not know: the same fix when
        // the line names the release, else a line that does not read.
        let later = line("999.0.0", 41234, ours).replace(r#""ready":1"#, r#""ready":2"#);
        let error = check(&later, address)
            .expect_err("later format")
            .error("desktop", "acme", "");
        assert_eq!(error.data["reason"], "version_mismatch");
        assert_eq!(error.data["fix"], "restart");
        let nameless = check(r#"{"ready":2}"#, address).expect_err("no release");
        assert!(matches!(nameless, HostFailure::BadLine(_)), "{nameless:?}");
        let moved = check(&line(devlanes_version::VERSION, 41235, ours), address);
        assert!(
            matches!(moved, Err(HostFailure::WrongAddress(_))),
            "{moved:?}"
        );
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
