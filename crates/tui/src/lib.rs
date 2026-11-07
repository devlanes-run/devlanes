//! The devlanes terminal UI: a daemon client like the CLI, with a lane tree,
//! a live terminal pane for one unit, a resource monitor, a daemon tab, and a
//! boxes view that connects to boxes, shows pairing codes for other machines
//! and lists the clients connected to this one.
//!
//! Quitting closes connections and units keep running in the daemon, unless
//! the user chooses to stop the project's lanes on the way out.
//!
//! The crate also builds the app and its drawing for the browser (wasm32);
//! what needs a real terminal (the event loop, the exit summary,
//! the inline step printing) is left out of that build, and what reads the
//! daemon's machine (daemon.log, config.yaml) fails there at run time.

#[cfg(not(target_family = "wasm"))]
mod ansi;
pub mod api;
pub mod app;
pub mod banner;
pub mod blockcopy;
pub mod boxes;
pub mod clients;
pub mod connect;
pub mod daemonlog;
pub mod doctor;
#[cfg(not(target_family = "wasm"))]
pub mod exit_summary;
pub mod graph;
#[cfg(windows)]
mod home;
pub mod hyperlink;
pub mod input;
pub mod keys;
pub mod logfile;
pub mod menu;
pub mod monitor;
#[cfg(not(target_family = "wasm"))]
mod runtime;
pub mod screen;
pub mod settings;
pub mod start;
pub mod steps;
pub mod text;
pub mod theme;
pub mod tree;
pub mod ui;
pub mod welcome;
pub mod whats_new;
pub mod words;

#[cfg(not(target_family = "wasm"))]
use std::{io::IsTerminal, sync::Arc};

pub use api::{
    ApiError, AttachEvent, AttachInput, AttachSession, ClientApi, ConnectEvent, ConnectSession,
    DaemonApi, DaemonMetrics, EventStream, LaneMetrics, Metrics, MetricsQuery, SetupCheck,
    SystemMetrics, UnitMetrics, Window,
};
pub use devlanes_client::Paths;
#[cfg(not(target_family = "wasm"))]
pub use runtime::{TerminalGuard, drive, drive_start};

/// The variable a relaunched TUI reads its [`Reopen`] from, as JSON.
pub const REOPEN_ENV: &str = "DEVLANES_TUI_REOPEN";

/// The variable `devlanes update` hands its step block to the new binary
/// in. Like [`REOPEN_ENV`] it is for one process, never for lanes.
pub const UPDATE_STEPS_ENV: &str = "DEVLANES_UPDATE_STEPS";

/// Where a TUI that closed to update, reload or set up opens again: the lane
/// it showed and its view (`project`, `monitor`, `daemon` or `boxes`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Reopen {
    /// The selected lane's id, if any.
    pub lane: Option<String>,
    /// The view's name.
    pub view: Option<String>,
}

/// Why the TUI closed for the binary to act on, then open it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// `U` confirmed: install the update, restart the daemon into it.
    Update,
    /// The daemon runs a newer devlanes than this TUI: run the TUI installed now.
    Reload,
    /// The setup prompt confirmed: run `devlanes setup` in the terminal.
    Setup,
}

/// A close the binary acts on: why, and where to open again afterwards.
pub type Relaunch = (ExitReason, Reopen);

