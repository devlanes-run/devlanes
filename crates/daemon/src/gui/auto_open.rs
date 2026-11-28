//! Auto-open: the daemon starts the viewer of a running GUI service whose
//! display is not this machine's desktop (every mirrored box service, and
//! this machine's own on a display of their own) while the user config's
//! `gui.auto_open` is true.
//!
//! The daemon is the one process that always runs on the viewer's machine,
//! which is why it starts the viewer although clients own rendering (an
//! accepted exception in `AGENTS.md`). The viewer stays a client process: it
//! is no unit, no member of a unit's run and not in `debug top`, and its
//! input counts as lane activity only through the GUI host, like any
//! viewer's.
//!
//! Every [`POLL`] the daemon reads its GUI services as `gui open` does
//! ([`super::lookup`]) and [`Planner::plan`] decides. A service gets one
//! viewer per period it runs: a viewer the user ended is not started again
//! until the service stopped and runs again, and one that runs already (from
//! `gui open`, or left by an earlier daemon) is not doubled, through the lock
//! of [`devlanes_gui_client::instance`]. A viewer this daemon or an earlier
//! one started ends once its service has not run for [`END_GRACE`]: a
//! restart whose new start failed leaves no host to tell it, and a box whose
//! link is gone leaves it reconnecting. A daemon that stops for good ends the
//! viewers daemons started; a restart, an update's included, leaves them
//! open for the next daemon, which takes them on. A viewer the daemon started
//! also asks the daemon itself while it cannot reconnect, so it ends when no
//! daemon comes back.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devlanes_core::ProcessIdentity;
use devlanes_gui_client::instance::{self, StartedBy};

use super::lookup;
use super::viewer::{self, Launch, Process};
use crate::engine::Inner;

/// How often the daemon looks at its GUI services.
pub(crate) const POLL: Duration = Duration::from_secs(2);

/// How long a service may not run before the viewer the daemon started for
/// it ends. Long enough for a restart and for a box link that reconnects.
pub(crate) const END_GRACE: Duration = Duration::from_secs(30);

/// A GUI service as auto-open sees it.
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    /// The service as `lane:unit`.
    pub label: String,
    /// Where its host is reached here while it runs.
    pub address: Option<SocketAddr>,
    /// Whether the service runs or is on its way up.
    pub runs: bool,
    /// Whether its display is this machine's desktop, for a local service.
    pub real_desktop: Option<bool>,
}

/// Who holds the viewer lock of a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Holder {
    /// Nobody: no viewer runs.
    Free,
    /// A viewer a daemon started, this one or an earlier one.
    Daemon,
    /// A viewer `devlanes gui open` or a person started.
    User,
}

/// What the daemon does about a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Start a viewer.
    Open {
        /// The service.
        label: String,
        /// Its host.
        address: SocketAddr,
    },
    /// End the viewer a daemon started, of the lock `key`.
    End {
        /// The service.
        label: String,
        /// Its lock.
        key: String,
    },
    /// Say once that no viewer can open: there is no display.
    NoDisplay {
        /// The service.
        label: String,
    },
}

/// A service whose viewer was opened, adopted or left to the user for the
/// period it runs.
#[derive(Debug, Clone)]
struct Opened {
    /// The lock of the viewer.
    key: String,
    /// Whether a daemon started it, so this daemon ends it.
    ours: bool,
    /// Since when the service has not run.
    gone_since: Option<Instant>,
}

/// The bookkeeping of auto-open.
#[derive(Debug, Default)]
pub(crate) struct Planner {
    opened: HashMap<String, Opened>,
    noted: HashSet<String>,
}

impl Planner {
    /// What to do at `now` about `candidates`. `holder` says who holds the
    /// lock of a key; `display` whether a viewer could open windows here.
    pub(crate) fn plan(
        &mut self,
        candidates: &[Candidate],
        holder: impl Fn(&str) -> Holder,
        display: bool,
        now: Instant,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        for candidate in candidates.iter().filter(|c| c.runs) {
            if let Some(opened) = self.opened.get_mut(&candidate.label) {
                opened.gone_since = None;
                continue;
            }
            // A window already on this machine's desktop needs no viewer,
            // and a host that does not run yet has nothing to show.
            let Some(address) = candidate.address else {
                continue;
            };
            if candidate.real_desktop == Some(true) {
                continue;
            }
            let key = instance::key(&candidate.label, address.port());
            let ours = match holder(&key) {
                Holder::Daemon => true,
                Holder::User => false,
                Holder::Free if !display => {
                    if self.noted.insert(candidate.label.clone()) {
                        actions.push(Action::NoDisplay {
                            label: candidate.label.clone(),
                        });
                    }
                    continue;
                }
                Holder::Free => {
                    actions.push(Action::Open {
                        label: candidate.label.clone(),
                        address,
                    });
                    true
                }
            };
            self.opened.insert(
                candidate.label.clone(),
                Opened {
                    key,
                    ours,
                    gone_since: None,
                },
            );
        }
        let running: HashSet<&str> = candidates
            .iter()
            .filter(|c| c.runs)
            .map(|c| c.label.as_str())
            .collect();
        self.opened.retain(|label, opened| {
            if running.contains(label.as_str()) {
                return true;
            }
            let since = *opened.gone_since.get_or_insert(now);
            if now < since + END_GRACE {
                return true;
            }
            if opened.ours {
                actions.push(Action::End {
                    label: label.clone(),
                    key: opened.key.clone(),
                });
            }
            false
        });
        actions
    }

