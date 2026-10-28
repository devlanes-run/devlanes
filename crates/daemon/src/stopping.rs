//! The steps of a daemon shutdown as clients see them: one
//! [`events::DAEMON_STOPPING`] event per change on the bus, and an INFO log
//! line with its time once each step is done, so a slow stop can be explained
//! from `daemon.log`.

use std::time::Instant;

use devlanes_proto::{DaemonStopping, StoppingState, StoppingStep, events};

use crate::{ShutdownReason, bus::EventBus};

/// Reports the steps of one shutdown.
pub(crate) struct StopSteps<'a> {
    bus: &'a EventBus,
    reason: ShutdownReason,
    restart: bool,
}

/// A step that started: what its `progress` and `done` events repeat.
pub(crate) struct StepRun {
    step: StoppingStep,
    total: Option<u64>,
    lanes: Option<u64>,
    begun: Instant,
}

impl<'a> StopSteps<'a> {
    /// Reports on `bus` for a shutdown for `reason`, a restart when `restart`.
    pub(crate) fn new(bus: &'a EventBus, reason: ShutdownReason, restart: bool) -> Self {
        Self {
            bus,
            reason,
            restart,
        }
    }

    /// Reports `step` started over `total` items in `lanes` lanes.
    pub(crate) fn started(
        &self,
        step: StoppingStep,
        total: Option<u64>,
        lanes: Option<u64>,
    ) -> StepRun {
        let run = StepRun {
            step,
            total,
            lanes,
            begun: Instant::now(),
        };
        self.emit(&run, StoppingState::Started, None, None);
        run
    }

    /// Reports `step` started over `total` items, or skipped when there are
    /// none; `None` then.
    pub(crate) fn begin(
        &self,
        step: StoppingStep,
        total: usize,
        lanes: Option<u64>,
    ) -> Option<StepRun> {
        if total == 0 {
            self.skipped(step);
            return None;
        }
        Some(self.started(step, Some(total as u64), lanes))
    }

    /// Reports that `left` items of `run` are not finished yet.
    pub(crate) fn progress(&self, run: &StepRun, left: usize) {
        self.emit(run, StoppingState::Progress, Some(left as u64), None);
    }

    /// Reports `run` done and logs how long it took.
    pub(crate) fn done(&self, run: &StepRun) {
        let elapsed_ms = run.begun.elapsed().as_millis() as u64;
        tracing::info!(
            step = name(run.step),
            count = run.total.unwrap_or(0),
            elapsed_ms,
            "shutdown step done"
        );
        self.emit(run, StoppingState::Done, None, Some(elapsed_ms));
    }

    /// Reports that `step` had nothing to do.
    pub(crate) fn skipped(&self, step: StoppingStep) {
        tracing::debug!(step = name(step), "shutdown step skipped");
        let run = StepRun {
            step,
            total: None,
            lanes: None,
            begun: Instant::now(),
        };
        self.emit(&run, StoppingState::Skipped, None, None);
    }

    /// Publishes one [`DaemonStopping`].
    fn emit(
        &self,
        run: &StepRun,
        state: StoppingState,
        left: Option<u64>,
        elapsed_ms: Option<u64>,
    ) {
        let payload = DaemonStopping {
            reason: self.reason.as_str().to_owned(),
            restart: self.restart,
            step: run.step,
            state,
            total: run.total,
            left,
            lanes: run.lanes,
            elapsed_ms,
        };
        match serde_json::to_value(payload) {
            Ok(data) => self.bus.emit(events::DAEMON_STOPPING, data),
            Err(error) => tracing::warn!(%error, "shutdown step not reported"),
        }
    }
}

/// The step's name in logs, as the event spells it.
fn name(step: StoppingStep) -> &'static str {
    match step {
        StoppingStep::Remote => "remote",
        StoppingStep::Thaw => "thaw",
        StoppingStep::Tasks => "tasks",
        StoppingStep::Units => "units",
        StoppingStep::State => "state",
        StoppingStep::Other => "other",
    }
}