/// Failures that end the TUI before or while it runs.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the terminal UI needs a terminal; use devlanes status instead")]
    NotATerminal,
    #[error(transparent)]
    Client(#[from] devlanes_client::Error),
    #[error("terminal error: {0}")]
    Io(#[from] std::io::Error),
    /// A background task panicked; its message is already on stderr.
    #[error("the terminal UI stopped after an internal error; units keep running")]
    Panicked,
}

/// What the composition root reads from the user config.
#[derive(Default, Clone)]
pub struct Options {
    /// How to draw start progress in the tab's progress bar, from
    /// [`devlanes_term::progress_support`]; `None` draws nothing.
    pub progress: Option<devlanes_term::Wrap>,
    /// Whether the tab title says which lane is shown and how it stands
    /// ([`devlanes_term::title_support`]); the previous title returns on exit.
    pub title: bool,
    /// Clickable service URLs in the pane, from [`devlanes_term::links_support`].
    pub links: Option<devlanes_term::Links>,
    /// Desktop notifications while the terminal is not focused, from
    /// [`devlanes_term::notifications_support`].
    pub notifications: Option<devlanes_term::Notifications>,
    /// Wraps each frame in synchronized output (`CSI ? 2026 h` / `l`), so the
    /// terminal shows it at once; terminals without it ignore the mode.
    pub sync_output: bool,
    /// `tui.theme` from the user config: `auto` (or unset), `dark`, `light` or `ansi`.
    pub theme: Option<String>,
    /// `tui.glyphs` from the user config: `auto` (or unset), `unicode` or `ascii`.
    pub glyphs: Option<String>,
    /// `tui.on_quit` from the user config: `ask` (or unset), `keep` or `stop`.
    pub on_quit: Option<String>,
    /// `start.restore: false` from the user config: `s` on a lane starts its
    /// default selection instead of the services it ran before.
    pub start_default: bool,
    /// The per-user service definition as `devlanes doctor` reads it
    /// (`{manager, path, installed, stop_window_s, ...}`), for the daemon
    /// page's stop-window check; `None` leaves that check out.
    pub service: Option<serde_json::Value>,
    /// The environment the TUI reads (`NO_COLOR`, `COLORTERM`, the locale,
    /// `TMUX`, `HOME`); `None` reads the process's. Tests pass their own so
    /// the machine they run on does not show in what they draw.
    pub env: Option<std::collections::BTreeMap<String, String>>,
    /// Where daemon.log times are shown; `None` is the system's zone.
    pub time_zone: Option<jiff::tz::TimeZone>,
    /// The directory whose lane opens first; `None` is the process's.
    pub cwd: Option<std::path::PathBuf>,
    /// The CLI's request context; its cwd, `-p`, `--file` and `--lane` pick the
    /// lane the TUI opens on. `None` is the process's directory alone.
    pub scope: Option<devlanes_proto::Context>,
    /// The lane and view of the TUI this one replaces, from [`REOPEN_ENV`];
    /// the lane wins over `cwd`'s.
    pub reopen: Option<Reopen>,
    /// The setup steps only this user can check (the CA's trust), for the
    /// setup prompt; `None` checks nothing on this side.
    pub setup_check: Option<SetupCheck>,
    /// Reads the notes the release highlights come from and claims them;
    /// `None` has none.
    pub whats_new: Option<WhatsNewClaim>,
}

/// The release notes the highlights dialog draws from, taken from the
/// binary's changelog, and whether it opens by itself after the welcome: those since the
/// version seen before when this is the first TUI after an update, else
/// this version's own for the Esc menu. The binary records them as seen in
/// the call, under a lock so that one TUI shows them, which is why the TUI
/// calls it only once its terminal is set up: a TUI that does not open
/// claims nothing. Blocking.
pub type WhatsNewClaim =
    std::sync::Arc<dyn Fn() -> (Option<devlanes_update::notes::WhatsNew>, bool) + Send + Sync>;

impl Options {
    /// The colors and glyphs for this terminal, from the environment and
    /// the user config.
    pub fn theme(&self) -> theme::Theme {
        theme::Theme::detect(self.theme.as_deref(), self.glyphs.as_deref(), |key| {
            self.var(key)
        })
    }

    /// An environment variable from [`Options::env`], else the process's.
    pub fn var(&self, key: &str) -> Option<String> {
        match &self.env {
            Some(env) => env.get(key).cloned(),
            None => std::env::var(key).ok(),
        }
    }
}

/// Opens the TUI for bare `devlanes` and `devlanes ui` with default options.
#[cfg(not(target_family = "wasm"))]
pub async fn run(paths: Paths) -> Result<Option<Relaunch>, Error> {
    run_with(paths, Options::default()).await
}

/// Opens the TUI with the user config's options.
///
/// Connects (spawning the daemon when needed) before touching the terminal,
/// so connection errors print normally. Once the user quit, prints the
/// [`exit_summary`]; a close for an [`ExitReason`] prints nothing and
/// returns it with where to open again.
#[cfg(not(target_family = "wasm"))]
pub async fn run_with(paths: Paths, options: Options) -> Result<Option<Relaunch>, Error> {
    open(paths, options, None)
        .await
        .map(|(_, relaunch)| relaunch)
}

/// Opens the TUI on a start the daemon already accepted (see [`start`]) and
/// follows it until the user quits. Quitting never stops it.
#[cfg(not(target_family = "wasm"))]
pub async fn run_start(
    paths: Paths,
    options: Options,
    start: start::StartHandoff,
) -> Result<(start::StartExit, Option<Relaunch>), Error> {
    let (app, relaunch) = open(paths, options, Some(start)).await?;
    Ok((
        app.start_exit().unwrap_or(start::StartExit::Pending),
        relaunch,
    ))
}

/// Runs the TUI on the real terminal, then prints the [`exit_summary`]
/// unless it closed for an [`ExitReason`]; returns the app as it was when
/// it closed, and that reason with where to open again.
#[cfg(not(target_family = "wasm"))]
async fn open(
    paths: Paths,
    options: Options,
    start: Option<start::StartHandoff>,
) -> Result<(app::App, Option<Relaunch>), Error> {
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        return Err(Error::NotATerminal);
    }
    let api: Arc<dyn DaemonApi> =
        ClientApi::connect(paths, options.setup_check.clone(), options.scope.clone()).await?;
    let guard = TerminalGuard::enter()?;
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.clear()?;
    let events = crossterm::event::EventStream::new();
    let result = runtime::run_loop(
        &mut terminal,
        Arc::clone(&api),
        &options,
        events,
        Box::new(std::io::stdout()),
        start,
    )
    .await;
    drop(guard);
    let app = result?;
    if let Some(relaunch) = app.relaunch() {
        return Ok((app, Some(relaunch)));
    }
    // A box's lane is the box's to report; the summary is this machine's.
    let lane = app
        .selected
        .as_ref()
        .map(|selected| selected.lane.as_str())
        .filter(|lane| app.box_lane(lane).is_none());
    exit_summary::print(api.as_ref(), &app.theme, lane, app.release.0.as_ref()).await;
    Ok((app, None))
}