    /// Takes on the viewer of `label` holding the lock `key`, which an
    /// earlier daemon started: it ends like one this daemon started, once
    /// its service has not run for [`END_GRACE`].
    pub(crate) fn adopt(&mut self, label: String, key: String) {
        self.opened.entry(label).or_insert(Opened {
            key,
            ours: true,
            gone_since: None,
        });
    }

    /// The locks of the viewers a daemon started, which this daemon ends on
    /// shutdown.
    pub(crate) fn ours(&self) -> Vec<String> {
        self.opened
            .values()
            .filter(|opened| opened.ours)
            .map(|opened| opened.key.clone())
            .collect()
    }
}

/// Auto-open's state in the daemon: the bookkeeping and the viewer processes
/// this daemon started, kept to reap them.
#[derive(Debug, Default)]
pub(crate) struct AutoOpen {
    planner: Planner,
    children: HashMap<String, Process>,
    /// The viewers earlier daemons left were taken on.
    adopted: bool,
}

impl AutoOpen {
    /// Once per daemon, takes on the viewers in `dir` that an earlier daemon
    /// started (before a restart, an update or a crash), so they end like
    /// this daemon's once their service stays away. Blocking.
    fn adopt_left(&mut self, dir: &std::path::Path) {
        if std::mem::replace(&mut self.adopted, true) {
            return;
        }
        for key in instance::keys(dir).unwrap_or_default() {
            if let Ok(Some(info)) = instance::holder(dir, &key)
                && info.started_by == StartedBy::Daemon
            {
                self.planner.adopt(info.service, key);
            }
        }
    }
}

