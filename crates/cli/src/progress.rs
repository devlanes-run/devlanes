//! Which commands drive the terminal's progress bar, and for which lanes. The
//! encoding, detection and start computation live in `devlanes-term`; the
//! binary follows events and writes the bar.

use devlanes_proto::{Method, Request};
use devlanes_term::{ProgressMode, StartTracker, Wrap, progress_support};
use serde_json::Value;

use crate::{OutputMode, Runtime};

/// How long a failed start's red bar stays before the command exits and clears it.
pub const ERROR_HOLD: std::time::Duration = std::time::Duration::from_secs(1);

/// The tracker for a request that follows a start: `start`, `restart`,
/// `lane resume`, `lane new --start` and `wait`. `None` for everything else.
/// It reads the request's own lane events, which the daemon limits to the
/// request's lanes and starts, so it needs no lane of its own.
pub fn start_tracker(request: &Request) -> Option<StartTracker> {
    let units = || -> Vec<String> {
        request
            .params
            .get("units")
            .and_then(Value::as_array)
            .map(|units| {
                units
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    match request.method {
        Method::Start | Method::Restart | Method::Reload => {
            Some(StartTracker::command().with_units(units()))
        }
        Method::Resume => Some(StartTracker::command()),
        // The daemon streams only the unit a unit target names.
        Method::Wait => request
            .params
            .get("target")
            .and_then(Value::as_str)
            .map(|_| StartTracker::command()),
        _ => None,
    }
}

/// Erases the previous line on a terminal: cursor up, clear line.
const ERASE_LINE: &str = "\x1b[1A\x1b[2K";

/// Turns a human start's `… phase` line into its outcome in place: the line
/// printed when a phase began is replaced by each newer detail on it
/// (`… infra: waiting for postgres (1 of 2 healthy)`), then by `✓ phase` (or
/// erased when the phase had nothing to do), as long as nothing was printed in
/// between. In a task phase, a finished task's line (`✓ seed`) replaces the
/// `… tasks: seed` line instead.
pub struct PhaseLines {
    mode: PhaseMode,
    /// The running phase's line printed last, while it is still the last line
    /// on stdout.
    open: Option<OpenLine>,
}

/// The phase line [`PhaseLines`] may still replace.
struct OpenLine {
    /// `lane phase start_id`, the same for every event of one phase run and
    /// for the `task` events of the tasks it runs.
    key: String,
    /// The task or setup entry the line says runs (`… tasks: seed`).
    step: Option<String>,
}

/// What happens to `started` and `progress` phase lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhaseMode {
    /// Human output not on a terminal: no started lines.
    Hide,
    /// A terminal: started lines are replaced when the phase ends.
    Replace,
    /// Lines stay as rendered: verbose output interleaves stderr, so lines are
    /// never erased; plain output has no started lines of its own, only the
    /// CI log group a phase opens; JSON writes every event as one record, and
    /// agent output renders none.
    Keep,
}

impl PhaseLines {
    /// The policy for this invocation's stdout. Only human output has phase
    /// lines to hide or replace.
    pub fn new(runtime: &Runtime) -> Self {
        let mode = match runtime.mode {
            OutputMode::Human if runtime.verbose > 0 => PhaseMode::Keep,
            OutputMode::Human if runtime.stdout_tty && runtime.ci.is_none() => PhaseMode::Replace,
            OutputMode::Human => PhaseMode::Hide,
            OutputMode::Plain | OutputMode::Json | OutputMode::Agent => PhaseMode::Keep,
        };
        Self { mode, open: None }
    }

    /// Adjusts the rendered stdout of `event` before it is written.
    pub fn rewrite(&mut self, event: &devlanes_proto::Event, stdout: &mut String) {
        if self.mode == PhaseMode::Keep {
            return;
        }
        let data = &event.data;
        let field = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or_default();
        let text = |key: &str| data.get(key).map(Value::to_string).unwrap_or_default();
        let key = format!("{} {} {}", text("lane"), text("phase"), text("start_id"));
        let phase = (event.event == devlanes_proto::events::PHASE).then(|| field("state"));
        let open_key = self.open.as_ref().map(|open| open.key.as_str());
        match phase {
            Some(state @ ("started" | "progress")) => {
                if self.mode == PhaseMode::Hide {
                    stdout.clear();
                    return;
                }
                if stdout.is_empty() {
                    // A progress event without detail leaves the line as it is.
                    if state == "started" {
                        self.open = None;
                    }
                    return;
                }
                // New detail on the same phase replaces its line.
                if state == "progress" && open_key == Some(key.as_str()) {
                    stdout.insert_str(0, ERASE_LINE);
                }
                let step = data
                    .get("step")
                    .and_then(|step| step.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.open = Some(OpenLine { key, step });
            }
            Some(_) if open_key == Some(key.as_str()) => {
                stdout.insert_str(0, ERASE_LINE);
                self.open = None;
            }
            // Only finished tasks render a line; the end of the task the open
            // line says runs replaces it. The daemon sends the phase and
            // `start_id` on the tasks a start's phase runs.
            None if event.event == devlanes_proto::events::TASK && !stdout.is_empty() => {
                if self.open.as_ref().is_some_and(|open| {
                    open.key == key && open.step.as_deref() == Some(field("task"))
                }) {
                    stdout.insert_str(0, ERASE_LINE);
                }
                self.open = None;
            }
            _ if !stdout.is_empty() => self.open = None,
            _ => {}
        }
    }
}

impl Runtime {
    /// Whether this invocation draws the progress bar: only human output on a
    /// terminal, then as `mode`, [`devlanes_term::PROGRESS_ENV`] and the terminal
    /// allow. Agent, JSON, plain, CI and piped output never get it.
    pub fn progress_wrap(
        &self,
        mode: ProgressMode,
        env: impl Fn(&str) -> Option<String>,
    ) -> Option<Wrap> {
        if self.mode != OutputMode::Human || self.ci.is_some() {
            return None;
        }
        progress_support(mode, self.stdout_tty, env)
    }
}
