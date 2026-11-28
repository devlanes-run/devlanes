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
//! of [`devlanes_gui_client::instance`]; a viewer that ended with its
//! service's last run is no viewer the user ended. A viewer this daemon or
//! an earlier one started ends once its service has not run for
//! [`END_GRACE`], counted only while its box is connected: a
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
    /// The box it runs on, for a box's lane.
    pub host_box: Option<String>,
    /// When its current run started (Unix ms), for a local service: a new
    /// value is a new run. A box's registry has none.
    pub run: Option<u64>,
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
        /// The box it runs on, for a box's lane.
        host_box: Option<String>,
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

/// How long after its service runs again a viewer of the run before may
/// still end by itself: it ends 3 seconds after its host said Closed. A free
/// lock in that time means the service gets a new viewer.
const CARRY_OVER: Duration = Duration::from_secs(10);

/// A service whose viewer was opened, adopted or left to the user for the
/// period it runs.
#[derive(Debug, Clone)]
struct Opened {
    /// The lock of the viewer.
    key: String,
    /// Whether a daemon started it, so this daemon ends it.
    ours: bool,
    /// The box the service runs on, for a box's lane.
    host_box: Option<String>,
    /// Since when the service has not run.
    gone_since: Option<Instant>,
    /// The service was seen not running since the viewer opened, and since
    /// when it runs again: a viewer that ended meanwhile is no window the
    /// user closed in this run.
    stopped: Option<Option<Instant>>,
    /// The run of the service the viewer is for, by [`Candidate::run`].
    run: Option<u64>,
}

/// The bookkeeping of auto-open.
#[derive(Debug, Default)]
pub(crate) struct Planner {
    opened: HashMap<String, Opened>,
    noted: HashSet<String>,
    /// The boxes this machine knows but is not connected to: their services
    /// count as running, since nothing is known about them.
    away: HashSet<String>,
    /// Every box counts as away: the daemon is still reopening them.
    restoring: bool,
}