impl Inner {
    /// Runs auto-open until shutdown, when GUI services and `gui.auto_open`
    /// are on.
    pub(crate) async fn auto_open_loop(self: Arc<Self>) {
        if !self.opts.gui.enabled || !self.opts.user.gui.auto_open {
            return;
        }
        let mut tick = tokio::time::interval(POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                _ = tick.tick() => {}
            }
            let candidates = self.gui_candidates().await;
            let engine = self.clone();
            let step = tokio::task::spawn_blocking(move || engine.auto_open_step(&candidates));
            match step.await {
                Ok(unseen) => {
                    for label in unseen {
                        self.note_no_display(&label).await;
                    }
                }
                Err(error) => tracing::warn!(%error, "auto-open failed"),
            }
        }
    }

    /// The GUI services of this machine's lanes and of the boxes it mirrors.
    /// Cheap, since it runs every [`POLL`]: lane summaries without metrics,
    /// and the connected boxes from memory, read only when there are some.
    async fn gui_candidates(self: &Arc<Self>) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        for (project, lane) in self.all_lanes() {
            let summary = {
                let st = lane.st();
                // Most lanes have no GUI service: no summary is built for them.
                let gui = st
                    .units()
                    .values()
                    .any(|unit| unit.ports.contains_key(devlanes_core::ports::GUI_PORT));
                if !gui || !st.listed() {
                    continue;
                }
                crate::model::lane_summary(&project.name(), &st)
            };
            candidates.extend(lookup::lane_services(&summary).into_iter().map(|service| {
                Candidate {
                    label: format!("{}:{}", summary.reference, service.unit),
                    address: service.address,
                    runs: service.runs,
                    real_desktop: service.real_desktop,
                }
            }));
        }
        if self.remote.has_boxes()
            && let Ok(boxes) = self.live_host_list().await
        {
            for found in lookup::box_lanes(&boxes) {
                candidates.extend(found.services.into_iter().map(|service| Candidate {
                    label: format!("{}:{}", found.lane, service.unit),
                    address: service.address,
                    runs: service.runs,
                    real_desktop: None,
                }));
            }
        }
        candidates
    }

    /// Says on the unit of the local service `label` that its windows did not
    /// open by themselves for lack of a display, as a launch note `status`
    /// and `debug why` show. A box's service has no unit here; the daemon log
    /// has the line either way.
    async fn note_no_display(self: &Arc<Self>, label: &str) {
        let note = format!(
            "[devlanes] gui: the windows of {label} did not open by themselves: the daemon has no \
             display (DISPLAY and WAYLAND_DISPLAY were not set where it started); run devlanes gui \
             open {label} from a desktop terminal"
        );
        tracing::warn!("{note}");
        let Some((lane_ref, unit)) = label.split_once(':') else {
            return;
        };
        for (project, lane) in self.all_lanes() {
            let host = {
                let st = lane.st();
                if st.reference(&project.name()) != lane_ref {
                    continue;
                }
                st.units().get(unit).map(|rec| rec.host.clone())
            };
            if let Some(host) = host {
                self.note_unit(&project, &lane, unit, &host, &note).await;
            }
            return;
        }
    }

    /// One auto-open pass over `candidates`. Blocking: it reads the locks
    /// and starts and ends processes. Returns the services whose windows did
    /// not open for lack of a display, said once each.
    fn auto_open_step(&self, candidates: &[Candidate]) -> Vec<String> {
        let dir = viewer::instance_dir(&self.opts.home);
        let display = viewer::display_env();
        let mut unseen = Vec::new();
        let holder = |key: &str| match instance::holder(&dir, key) {
            Ok(Some(info)) if info.started_by == StartedBy::Daemon => Holder::Daemon,
            Ok(Some(_)) => Holder::User,
            // Locked, but between taking the lock and writing its info.
            Ok(None) if instance::running(&dir, key).unwrap_or(true) => Holder::User,
            Ok(None) => Holder::Free,
            Err(error) => {
                tracing::debug!(%error, "cannot read a viewer lock");
                Holder::User
            }
        };
        let mut state = crate::model::lock(&self.gui_viewers);
        state
            .children
            .retain(|_, child| !matches!(child.exited(), Ok(Some(_))));
        state.adopt_left(&dir);
        let actions = state
            .planner
            .plan(candidates, holder, display.is_some(), Instant::now());
        for action in actions {
            match action {
                Action::Open { label, address } => {
                    let key = instance::key(&label, address.port());
                    let launch = Launch {
                        program: self.opts.gui.program.clone(),
                        address,
                        service: label.clone(),
                        instance_dir: dir.clone(),
                        auto: true,
                        // So it ends by itself when no daemon comes back.
                        devlanes: std::env::current_exe().ok(),
                        log: viewer::log_path(&self.opts.home, &key),
                        env: display.clone().unwrap_or_default(),
                    };
                    match viewer::spawn(&launch) {
                        Ok(child) => {
                            tracing::info!(service = %label, "opened the viewer");
                            state.children.insert(key, child);
                        }
                        // Not tried again for this run of the service: no auto-restart.
                        Err(error) => {
                            tracing::warn!(service = %label, %error, "the viewer did not start");
                        }
                    }
                }
                Action::End { label, key } => {
                    tracing::info!(service = %label, "ending the viewer: the service is not running");
                    end(&dir, &key, state.children.remove(&key));
                }
                Action::NoDisplay { label } => unseen.push(label),
            }
        }
        unseen
    }

    /// Ends the viewers a daemon started, for a daemon that stops for good;
    /// a restart leaves them to the next daemon. Blocking.
    pub(crate) fn end_viewers(&self) {
        let dir = viewer::instance_dir(&self.opts.home);
        let mut state = crate::model::lock(&self.gui_viewers);
        state.adopt_left(&dir);
        for key in state.planner.ours() {
            let child = state.children.remove(&key);
            end(&dir, &key, child);
        }
    }
}