impl Planner {
    /// The boxes known but not connected for the next [`Planner::plan`],
    /// or every box while `restoring`.
    pub(crate) fn set_away(&mut self, away: HashSet<String>, restoring: bool) {
        self.away = away;
        self.restoring = restoring;
    }

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
                // A stop and start quicker than one pass is a new run all
                // the same: the service's start time changed.
                if opened.run.is_some() && candidate.run.is_some() && opened.run != candidate.run {
                    opened.stopped.get_or_insert(None);
                }
                opened.run = candidate.run.or(opened.run);
                let Some(stopped) = &mut opened.stopped else {
                    continue;
                };
                let back = *stopped.get_or_insert(now);
                if holder(&opened.key) != Holder::Free {
                    // The viewer carried over into this run once it outlived
                    // its own end after a Closed.
                    if now >= back + CARRY_OVER {
                        opened.stopped = None;
                    }
                    continue;
                }
                self.opened.remove(&candidate.label);
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
                        host_box: candidate.host_box.clone(),
                    });
                    true
                }
            };
            self.opened.insert(
                candidate.label.clone(),
                Opened {
                    key,
                    ours,
                    host_box: candidate.host_box.clone(),
                    gone_since: None,
                    stopped: None,
                    run: candidate.run,
                },
            );
        }
        let running: HashSet<&str> = candidates
            .iter()
            .filter(|c| c.runs)
            .map(|c| c.label.as_str())
            .collect();
        let (away, restoring) = (&self.away, self.restoring);
        self.opened.retain(|label, opened| {
            if running.contains(label.as_str()) {
                return true;
            }
            // A box that is not connected (yet) says nothing about its
            // services: the grace counts only while it is connected.
            if opened
                .host_box
                .as_ref()
                .is_some_and(|host_box| restoring || away.contains(host_box))
            {
                opened.gone_since = None;
                return true;
            }
            opened.stopped = Some(None);
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

    /// Takes on the viewer of `label` (on `host_box`, for a box's lane)
    /// holding the lock `key`, which an earlier daemon started: it ends like
    /// one this daemon started, once its service has not run for
    /// [`END_GRACE`].
    pub(crate) fn adopt(&mut self, label: String, key: String, host_box: Option<String>) {
        self.opened.entry(label).or_insert(Opened {
            key,
            ours: true,
            host_box,
            gone_since: None,
            stopped: None,
            run: None,
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
                self.planner.adopt(info.service, key, info.host_box);
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
            let restoring = self.remote.restoring();
            let (candidates, away) = self.gui_candidates().await;
            let engine = self.clone();
            let step = tokio::task::spawn_blocking(move || {
                engine.auto_open_step(&candidates, away, restoring)
            });
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

    /// The GUI services of this machine's lanes and of the boxes it mirrors,
    /// and the boxes with a connection that is not up. Cheap, since it runs
    /// every [`POLL`]: lane summaries without metrics, and the box
    /// connections from memory, read only when there are some.
    async fn gui_candidates(self: &Arc<Self>) -> (Vec<Candidate>, HashSet<String>) {
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
                let run = summary
                    .units
                    .iter()
                    .find(|unit| unit.name == service.unit)
                    .and_then(|unit| unit.started_at_ms);
                Candidate {
                    label: format!("{}:{}", summary.reference, service.unit),
                    address: service.address,
                    runs: service.runs,
                    real_desktop: service.real_desktop,
                    host_box: None,
                    run,
                }
            }));
        }
        let mut away = HashSet::new();
        if self.remote.has_boxes()
            && let Ok(boxes) = self.live_host_list().await
        {
            for found in lookup::box_lanes(&boxes) {
                candidates.extend(found.services.into_iter().map(|service| Candidate {
                    label: format!("{}:{}", found.lane, service.unit),
                    address: service.address,
                    runs: service.runs,
                    real_desktop: None,
                    host_box: found.host_box.clone(),
                    run: None,
                }));
            }
            away = boxes["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|host| host["state"] != "connected")
                .filter_map(|host| host["host"].as_str().map(str::to_owned))
                .collect();
        }
        (candidates, away)
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
    fn auto_open_step(
        &self,
        candidates: &[Candidate],
        away: HashSet<String>,
        restoring: bool,
    ) -> Vec<String> {
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
        state.planner.set_away(away, restoring);
        let actions = state
            .planner
            .plan(candidates, holder, display.is_some(), Instant::now());
        for action in actions {
            match action {
                Action::Open {
                    label,
                    address,
                    host_box,
                } => {
                    let key = instance::key(&label, address.port());
                    let launch = Launch {
                        program: self.opts.gui.program.clone(),
                        address,
                        service: label.clone(),
                        instance_dir: dir.clone(),
                        auto: true,
                        // So it ends by itself when no daemon comes back.
                        devlanes: std::env::current_exe().ok(),
                        host_box,
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
            host_box: None,
            run: None,
        }
    }

    /// The same service, stopped: no host, no address.
    fn stopped(label: &str) -> Candidate {
        Candidate {
            label: label.into(),
            address: None,
            runs: false,
            real_desktop: None,
            host_box: None,
            run: None,
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

        // A restart: stopped for a moment, then running again with its
        // viewer, which still holds the lock.
        let held = |_: &str| Holder::Daemon;
        let restarting = [stopped("acme@2:desktop"), services[1].clone()];
        assert_eq!(planner.plan(&restarting, held, true, t0), []);
        let back = t0 + END_GRACE - Duration::from_secs(1);
        assert_eq!(planner.plan(&services[..2], held, true, back), []);
        assert_eq!(
            planner.plan(&services[..2], held, true, back + CARRY_OVER),
            []
        );

        // Stopped for good: the viewer ends after the grace, and opens again
        // with the next run.
        let later = back + CARRY_OVER + Duration::from_secs(1);
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
        planner.adopt("acme@2:desktop".into(), key.clone(), None);
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

    /// A service stopped and started again within the grace gets a new
    /// viewer once the old one ended by itself (3 seconds after its host
    /// said Closed). A viewer the user ended during a run, with no stop in
    /// between, stays ended for that run, also after the old viewer
    /// carried over into a new run.
    #[test]
    fn a_quick_restart_reopens_a_viewer_that_ended_with_the_old_run() {
        let t0 = Instant::now();
        let viewer = std::cell::Cell::new(Holder::Free);
        let holder = |_: &str| viewer.get();
        let services = [running("acme@2:desktop", 26012, None)];
        let opens = |actions: &[Action]| matches!(actions, [Action::Open { .. }]);
        let mut planner = Planner::default();
        assert!(opens(&planner.plan(&services, holder, true, t0)));
        viewer.set(Holder::Daemon);

        // The user ends the viewer during the run: nothing reopens it.
        viewer.set(Holder::Free);
        let t1 = t0 + Duration::from_secs(5);
        assert_eq!(planner.plan(&services, holder, true, t1), []);

        // Stopped, and running again 4 seconds later: the old viewer ended
        // by itself meanwhile, so a new one opens.
        let stop = [stopped("acme@2:desktop")];
        assert_eq!(planner.plan(&stop, holder, true, t1), []);
        let t2 = t1 + Duration::from_secs(4);
        assert!(opens(&planner.plan(&services, holder, true, t2)));
        viewer.set(Holder::Daemon);

        // A viewer that outlived the restart carries over; ended by the
        // user later in that run, it stays ended.
        assert_eq!(planner.plan(&stop, holder, true, t2), []);
        let t3 = t2 + Duration::from_secs(2);
        assert_eq!(planner.plan(&services, holder, true, t3), []);
        assert_eq!(planner.plan(&services, holder, true, t3 + CARRY_OVER), []);
        viewer.set(Holder::Free);
        let t4 = t3 + CARRY_OVER + Duration::from_secs(1);
        assert_eq!(planner.plan(&services, holder, true, t4), []);
    }

    /// A stop and start between two passes is a new run by its start time:
    /// once the old viewer ended (after its host's Closed) a new one opens.
    /// A viewer the user ended in the same run, with the same start time,
    /// is not reopened.
    #[test]
    fn a_restart_between_passes_is_a_new_run() {
        let t0 = Instant::now();
        let viewer = std::cell::Cell::new(Holder::Free);
        let holder = |_: &str| viewer.get();
        let run = |started| Candidate {
            run: Some(started),
            ..running("acme@1:desktop", 26002, Some(false))
        };
        let opens = |actions: &[Action]| matches!(actions, [Action::Open { .. }]);
        let mut planner = Planner::default();
        assert!(opens(&planner.plan(&[run(1)], holder, true, t0)));
        viewer.set(Holder::Daemon);
        // The user ends the viewer: the same run, nothing reopens.
        viewer.set(Holder::Free);
        assert_eq!(planner.plan(&[run(1)], holder, true, t0), []);
        // Restarted between passes; the old viewer still lingers on Closed.
        viewer.set(Holder::Daemon);
        let t1 = t0 + POLL;
        assert_eq!(planner.plan(&[run(2)], holder, true, t1), []);
        viewer.set(Holder::Free);
        assert!(opens(&planner.plan(&[run(2)], holder, true, t1 + POLL)));
    }

    /// The grace of a box's viewer counts only while the box is connected:
    /// a box that is away, or every box while the daemon still reopens
    /// them, counts as running; once connected without the service, the
    /// grace runs.
    #[test]
    fn a_box_that_is_away_keeps_its_viewers() {
        let t0 = Instant::now();
        let free = |_: &str| Holder::Free;
        let key = instance::key("acme@2:desktop", 26012);
        let mut planner = Planner::default();
        planner.adopt("acme@2:desktop".into(), key.clone(), Some("hetzner".into()));
        planner.set_away(HashSet::new(), true);
        assert_eq!(planner.plan(&[], free, true, t0), []);
        planner.set_away(HashSet::from(["hetzner".to_owned()]), false);
        assert_eq!(planner.plan(&[], free, true, t0 + END_GRACE), []);
        assert_eq!(planner.plan(&[], free, true, t0 + 3 * END_GRACE), []);
        // Another box being away does not keep it.
        planner.set_away(HashSet::from(["other".to_owned()]), false);
        let connected = t0 + 4 * END_GRACE;
        assert_eq!(planner.plan(&[], free, true, connected), []);
        assert_eq!(
            planner.plan(&[], free, true, connected + END_GRACE),
            [Action::End {
                label: "acme@2:desktop".into(),
                key,
            }]
        );
    }
}