/// Ends the viewer of `key` in `dir`, by the process this daemon started or
/// by the identity the viewer wrote, checked so a reused pid is never hit.
fn end(dir: &std::path::Path, key: &str, child: Option<Process>) {
    let identity = match instance::holder(dir, key) {
        Ok(Some(info)) if info.started_by == StartedBy::Daemon && info.start_time != 0 => {
            // Matching compares the pid and start time only.
            Some(ProcessIdentity {
                pid: info.pid,
                sid: 0,
                start_time: info.start_time,
            })
        }
        _ => child.as_ref().and_then(Process::identity),
    };
    if let Some(identity) = identity {
        #[cfg(unix)]
        let ended = devlanes_sys::signal_verified(&identity, devlanes_sys::Signal::TERM);
        #[cfg(windows)]
        let ended = devlanes_sys::terminate_verified(&identity, 1);
        if let Err(error) = ended {
            tracing::debug!(%error, "the viewer was not ended");
        }
    }
    // Reaps it, by its pid alone, once it is gone.
    if let Some(mut child) = child {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !matches!(child.exited(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A running service `label` at `port`, `real_desktop` as its host said.
    fn running(label: &str, port: u16, real_desktop: Option<bool>) -> Candidate {
        Candidate {
            label: label.into(),
            address: Some(SocketAddr::from(([127, 0, 0, 1], port))),
            runs: true,
            real_desktop,
        }
    }

    /// The same service, stopped: no host, no address.
    fn stopped(label: &str) -> Candidate {
        Candidate {
            label: label.into(),
            address: None,
            runs: false,
            real_desktop: None,
        }
    }

    /// A viewer opens once per period its service runs, only for a display
    /// that is not this machine's desktop and only while the lock is free;
    /// it ends after the service stayed stopped for the grace, and a new run
    /// opens it again. A viewer of the user's is never ended.
    #[test]
    fn viewers_open_once_per_run_and_end_with_their_service() {
        let t0 = Instant::now();
        let free = |_: &str| Holder::Free;
        let mut planner = Planner::default();
        let services = [
            running("acme@2:desktop", 26012, None),
            running("acme@1:desktop", 26002, Some(false)),
            running("acme@1:windows-app", 26003, Some(true)),
            Candidate {
                address: None,
                ..running("acme@1:starting", 26004, None)
            },
        ];
        let actions = planner.plan(&services, free, true, t0);
        let opened: Vec<&str> = actions
            .iter()
            .filter_map(|action| match action {
                Action::Open { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(opened, ["acme@2:desktop", "acme@1:desktop"]);
        assert_eq!(planner.plan(&services, free, true, t0), [], "once per run");

        // A restart: stopped for a moment, then running again with its viewer.
        let restarting = [stopped("acme@2:desktop"), services[1].clone()];
        assert_eq!(planner.plan(&restarting, free, true, t0), []);
        let back = t0 + END_GRACE - Duration::from_secs(1);
        assert_eq!(planner.plan(&services[..2], free, true, back), []);

        // Stopped for good: the viewer ends after the grace, and opens again
        // with the next run.
        let later = back + Duration::from_secs(1);
        assert_eq!(planner.plan(&restarting, free, true, later), []);
        let ended = planner.plan(&restarting, free, true, later + END_GRACE);
        assert_eq!(
            ended,
            [Action::End {
                label: "acme@2:desktop".into(),
                key: instance::key("acme@2:desktop", 26012),
            }]
        );
        let again = planner.plan(&services[..2], free, true, later + END_GRACE);
        assert!(matches!(&again[..], [Action::Open { label, .. }] if label == "acme@2:desktop"));
    }

    /// A viewer that runs already is not doubled: a daemon's (an earlier
    /// daemon's) is adopted and ended with its service, the user's is left
    /// alone. Without a display nothing opens, which is said once.
    #[test]
    fn running_viewers_are_adopted_or_left_and_no_display_is_said_once() {
        let t0 = Instant::now();
        let services = [
            running("acme@2:desktop", 26012, None),
            running("acme@2:editor", 26013, None),
        ];
        let ours = instance::key("acme@2:desktop", 26012);
        let holder = |key: &str| {
            if key == ours {
                Holder::Daemon
            } else {
                Holder::User
            }
        };
        let mut planner = Planner::default();
        assert_eq!(planner.plan(&services, holder, true, t0), []);
        let gone = [stopped("acme@2:desktop"), stopped("acme@2:editor")];
        planner.plan(&gone, holder, true, t0);
        let ended = planner.plan(&gone, holder, true, t0 + END_GRACE);
        assert_eq!(
            ended,
            [Action::End {
                label: "acme@2:desktop".into(),
                key: ours.clone(),
            }],
            "the user's viewer is not ended"
        );

        let mut planner = Planner::default();
        let free = |_: &str| Holder::Free;
        let first = planner.plan(&services[..1], free, false, t0);
        assert_eq!(
            first,
            [Action::NoDisplay {
                label: "acme@2:desktop".into()
            }]
        );
        assert_eq!(planner.plan(&services[..1], free, false, t0), []);
    }

    /// A viewer an earlier daemon left is taken on: it stays while its
    /// service comes back, and ends once the service has stayed away for the
    /// grace, also when no candidate ever named it.
    #[test]
    fn viewers_left_by_an_earlier_daemon_end_with_their_service() {
        let t0 = Instant::now();
        let free = |_: &str| Holder::Free;
        let key = instance::key("acme@2:desktop", 26012);
        let mut planner = Planner::default();
        planner.adopt("acme@2:desktop".into(), key.clone());
        let back = [running("acme@2:desktop", 26012, None)];
        assert_eq!(planner.plan(&back, free, true, t0), [], "kept, not doubled");
        assert_eq!(planner.plan(&[], free, true, t0), []);
        assert_eq!(
            planner.plan(&[], free, true, t0 + END_GRACE),
            [Action::End {
                label: "acme@2:desktop".into(),
                key,
            }]
        );
    }
}
